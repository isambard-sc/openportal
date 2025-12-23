// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent::filesystem::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{
    AddLocalProject, AddLocalUser, GetLocalHomeDir, GetLocalProjectDirs, GetLocalProjectQuota,
    GetLocalProjectQuotas, GetLocalUserDirs, GetLocalUserQuota, GetLocalUserQuotas,
    RemoveLocalProject, RemoveLocalUser, SetLocalProjectQuota, SetLocalUserQuota,
};
use templemeads::grammar::{ProjectMapping, UserMapping};
use templemeads::job::{Envelope, Job};
use templemeads::storage::Quota;
use templemeads::Error;

mod cache;
mod filesystem;
mod quotaengine;
mod volumeconfig;

use volumeconfig::FilesystemConfig;

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
    let defaults: Defaults<FilesystemConfig> = Defaults::parse(
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

    cache::set_filesystem_config(config.agent_config.clone()).await?;

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
                    create_project_dirs_and_links(&mapping).await?;
                    job.completed_none()
                },
                RemoveLocalProject(mapping) => {
                    remove_project_dirs_and_links(&mapping).await?;
                    job.completed_none()
                },
                AddLocalUser(mapping) => {
                    create_user_dirs(&mapping).await?;
                    job.completed_none()
                },
                RemoveLocalUser(mapping) => {
                    remove_user_dirs(&mapping).await?;
                    job.completed_none()
                },
                GetLocalHomeDir(mapping) => {
                    let config = cache::get_filesystem_config().await?;
                    let home_dir = config.home_volume()?.home_path(&mapping)?;
                    job.completed(home_dir.to_string_lossy().to_string())
                },
                GetLocalUserDirs(mapping) => {
                    let config = cache::get_filesystem_config().await?;

                    let mut user_dirs = Vec::new();

                    for (volume, volume_config) in config.get_user_volumes() {
                        for path_config in volume_config.path_configs() {
                            match path_config.path(mapping.clone().into()) {
                                Ok(path) => {
                                    user_dirs.push(path.to_string_lossy().to_string());
                                }
                                Err(error) => {
                                    tracing::warn!(
                                        "Could not get user directory path for volume {}: {}",
                                        volume,
                                        error
                                    );
                                }
                            }
                        }
                    }

                    job.completed(user_dirs)
                },
                GetLocalProjectDirs(mapping) => {
                    let config = cache::get_filesystem_config().await?;

                    let mut project_dirs = Vec::new();

                    for (volume, volume_config) in config.get_project_volumes() {
                        for path_config in volume_config.path_configs() {
                            match path_config.path(mapping.clone().into()) {
                                Ok(path) => {
                                    project_dirs.push(path.to_string_lossy().to_string());
                                }
                                Err(error) => {
                                    tracing::warn!(
                                        "Could not get project directory path for volume {}: {}",
                                        volume,
                                        error
                                    );
                                }
                            }
                        }
                    }

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
/// Create the project directories and links for a given ProjectMapping,
///
async fn create_project_dirs_and_links(mapping: &ProjectMapping) -> Result<(), Error> {
    let config = cache::get_filesystem_config().await?;

    // create all of the project volume directories first
    for (volume, volume_config) in config.get_project_volumes() {
        tracing::info!("Creating project volume: {}", volume);
        for path_config in volume_config.path_configs() {
            match path_config.path(mapping.clone().into()) {
                Ok(path) => {
                    tracing::info!("    - Directory path to create: {}", path.to_string_lossy());
                    filesystem::create_dir(
                        &path,
                        "root",
                        mapping.local_group(),
                        path_config.permission(),
                    )
                    .await?;
                }
                Err(error) => {
                    tracing::warn!("Could not get path for creation: {}", error);
                }
            }
        }
    }

    // now create all of the project volume links (as the directories should exist)
    for (volume, volume_config) in config.get_project_volumes() {
        tracing::info!("Creating project volume links for: {}", volume);
        for path_config in volume_config.path_configs() {
            if let Ok(Some(link_path)) = path_config.link_path(mapping.clone().into()) {
                tracing::info!("    - Link path to create: {}", link_path.to_string_lossy());
                let dir_path = path_config.path(mapping.clone().into())?;
                filesystem::create_link(&dir_path, &link_path).await?;
            }
        }
    }

    // now create the roots of all of the user directories
    for (volume, volume_config) in config.get_user_volumes() {
        tracing::info!("Creating user volume: {}", volume);

        for path_config in volume_config.path_configs() {
            match path_config.project_path(mapping) {
                Ok(path) => {
                    tracing::info!(
                        "    - User directory root to create: {}",
                        path.to_string_lossy()
                    );
                    filesystem::create_dir(
                        &path,
                        "root",
                        mapping.local_group(),
                        path_config.permission(),
                    )
                    .await?;
                }
                Err(error) => {
                    tracing::warn!("Could not get user directory root for creation: {}", error);
                }
            }
        }
    }

    Ok(())
}

///
/// Create the user directories for a given UserMapping,
///
async fn create_user_dirs(mapping: &UserMapping) -> Result<(), Error> {
    create_project_dirs_and_links(&mapping.project()).await?;

    let config = cache::get_filesystem_config().await?;

    for (volume, volume_config) in config.get_user_volumes() {
        tracing::info!("Creating user volume: {}", volume);

        for path_config in volume_config.path_configs() {
            match path_config.path(mapping.clone().into()) {
                Ok(path) => {
                    tracing::info!(
                        "    - Home directory path to create: {}",
                        path.to_string_lossy()
                    );
                    filesystem::create_dir(
                        &path,
                        mapping.local_user(),
                        mapping.local_group(),
                        path_config.permission(),
                    )
                    .await?;
                }
                Err(error) => {
                    tracing::warn!("Could not get path for creation: {}", error);
                }
            }
        }
    }

    Ok(())
}

///
/// Remove (recycle) the project directories, links, and home roots for a given ProjectMapping.
/// This is non-destructive - directories are moved to .recycle subdirectories.
///
async fn remove_project_dirs_and_links(mapping: &ProjectMapping) -> Result<(), Error> {
    let config = cache::get_filesystem_config().await?;

    for (volume, volume_config) in config.get_project_volumes() {
        tracing::info!("Removing project volume: {}", volume);
        for path_config in volume_config.path_configs() {
            if let Ok(Some(link_path)) = path_config.link_path(mapping.clone().into()) {
                tracing::info!("    - Link path to remove: {}", link_path.to_string_lossy());
                if link_path.exists() && link_path.is_symlink() {
                    tracing::info!("      - Removing symlink '{}'", link_path.to_string_lossy());
                    match std::fs::remove_file(&link_path) {
                        Ok(_) => tracing::info!("Successfully removed symlink"),
                        Err(e) => {
                            tracing::warn!(
                                "Could not remove symlink '{}': {}",
                                link_path.to_string_lossy(),
                                e
                            )
                        }
                    }
                }
            }

            match path_config.path(mapping.clone().into()) {
                Ok(path) => {
                    tracing::info!("    - Directory path to remove: {}", path.to_string_lossy());
                    filesystem::recycle_dir(&path).await?;
                }
                Err(error) => {
                    tracing::warn!("Could not get path for removal: {}", error);
                }
            }
        }
    }

    for (volume, volume_config) in config.get_user_volumes() {
        tracing::info!("Removing user volume: {}", volume);

        for path_config in volume_config.path_configs() {
            match path_config.project_path(mapping) {
                Ok(path) => {
                    tracing::info!("    - Directory path to remove: {}", path.to_string_lossy());
                    filesystem::recycle_dir(&path).await?;
                }
                Err(error) => {
                    tracing::warn!("Could not get path for removal: {}", error);
                }
            }
        }
    }

    Ok(())
}

///
/// Remove (recycle) the user's home directories in all home roots.
/// This is non-destructive - directories are moved to .recycle subdirectories.
///
async fn remove_user_dirs(mapping: &UserMapping) -> Result<(), Error> {
    let config = cache::get_filesystem_config().await?;

    for (volume, volume_config) in config.get_user_volumes() {
        tracing::info!("Removing user volume: {}", volume);

        for path_config in volume_config.path_configs() {
            match path_config.path(mapping.clone().into()) {
                Ok(path) => {
                    tracing::info!(
                        "    - Home directory path to remove: {}",
                        path.to_string_lossy()
                    );
                    filesystem::recycle_dir(&path).await?;
                }
                Err(error) => {
                    tracing::warn!("Could not get path for removal: {}", error);
                }
            }
        }
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
    let config = cache::get_filesystem_config().await?;

    let volume_config = config.get_project_volume(volume)?;

    let engine_name = match volume_config.quota_engine_name() {
        Some(engine_name) => engine_name,
        None => {
            return Ok(Quota::unlimited());
        }
    };

    let engine = config.get_quota_engine(engine_name)?;

    engine
        .set_project_quota(mapping, &volume_config, limit)
        .await
        .map_err(|e| Error::Failed(e.to_string()))
}

///
/// Get the storage quota for a project on a specific volume
///
pub async fn get_project_quota(
    mapping: &templemeads::grammar::ProjectMapping,
    volume: &templemeads::storage::Volume,
) -> Result<templemeads::storage::Quota, Error> {
    let config = cache::get_filesystem_config().await?;

    let volume_config = config.get_project_volume(volume)?;

    let engine_name = match volume_config.quota_engine_name() {
        Some(engine_name) => engine_name,
        None => {
            return Ok(Quota::unlimited());
        }
    };

    let engine = config.get_quota_engine(engine_name)?;

    engine
        .get_project_quota(mapping, &volume_config)
        .await
        .map_err(|e| Error::Failed(e.to_string()))
}

///
/// Get all storage quotas for a project across all volumes
///
pub async fn get_project_quotas(
    mapping: &templemeads::grammar::ProjectMapping,
) -> Result<
    std::collections::HashMap<templemeads::storage::Volume, templemeads::storage::Quota>,
    Error,
> {
    let config = cache::get_filesystem_config().await?;

    let mut quotas = std::collections::HashMap::new();

    // Iterate through all configured project volumes and get quotas
    for (volume, volume_config) in config.get_project_volumes() {
        let engine_name = match volume_config.quota_engine_name() {
            Some(engine_name) => engine_name,
            None => {
                // no engine, so this is not quota-able
                continue;
            }
        };

        let engine = match config.get_quota_engine(engine_name) {
            Ok(engine) => engine,
            Err(e) => {
                tracing::warn!("Failed to get quota engine for volume {}: {}", volume, e);
                continue;
            }
        };

        match engine.get_project_quota(mapping, &volume_config).await {
            Ok(quota) => {
                quotas.insert(volume.clone(), quota);
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to get quota for project {} on volume {}: {}",
                    mapping.project(),
                    volume,
                    e
                );
                // Continue to next volume rather than failing entirely
            }
        }
    }

    Ok(quotas)
}

///
/// Set a storage quota for a user on a specific volume
///
pub async fn set_user_quota(
    mapping: &templemeads::grammar::UserMapping,
    volume: &templemeads::storage::Volume,
    limit: &templemeads::storage::QuotaLimit,
) -> Result<templemeads::storage::Quota, Error> {
    let config = cache::get_filesystem_config().await?;

    let volume_config = config.get_user_volume(volume)?;

    let engine_name = match volume_config.quota_engine_name() {
        Some(engine_name) => engine_name,
        None => {
            return Ok(Quota::unlimited());
        }
    };

    let engine = config.get_quota_engine(engine_name)?;

    engine
        .set_user_quota(mapping, &volume_config, limit)
        .await
        .map_err(|e| Error::Failed(e.to_string()))
}

///
/// Get the storage quota for a user on a specific volume
///
pub async fn get_user_quota(
    mapping: &templemeads::grammar::UserMapping,
    volume: &templemeads::storage::Volume,
) -> Result<templemeads::storage::Quota, Error> {
    let config = cache::get_filesystem_config().await?;

    let volume_config = config.get_user_volume(volume)?;

    let engine_name = match volume_config.quota_engine_name() {
        Some(engine_name) => engine_name,
        None => {
            return Ok(Quota::unlimited());
        }
    };

    let engine = config.get_quota_engine(engine_name)?;

    engine
        .get_user_quota(mapping, &volume_config)
        .await
        .map_err(|e| Error::Failed(e.to_string()))
}

///
/// Get all storage quotas for a user across all volumes
///
pub async fn get_user_quotas(
    mapping: &templemeads::grammar::UserMapping,
) -> Result<
    std::collections::HashMap<templemeads::storage::Volume, templemeads::storage::Quota>,
    Error,
> {
    let config = cache::get_filesystem_config().await?;

    let mut quotas = std::collections::HashMap::new();

    // Iterate through all configured user volumes and get quotas
    for (volume, user_config) in config.get_user_volumes() {
        let engine_name = match user_config.quota_engine_name() {
            Some(engine_name) => engine_name,
            None => {
                // no engine, so this is not quota-able
                continue;
            }
        };

        let engine = match config.get_quota_engine(engine_name) {
            Ok(engine) => engine,
            Err(e) => {
                tracing::warn!("Failed to get quota engine for volume {}: {}", volume, e);
                continue;
            }
        };

        match engine.get_user_quota(mapping, &user_config).await {
            Ok(quota) => {
                quotas.insert(volume.clone(), quota);
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to get quota for user {} on volume {}: {}",
                    mapping.local_user(),
                    volume,
                    e
                );
                // Continue to next volume rather than failing entirely
            }
        }
    }

    Ok(quotas)
}
