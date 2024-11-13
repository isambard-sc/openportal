// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent::scheduler::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{AddLocalUser, RemoveLocalUser};
use templemeads::job::{Envelope, Job};
use templemeads::Error;

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

    async_runnable! {
        ///
        /// Runnable function that will be called when a job is received
        /// by the agent
        ///
        pub async fn slurm_runner(envelope: Envelope) -> Result<Job, templemeads::Error>
        {
            let job = envelope.job();

            match job.instruction() {
                AddLocalUser(mapping) => {
                    Err(Error::IncompleteCode(
                        format!("AddLocalUser instruction not implemented yet - cannot remove {}", mapping),
                    ))
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
