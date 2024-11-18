// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent::scheduler::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{AddLocalUser, RemoveLocalUser};
use templemeads::grammar::{UserIdentifier, UserMapping};
use templemeads::job::{Envelope, Job};
use templemeads::Error;

mod cache;
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
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

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
    let token_command = config.option("token-command", "");
    let slurm_server = config.option("slurm-server", "");
    let slurm_user = config.option("slurm-user", "");

    if token_command.is_empty() {
        return Err(anyhow::anyhow!(
            "No token command provided. This should be the command needed to \
             generate a valid JWT token. Set this in the token-command \
             option."
                .to_owned(),
        ));
    }

    if slurm_server.is_empty() {
        return Err(anyhow::anyhow!(
            "No Slurm server specified. Please set this in the slurm-server option.".to_owned(),
        ));
    }

    // connect the single shared Slurm client - this will be used in the
    // async function (we can't bind variables to async functions, or else
    // we would just pass the client with the environment)
    slurm::connect(&slurm_server, &slurm_user, &token_command).await?;

    tracing::info!("Connected to slurm server at {}", slurm_server);

    let mapping = UserMapping::new(
        &UserIdentifier::parse("chris.demo.brics")?,
        "chris.demo",
        "demo",
    )?;

    slurm::add_user(&mapping).await?;

    async_runnable! {
        ///
        /// Runnable function that will be called when a job is received
        /// by the agent
        ///
        pub async fn slurm_runner(envelope: Envelope) -> Result<Job, templemeads::Error>
        {
            let job = envelope.job();

            match job.instruction() {
                AddLocalUser(user) => {
                    slurm::add_user(&user).await?;
                    let job = job.completed("Success!")?;
                    Ok(job)
                },
                RemoveLocalUser(mapping) => {
                    Err(Error::IncompleteCode(
                        format!("RemoveLocalUser instruction not implemented yet - cannot remove {}", mapping),
                    ))
                },
                _ => {
                    Err(Error::InvalidInstruction(
                        format!("Invalid instruction: {}. Slurm only supports add_local_user and remove_local_user", job.instruction()),
                    ))
                }
            }
        }
    }

    run(config, slurm_runner).await?;

    Ok(())
}
