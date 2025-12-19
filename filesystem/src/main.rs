// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent::filesystem::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{
    AddLocalProject, AddLocalUser, GetLocalHomeDir, GetLocalProjectDirs, GetLocalProjectQuota,
    GetLocalProjectQuotas, GetLocalUserQuota, GetLocalUserQuotas, RemoveLocalProject,
    RemoveLocalUser, SetLocalProjectQuota, SetLocalUserQuota,
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

    // start system monitoring
    templemeads::spawn_system_monitor();

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

    cache::set_home_roots(
        &config
            .option("home-root", "/home")
            .split(":")
            .map(|s| s.to_owned())
            .collect(),
    )
    .await?;

    cache::set_home_permissions(
        &config
            .option("home-permissions", "0755")
            .split(":")
            .map(|s| s.to_owned())
            .collect(),
    )
    .await?;

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
                    let home_roots = create_project_dirs_and_links(&mapping).await?;
                    job.completed(home_roots.first().cloned().unwrap_or_default())
                },
                RemoveLocalProject(mapping) => {
                    remove_project_dirs_and_links(&mapping).await?;
                    job.completed_none()
                },
                AddLocalUser(mapping) => {
                    // make sure all project dirs are created, and get back the
                    // project home roots
                    let home_roots = create_project_dirs_and_links(&mapping.clone().into()).await?;
                    let home_permissions = cache::get_home_permissions().await?;

                    if home_roots.len() != home_permissions.len() {
                        return Err(Error::Misconfigured(
                            "Number of home roots does not match number of home permissions".to_owned(),
                        ));
                    }

                    // create the home directories, e.g. /home/project/user and /scratch/project/user
                    let mut home_dirs = Vec::new();
                    for i in 0..home_roots.len() {
                        let home_dir = format!("{}/{}", home_roots[i], mapping.local_user());
                        filesystem::create_home_dir(&home_dir, mapping.local_user(),
                                                    mapping.local_group(),
                                                    &home_permissions[i]).await?;
                        home_dirs.push(home_dir);
                    }

                    // update the job with the user's home directories
                    job.completed(home_dirs.first().cloned().unwrap_or_default())
                },
                RemoveLocalUser(mapping) => {
                    remove_user_dirs(&mapping).await?;
                    job.completed_none()
                },
                GetLocalHomeDir(mapping) => {
                    let home_roots = get_home_roots(&mapping.clone().into()).await?;

                    if home_roots.is_empty() {
                        return Err(Error::Misconfigured(
                            "No home roots configured".to_owned(),
                        ));
                    }

                    let home_dir = format!("{}/{}", home_roots[0], mapping.local_user());
                    job.completed(home_dir)
                },
                GetLocalProjectDirs(mapping) => {
                    let project_dirs = get_project_dirs_and_links(&mapping).await?;
                    job.completed(project_dirs)
                },
                SetLocalProjectQuota(mapping, volume, limit) => {
                    let quota = set_project_quota(&mapping, &volume, &limit).await?;
                    job.completed(quota)
                },
                GetLocalProjectQuota(mapping, volume) => {
                    let quota = get_project_quota(&mapping, &volume).await?;
                    job.completed(quota)
                },
                GetLocalProjectQuotas(mapping) => {
                    let quotas = get_project_quotas(&mapping).await?;
                    job.completed(quotas)
                },
                SetLocalUserQuota(mapping, volume, limit) => {
                    let quota = set_user_quota(&mapping, &volume, &limit).await?;
                    job.completed(quota)
                },
                GetLocalUserQuota(mapping, volume) => {
                    let quota = get_user_quota(&mapping, &volume).await?;
                    job.completed(quota)
                },
                GetLocalUserQuotas(mapping) => {
                    let quotas = get_user_quotas(&mapping).await?;
                    job.completed(quotas)
                },
                _ => {
                    Err(Error::InvalidInstruction(
                        format!("Invalid instruction: {}", job.instruction()),
                    ))
                }
            }
        }
    }

    run(config, filesystem_runner).await?;

    Ok(())
}

///
/// Return the root directories for all users in the passed project
///
async fn get_home_roots(mapping: &ProjectMapping) -> Result<Vec<String>, Error> {
    // The name of the project directory comes from the project part of the ProjectIdentifier
    // Eventually we would need to encode the portal into this...
    let project_dir_name = mapping.project().project();

    let home_roots = cache::get_home_roots().await?;

    let mut roots = Vec::new();
    for home_root in home_roots {
        roots.push(format!("{}/{}", home_root, project_dir_name));
    }

    Ok(roots)
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
/// returning the home root directories for the project
///
async fn create_project_dirs_and_links(mapping: &ProjectMapping) -> Result<Vec<String>, Error> {
    // The name of the project directory comes from the project part of the ProjectIdentifier
    // Eventually we would need to encode the portal into this...
    let project_dir_name = mapping.project().project();

    // The group name for any project dirs are the mapping local group IDs
    let group_name = mapping.local_group();

    // home directories are, e.g. /home/project and /scratch/project
    let home_roots_base = cache::get_home_roots().await?;
    let mut home_roots = Vec::new();
    for home_root_base in &home_roots_base {
        home_roots.push(format!("{}/{}", home_root_base, project_dir_name));
    }

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

    // create the roots in which the user's home directories will be created - these are /{home_root}/{project}
    for home_root in &home_roots {
        filesystem::create_project_dir(home_root, group_name, "0755").await?;
    }

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

    // return the home roots
    Ok(home_roots)
}

///
/// Remove (recycle) the project directories, links, and home roots for a given ProjectMapping.
/// This is non-destructive - directories are moved to .recycle subdirectories.
///
async fn remove_project_dirs_and_links(mapping: &ProjectMapping) -> Result<(), Error> {
    // The name of the project directory comes from the project part of the ProjectIdentifier
    let project_dir_name = mapping.project().project();

    // Get all the paths that need to be recycled
    let home_roots_base = cache::get_home_roots().await?;
    let project_dirs = cache::get_project_roots().await?;
    let project_links = cache::get_project_links().await?;

    // First, recycle any project links
    for project_link in project_links.iter() {
        if let Some(link) = project_link.as_ref() {
            let link_path = filesystem::get_project_link(link, &project_dir_name).await?;
            // For symlinks, we just remove them rather than recycling
            let path = std::path::Path::new(&link_path);
            if path.exists() && path.is_symlink() {
                tracing::info!("Removing symlink '{}'", link_path);
                match std::fs::remove_file(path) {
                    Ok(_) => tracing::info!("Successfully removed symlink"),
                    Err(e) => tracing::warn!("Could not remove symlink '{}': {}", link_path, e),
                }
            }
        }
    }

    // Recycle project directories
    for project_root in project_dirs {
        let project_dir = format!("{}/{}", project_root, project_dir_name);
        filesystem::recycle_dir(&project_dir).await?;
    }

    // Recycle home roots (e.g., /home/projectname, /scratch/projectname)
    for home_root_base in home_roots_base {
        let home_root = format!("{}/{}", home_root_base, project_dir_name);
        filesystem::recycle_dir(&home_root).await?;
    }

    Ok(())
}

///
/// Remove (recycle) the user's home directories in all home roots.
/// This is non-destructive - directories are moved to .recycle subdirectories.
///
async fn remove_user_dirs(mapping: &templemeads::grammar::UserMapping) -> Result<(), Error> {
    let home_roots = get_home_roots(&mapping.clone().into()).await?;
    let username = mapping.local_user();

    // Recycle the user's home directory in each home root
    for home_root in home_roots {
        let home_dir = format!("{}/{}", home_root, username);
        filesystem::recycle_dir(&home_dir).await?;
    }

    Ok(())
}
///
/// Set a storage quota for a project on a specific volume
///
pub async fn set_project_quota(
    mapping: &templemeads::grammar::ProjectMapping,
    volume: &templemeads::storage::Volume,
    limit: &templemeads::storage::QuotaLimit,
) -> Result<templemeads::storage::Quota, Error> {
    tracing::info!(
        "set_project_quota called: project={}, volume={}, limit={}",
        mapping.project(),
        volume,
        limit
    );

    // TODO: Implement actual quota setting logic
    // For now, create a quota from the limit without usage information
    let quota = match limit {
        templemeads::storage::QuotaLimit::Limited(size) => {
            templemeads::storage::Quota::limited(*size)
        }
        templemeads::storage::QuotaLimit::Unlimited => {
            templemeads::storage::Quota::unlimited()
        }
    };
    Ok(quota)
}

///
/// Get the storage quota for a project on a specific volume
///
pub async fn get_project_quota(
    mapping: &templemeads::grammar::ProjectMapping,
    volume: &templemeads::storage::Volume,
) -> Result<templemeads::storage::Quota, Error> {
    tracing::info!(
        "get_project_quota called: project={}, volume={}",
        mapping.project(),
        volume
    );

    // TODO: Implement actual quota retrieval logic
    // For now, return an error indicating no quota found
    Err(Error::NotFound(format!(
        "No quota found for project {} on volume {}",
        mapping.project(),
        volume
    )))
}

///
/// Get all storage quotas for a project across all volumes
///
pub async fn get_project_quotas(
    mapping: &templemeads::grammar::ProjectMapping,
) -> Result<
    std::collections::HashMap<
        templemeads::storage::Volume,
        templemeads::storage::Quota,
    >,
    Error,
> {
    tracing::info!("get_project_quotas called: project={}", mapping.project());

    // TODO: Implement actual quota retrieval logic
    // For now, return an empty HashMap
    Ok(std::collections::HashMap::new())
}

///
/// Set a storage quota for a user on a specific volume
///
pub async fn set_user_quota(
    mapping: &templemeads::grammar::UserMapping,
    volume: &templemeads::storage::Volume,
    limit: &templemeads::storage::QuotaLimit,
) -> Result<templemeads::storage::Quota, Error> {
    tracing::info!(
        "set_user_quota called: user={}, volume={}, limit={}",
        mapping.user(),
        volume,
        limit
    );

    // TODO: Implement actual quota setting logic
    // For now, create a quota from the limit without usage information
    let quota = match limit {
        templemeads::storage::QuotaLimit::Limited(size) => {
            templemeads::storage::Quota::limited(*size)
        }
        templemeads::storage::QuotaLimit::Unlimited => {
            templemeads::storage::Quota::unlimited()
        }
    };
    Ok(quota)
}

///
/// Get the storage quota for a user on a specific volume
///
pub async fn get_user_quota(
    mapping: &templemeads::grammar::UserMapping,
    volume: &templemeads::storage::Volume,
) -> Result<templemeads::storage::Quota, Error> {
    tracing::info!(
        "get_user_quota called: user={}, volume={}",
        mapping.user(),
        volume
    );

    // TODO: Implement actual quota retrieval logic
    // For now, return an error indicating no quota found
    Err(Error::NotFound(format!(
        "No quota found for user {} on volume {}",
        mapping.user(),
        volume
    )))
}

///
/// Get all storage quotas for a user across all volumes
///
pub async fn get_user_quotas(
    mapping: &templemeads::grammar::UserMapping,
) -> Result<
    std::collections::HashMap<
        templemeads::storage::Volume,
        templemeads::storage::Quota,
    >,
    Error,
> {
    tracing::info!("get_user_quotas called: user={}", mapping.user());

    // TODO: Implement actual quota retrieval logic
    // For now, return an empty HashMap
    Ok(std::collections::HashMap::new())
}
