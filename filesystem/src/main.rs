// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent::filesystem::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{AddLocalUser, RemoveLocalUser};
use templemeads::job::{Envelope, Job};
use templemeads::Error;

mod cache;
mod filesystem;

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

    cache::set_home_root(&config.option("home-root", "/home")).await?;
    cache::set_home_permissions(&config.option("home-permissions", "0755")).await?;

    cache::set_project_roots(
        &config
            .option("project-roots", "/project")
            .split(":")
            .map(|s| s.to_owned())
            .collect(),
    )
    .await?;

    cache::set_project_permissions(
        &config
            .option("project-permissions", "2770")
            .split(":")
            .map(|s| s.to_owned())
            .collect(),
    )
    .await?;

    cache::set_project_links(
        &config
            .option("project-links", "")
            .split(":")
            .map(|s| s.to_owned())
            .collect(),
    )
    .await?;

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
                    // home directory is, e.g. /home/project/user
                    let home_root = format!("{}/{}", cache::get_home_root().await?, mapping.user().project());
                    let home_dir = format!("{}/{}", home_root, mapping.local_user());
                    let home_permissions = cache::get_home_permissions().await?;

                    let project_dirs = cache::get_project_roots().await?;
                    let project_permissions = cache::get_project_permissions().await?;
                    let project_links = cache::get_project_links().await?;

                    if project_dirs.len() != project_permissions.len() {
                        return Err(Error::Misconfigured(
                            "Number of project directories does not match number of permissions".to_owned(),
                        ));
                    }

                    if project_dirs.len() != project_links.len() {
                        return Err(Error::Misconfigured(
                            "Number of project directories does not match number of links".to_owned(),
                        ));
                    }

                    // create the root in which the user's home directory will be created - this is /{home_root}/{project}
                    filesystem::create_project_dir(&home_root, mapping.local_project(),
                                                   "0755").await?;

                    filesystem::create_home_dir(&home_dir, mapping.local_user(),
                                                mapping.local_project(),
                                                &home_permissions).await?;


                    // create the project directories
                    for i in 0..project_dirs.len() {
                        let project_dir = format!("{}/{}", project_dirs[i], mapping.user().project());
                        filesystem::create_project_dir(&project_dir, mapping.local_project(),
                                                       &project_permissions[i]).await?;
                    }

                    // now create any necessary project links
                    for i in 0..project_links.len() {
                        if let Some(link) = project_links[i].as_ref() {
                            filesystem::create_project_link(&format!("{}/{}", project_dirs[i], mapping.user().project()),
                                                            link, &mapping.user().project()).await?;
                        }
                    }

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
