// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent::scheduler::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{
    AddLocalProject, AddLocalUser, GetLocalLimit, GetLocalUsageReport, RemoveLocalProject,
    RemoveLocalUser, SetLocalLimit,
};
use templemeads::job::{Envelope, Job};
use templemeads::Error;

mod cache;
mod sacctmgr;
mod slurm;

///
/// Main function for the slurm scheduler application
///
/// The main purpose of this program is to do the work of creating
/// slurm accounts and adding users to those accounts. Plus
/// (in the future) communicating with the slurm controller to
/// do job accounting, set up qos limits etc.
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    templemeads::config::initialise_tracing();

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("slurm".to_owned()),
        Some(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("slurm-config.toml"),
        ),
        Some("ws://localhost:8048".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8048),
        None,
        None,
        Some(AgentType::Scheduler),
    );

    // now parse the command line arguments to get the service configuration
    let config = match process_args(&defaults).await? {
        Some(config) => config,
        None => {
            // Not running the service, so can safely exit
            return Ok(());
        }
    };

    // get the extra options needed for the slurm scheduler
    let slurm_default_node = config.option("slurm-default-node", "");

    if slurm_default_node.is_empty() {
        return Err(anyhow::anyhow!(
            "No default node provided. This should be a JSON representation of the default node type. Set this in the slurm-default-node option."
                .to_owned(),
        ));
    }

    // parse slurm_default_node as json...
    let slurm_default_node = match serde_json::from_str(&slurm_default_node) {
        Ok(node) => node,
        Err(e) => {
            return Err(anyhow::anyhow!(format!(
                "Invalid default node provided. This should be a JSON object representing the default node type that jobs will be submitted to. Set this in the slurm-default-node option: {}",
                 e)));
        }
    };

    cache::set_default_node(&slurm::SlurmNode::construct(&slurm_default_node)?).await?;

    let slurm_cluster = config.option("slurm-cluster", "");

    if !slurm_cluster.is_empty() {
        cache::set_cluster(&slurm_cluster).await?;
    }

    let slurm_server = config.option("slurm-server", "");

    // get the sacct, sacctmgr and scontrol commands - we may need these even if
    // we are using the REST API
    let sacct_command = config.option("sacct", "sacct");
    let sacctmgr_command = config.option("sacctmgr", "sacctmgr");
    let scontrol_command = config.option("scontrol", "scontrol");

    sacctmgr::set_commands(&sacct_command, &sacctmgr_command, &scontrol_command).await;

    if slurm_server.is_empty() {
        // we are using sacctmgr and the commandline to interact
        // with slurm, because slurmrestd is not available
        sacctmgr::find_cluster().await?;

        async_runnable! {
            ///
            /// Runnable function that will be called when a job is received
            /// by the agent
            ///
            pub async fn sacctmgr_runner(envelope: Envelope) -> Result<Job, templemeads::Error>
            {
                let job = envelope.job();

                match job.instruction() {
                    AddLocalProject(project) => {
                        sacctmgr::add_project(&project).await?;
                        job.completed_none()
                    },
                    RemoveLocalProject(project) => {
                        // we won't remove the project for now, as we want to
                        // make sure that the statistics are preserved. Will eventually
                        // disable the project instead.
                        tracing::warn!("RemoveLocalProject instruction not implemented yet - not actually removing {}", project);
                        job.completed_none()
                    },
                    AddLocalUser(user) => {
                        sacctmgr::add_user(&user).await?;
                        job.completed_none()
                    },
                    RemoveLocalUser(mapping) => {
                        // we won't remove the user for now, as we want to
                        // make sure that the statistics are preserved. Will eventually
                        // disable the user instead. Note that they are already
                        // disabled in FreeIPA, so cannot submit jobs to this account
                        tracing::warn!("RemoveLocalUser instruction not implemented yet - not actually removing {}", mapping);
                        job.completed_none()
                    },
                    GetLocalUsageReport(mapping, dates) => {
                        let report = sacctmgr::get_usage_report(&mapping, &dates).await?;
                        job.completed(report)
                    }
                    GetLocalLimit(mapping) => {
                        let limit = sacctmgr::get_limit(&mapping).await?;
                        job.completed(limit)
                    }
                    SetLocalLimit(mapping, limit) => {
                        let limit = sacctmgr::set_limit(&mapping, &limit).await?;
                        job.completed(limit)
                    }
                    _ => {
                        Err(Error::InvalidInstruction(
                            format!("Invalid instruction: {}. Slurm only supports add_local_user and remove_local_user", job.instruction()),
                        ))
                    }
                }
            }
        }

        run(config, sacctmgr_runner).await?;
    } else {
        // we will use slurmrestd to interact with slurm
        let slurm_user = config.option("slurm-user", "");
        let token_command = config.option("token-command", "");
        let token_lifespan = config.option("token-lifespan", "1800");

        let mut token_lifespan: u32 = match token_lifespan.parse() {
            Ok(lifespan) => lifespan,
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "Invalid token lifespan provided. This should be a number of seconds."
                        .to_owned(),
                ));
            }
        };

        if token_lifespan < 10 {
            tracing::warn!("Cannot set the token lifespan to less than 10 seconds.");
            tracing::warn!("Setting it to a minimum of 10 seconds...");
            token_lifespan = 10;
        }

        if token_command.is_empty() {
            return Err(anyhow::anyhow!(
                "No token command provided. This should be the command needed to \
             generate a valid JWT token. Set this in the token-command \
             option."
                    .to_owned(),
            ));
        }

        // connect the single shared Slurm client - this will be used in the
        // async function (we can't bind variables to async functions, or else
        // we would just pass the client with the environment)
        slurm::connect(&slurm_server, &slurm_user, &token_command, token_lifespan).await?;

        tracing::info!("Connected to slurm server at {}", slurm_server);

        async_runnable! {
            ///
            /// Runnable function that will be called when a job is received
            /// by the agent
            ///
            pub async fn slurm_runner(envelope: Envelope) -> Result<Job, templemeads::Error>
            {
                let job = envelope.job();

                match job.instruction() {
                    AddLocalProject(project) => {
                        slurm::add_project(&project).await?;
                        job.completed_none()
                    },
                    RemoveLocalProject(project) => {
                        // we won't remove the project for now, as we want to
                        // make sure that the statistics are preserved. Will eventually
                        // disable the project instead.
                        tracing::warn!("RemoveLocalProject instruction not implemented yet - not actually removing {}", project);
                        job.completed_none()
                    },
                    AddLocalUser(user) => {
                        slurm::add_user(&user).await?;
                        job.completed_none()
                    },
                    RemoveLocalUser(mapping) => {
                        // we won't remove the user for now, as we want to
                        // make sure that the statistics are preserved. Will eventually
                        // disable the user instead. Note that they are already
                        // disabled in FreeIPA, so cannot submit jobs to this account
                        tracing::warn!("RemoveLocalUser instruction not implemented yet - not actually removing {}", mapping);
                        job.completed_none()
                    },
                    GetLocalUsageReport(mapping, dates) => {
                        // use sacctmgr for now, as we need to validate the API response
                        let report = slurm::get_usage_report(&mapping, &dates).await?;
                        job.completed(report)
                    }
                    GetLocalLimit(mapping) => {
                        let limit = slurm::get_limit(&mapping).await?;
                        job.completed(limit)
                    }
                    SetLocalLimit(mapping, limit) => {
                        let limit = slurm::set_limit(&mapping, &limit).await?;
                        job.completed(limit)
                    }
                    _ => {
                        Err(Error::InvalidInstruction(
                            format!("Invalid instruction: {}. Slurm only supports add_local_user and remove_local_user", job.instruction()),
                        ))
                    }
                }
            }
        }

        run(config, slurm_runner).await?;
    }

    Ok(())
}
