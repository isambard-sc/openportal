// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent::filesystem::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{AddLocalUser, RemoveLocalUser};
use templemeads::job::{Envelope, Job};
use templemeads::Error;

///
/// Main function for the filesystem application
///
/// The main purpose of this program is to do the work of creating user
/// and project directories on a filesystem, and setting the correct
/// permissions. This way, only a single agent needs high level access
/// to the filesystem.
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("filesystem".to_owned()),
        Some(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("filesystem-config.toml"),
        ),
        Some("ws://localhost:8047".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8047),
        None,
        None,
        Some(AgentType::Filesystem),
    );

    // now parse the command line arguments to get the service configuration
    let config = match process_args(&defaults).await? {
        Some(config) => config,
        None => {
            // Not running the service, so can safely exit
            return Ok(());
        }
    };

    // get the details about the filesystem
    // let home_root = config.option("home-root", "/home");
    // let project_root: String = config.option("project-root", "/projects");

    async_runnable! {
        ///
        /// Runnable function that will be called when a job is received
        /// by the agent
        ///
        pub async fn filesystem_runner(envelope: Envelope) -> Result<Job, templemeads::Error>
        {
            let job = envelope.job();

            match job.instruction() {
                AddLocalUser(mapping) => {
                    let home_dir = format!("/shared/home/{}", mapping.local_user());
                    let project_dir = format!("/projects/{}", mapping.user().project());

                    tracing::info!("Creating directories for {} - home = {}, project = {}",
                                   mapping.user(), home_dir, project_dir);

                    // update the job with the user's home directory
                    let job = job.completed(home_dir)?;

                    Ok(job)
                },
                RemoveLocalUser(mapping) => {
                    Err(Error::IncompleteCode(
                        format!("RemoveUser instruction not implemented yet - cannot remove {}", mapping),
                    ))
                },
                _ => {
                    Err(Error::InvalidInstruction(
                        format!("Invalid instruction: {}. Filesystem only supports add_user and remove_user", job.instruction()),
                    ))
                }
            }
        }
    }

    run(config, filesystem_runner).await?;

    Ok(())
}
