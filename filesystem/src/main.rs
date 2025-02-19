// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent::filesystem::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{
    AddLocalProject, AddLocalUser, GetLocalHomeDir, GetLocalProjectDirs, RemoveLocalProject,
    RemoveLocalUser,
};
use templemeads::grammar::ProjectMapping;
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
    templemeads::config::initialise_tracing();

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
                AddLocalProject(mapping) => {
                    let home_root = create_project_dirs_and_links(&mapping).await?;
                    job.completed(home_root)
                },
                RemoveLocalProject(mapping) => {
                    tracing::warn!("RemoveLocalProject instruction not implemented yet - not actually removing {}", mapping);
                    job.completed_none()
                },
                AddLocalUser(mapping) => {
                    // make sure all project dirs are created, and get back the
                    // project home root
                    let home_root = create_project_dirs_and_links(&mapping.clone().into()).await?;

                    // create the home directory is, e.g. /home_root/user
                    let home_dir = format!("{}/{}", home_root, mapping.local_user());
                    let home_permissions = cache::get_home_permissions().await?;

                    filesystem::create_home_dir(&home_dir, mapping.local_user(),
                                                mapping.local_group(),
                                                &home_permissions).await?;

                    // update the job with the user's home directory
                    job.completed(home_dir)
                },
                RemoveLocalUser(mapping) => {
                    tracing::info!("Will remove user files of {} when the project is removed", mapping);
                    job.completed_none()
                },
                GetLocalHomeDir(mapping) => {
                    let home_root = get_home_root(&mapping.clone().into()).await?;
                    let home_dir = format!("{}/{}", home_root, mapping.local_user());
                    job.completed(home_dir)
                },
                GetLocalProjectDirs(mapping) => {
                    let project_dirs = get_project_dirs_and_links(&mapping).await?;
                    job.completed(project_dirs)
                },
                _ => {
                    Err(Error::InvalidInstruction(
                        format!("Invalid instruction: {}. Filesystem only supports add_local_user and remove_local_user", job.instruction()),
                    ))
                }
            }
        }
    }

    run(config, filesystem_runner).await?;

    Ok(())
}

///
/// Return the root directory for all users in the passed project
///
async fn get_home_root(mapping: &ProjectMapping) -> Result<String, Error> {
    // The name of the project directory comes from the project part of the ProjectIdentifier
    // Eventually we would need to encode the portal into this...
    let project_dir_name = mapping.project().project();

    let home_root = cache::get_home_root().await?;

    Ok(format!("{}/{}", home_root, project_dir_name))
}

///
/// Return the paths to all of the project directories (including links)
///
async fn get_project_dirs_and_links(mapping: &ProjectMapping) -> Result<Vec<String>, Error> {
    // The name of the project directory comes from the project part of the ProjectIdentifier
    // Eventually we would need to encode the portal into this...
    let project_dir_name = mapping.project().project();

    let project_dirs = cache::get_project_roots().await?;
    let project_links = cache::get_project_links().await?;

    if project_dirs.len() != project_links.len() {
        return Err(Error::Misconfigured(
            "Number of project directories does not match number of links".to_owned(),
        ));
    }

    let mut dirs = Vec::new();

    // Get the name of the project dirs
    for dir in project_dirs {
        let project_dir = format!("{}/{}", dir, project_dir_name);
        dirs.push(project_dir);
    }

    // And also the links
    for link in project_links.into_iter().flatten() {
        dirs.push(filesystem::get_project_link(&link, &project_dir_name).await?);
    }

    Ok(dirs)
}

///
/// Create the project directories and links for a given ProjectMapping,
/// returning the home root directory for the project
///
async fn create_project_dirs_and_links(mapping: &ProjectMapping) -> Result<String, Error> {
    // The name of the project directory comes from the project part of the ProjectIdentifier
    // Eventually we would need to encode the portal into this...
    let project_dir_name = mapping.project().project();

    // The group name for any project dirs are the mapping local group IDs
    let group_name = mapping.local_group();

    // home directory is, e.g. /home/project/user
    let home_root = format!("{}/{}", cache::get_home_root().await?, project_dir_name);

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
    filesystem::create_project_dir(&home_root, group_name, "0755").await?;

    // create the project directories
    for i in 0..project_dirs.len() {
        let project_dir = format!("{}/{}", project_dirs[i], project_dir_name);
        filesystem::create_project_dir(&project_dir, group_name, &project_permissions[i]).await?;
    }

    // now create any necessary project links
    for i in 0..project_links.len() {
        if let Some(link) = project_links[i].as_ref() {
            filesystem::create_project_link(
                &format!("{}/{}", project_dirs[i], project_dir_name),
                link,
                &project_dir_name,
            )
            .await?;
        }
    }

    // return the home root
    Ok(home_root)
}
