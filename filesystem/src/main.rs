// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use chrono::Utc;

use greatwestern::grammar::Instruction::{
    AddLocalProject, AddLocalUser, ClearLocalProjectQuota, ClearLocalUserQuota, GetLocalHomeDir,
    GetLocalProjectDirs, GetLocalProjectQuota, GetLocalProjectQuotas, GetLocalStorageReport,
    GetLocalUserDirs, GetLocalUserQuota, GetLocalUserQuotas, IsLocalProjectAdded,
    IsLocalProjectRemoved, IsLocalUserAdded, IsLocalUserRemoved, RemoveLocalProject,
    RemoveLocalUser, SetLocalProjectQuota, SetLocalUserQuota,
};
use greatwestern::grammar::{Date, ProjectMapping, UserMapping};
use greatwestern::storage::{Quota, Volume};
use greatwestern::storagereport::ProjectStorageReport;
use greatwestern::Hpc;
use std::collections::HashSet;
use std::path::PathBuf;
use templemeads::agent;
use templemeads::agent::filesystem::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::notification::default_notify_runner;
use templemeads::set_notify_runner;
use templemeads::Error;

type Envelope = templemeads::job::Envelope<Hpc>;
type Job = templemeads::job::Job<Hpc>;

mod cache;
mod fakequotaengine;
mod filesystem;
mod linuxquotaengine;
mod lustreengine;
mod nameservice;
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
    templemeads::spawn_system_monitor::<Hpc>();

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

    // Optional exec prefix for redirecting filesystem operations into a
    // container or remote host.  When set, every mkdir/chown/chmod/mv/ln-s
    // call is prefixed with these tokens instead of using the Rust stdlib.
    // Example: exec-prefix = "docker exec slurmctld"
    // Leave unset (or empty) to use native Rust calls (production default).
    let exec_prefix_str = config.option("exec-prefix", "");
    let exec_prefix = if exec_prefix_str.is_empty() {
        None
    } else {
        Some(
            exec_prefix_str
                .split_whitespace()
                .map(|s| s.to_owned())
                .collect::<Vec<String>>(),
        )
    };
    filesystem::set_exec_prefix(exec_prefix)?;

    async_runnable! {
        ///
        /// Runnable function that will be called when a job is received
        /// by the agent
        ///
        pub async fn filesystem_runner(envelope: Envelope) -> Result<Job, templemeads::Error>
        {
            let me = envelope.recipient();
            let sender = envelope.sender();
            let job = envelope.job();

            match job.instruction() {
                GetLocalStorageReport(mapping, dates) => {
                    let today = Date::today().day();
                    if dates != today {
                        return job.errored(&format!(
                            "Storage reports only support today's date; requested range: {}",
                            dates
                        ));
                    }
                    let report = get_local_storage_report(
                        me.name(), &sender, &mapping, job.expires()
                    ).await?;
                    job.completed(report)
                },
                AddLocalProject(mapping) => {
                    create_project_dirs_and_links(&mapping, job.expires()).await?;
                    job.completed_none()
                },
                RemoveLocalProject(mapping) => {
                    remove_project_dirs_and_links(&mapping).await?;
                    job.completed_none()
                },
                AddLocalUser(mapping) => {
                    create_user_dirs(&mapping, job.expires()).await?;
                    job.completed_none()
                },
                RemoveLocalUser(mapping) => {
                    remove_user_dirs(&mapping).await?;
                    job.completed_none()
                },
                IsLocalUserAdded(mapping) => {
                    job.completed(are_user_dirs_added(&mapping).await?)
                },
                IsLocalUserRemoved(mapping) => {
                    job.completed(are_user_dirs_removed(&mapping).await?)
                },
                IsLocalProjectAdded(mapping) => {
                    job.completed(are_project_dirs_added(&mapping).await?)
                },
                IsLocalProjectRemoved(mapping) => {
                    job.completed(are_project_dirs_removed(&mapping).await?)
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
                    let quota = set_project_quota(&mapping, &volume, &limit, job.expires()).await?;
                    job.completed(quota)
                },
                GetLocalProjectQuota(mapping, volume) => {
                    let quota = get_project_quota(&mapping, &volume, job.expires()).await?;
                    job.completed(quota)
                },
                GetLocalProjectQuotas(mapping) => {
                    let quotas = get_project_quotas(&mapping, job.expires()).await?;
                    job.completed(quotas)
                },
                SetLocalUserQuota(mapping, volume, limit) => {
                    let quota = set_user_quota(&mapping, &volume, &limit, job.expires()).await?;
                    job.completed(quota)
                },
                GetLocalUserQuota(mapping, volume) => {
                    let quota = get_user_quota(&mapping, &volume, job.expires()).await?;
                    job.completed(quota)
                },
                GetLocalUserQuotas(mapping) => {
                    let quotas = get_user_quotas(&mapping, job.expires()).await?;
                    job.completed(quotas)
                },
                ClearLocalProjectQuota(mapping, volume) => {
                    clear_project_quota(&mapping, &volume, job.expires()).await?;
                    job.completed_none()
                },
                ClearLocalUserQuota(mapping, volume) => {
                    clear_user_quota(&mapping, &volume, job.expires()).await?;
                    job.completed_none()
                },
                _ => {
                    Err(Error::InvalidInstruction(
                        format!("Invalid instruction: {}", job.instruction()),
                    ))
                }
            }
        }
    }

    set_notify_runner::<Hpc>(default_notify_runner).await?;
    run(config, filesystem_runner).await?;

    Ok(())
}

///
/// The paths `add_local_project` creates for `mapping`, and that
/// `remove_local_project` recycles again: every project-volume directory, the
/// link beside any of them that is configured to have one, and the per-project
/// root of every user volume.
///
/// Derived by walking exactly the same configuration in the same order as
/// `create_project_dirs_and_links`, so that "has this been added?" cannot drift
/// away from what adding it actually does. A path the configuration cannot
/// produce is skipped here for the same reason it is skipped there - it was
/// never created, so it is not evidence either way.
///
async fn project_paths(mapping: &ProjectMapping) -> Result<Vec<PathBuf>, Error> {
    let config = cache::get_filesystem_config().await?;

    let mut paths = Vec::new();

    for (volume, volume_config) in config.get_project_volumes() {
        for path_config in volume_config.path_configs() {
            match path_config.path(mapping.clone().into()) {
                Ok(path) => {
                    if let Ok(Some(link_path)) = path_config.link_path(mapping.clone().into()) {
                        paths.push(link_path);
                    }
                    paths.push(path);
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

    for (volume, volume_config) in config.get_user_volumes() {
        for path_config in volume_config.path_configs() {
            match path_config.project_path(mapping) {
                Ok(path) => paths.push(path),
                Err(error) => {
                    tracing::warn!(
                        "Could not get user directory root for volume {}: {}",
                        volume,
                        error
                    );
                }
            }
        }
    }

    Ok(paths)
}

///
/// The paths `add_local_user` creates for `mapping`, and that
/// `remove_local_user` recycles again - the user's own directory on every user
/// volume. Note that this deliberately does not include the project
/// directories `create_user_dirs` also ensures exist: those belong to the
/// project, not to this user, and `remove_user_dirs` leaves them alone.
///
async fn user_paths(mapping: &UserMapping) -> Result<Vec<PathBuf>, Error> {
    let config = cache::get_filesystem_config().await?;

    let mut paths = Vec::new();

    for (volume, volume_config) in config.get_user_volumes() {
        for path_config in volume_config.path_configs() {
            match path_config.path(mapping.clone().into()) {
                Ok(path) => paths.push(path),
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

    Ok(paths)
}

///
/// Return whether every path in `paths` exists (`want_present`) or whether
/// none of them do (`!want_present`). The first path that disagrees is logged
/// and decides the answer - it is the one a caller would have to re-run the
/// add or remove to fix.
///
async fn all_paths_match(paths: &[PathBuf], want_present: bool) -> Result<bool, Error> {
    let config = cache::get_filesystem_config().await?;
    let roots = config.all_roots();

    for path in paths {
        if filesystem::path_exists(path, &roots).await? != want_present {
            tracing::info!(
                "Path '{}' is {} - expected it to be {}",
                path.to_string_lossy(),
                if want_present {
                    "missing"
                } else {
                    "still present"
                },
                if want_present { "present" } else { "gone" }
            );
            return Ok(false);
        }
    }

    Ok(true)
}

///
/// Return true only if everything `add_local_project` creates for this project
/// is present. Read live from the filesystem - nothing here is cached.
///
async fn are_project_dirs_added(mapping: &ProjectMapping) -> Result<bool, Error> {
    all_paths_match(&project_paths(mapping).await?, true).await
}

///
/// Return true only if everything `remove_local_project` recycles for this
/// project is gone.
///
async fn are_project_dirs_removed(mapping: &ProjectMapping) -> Result<bool, Error> {
    all_paths_match(&project_paths(mapping).await?, false).await
}

///
/// Return true only if everything `add_local_user` creates for this user is
/// present - both their own directories and the project directories that
/// `create_user_dirs` makes sure exist before it creates them.
///
async fn are_user_dirs_added(mapping: &UserMapping) -> Result<bool, Error> {
    if !are_project_dirs_added(&mapping.project()).await? {
        return Ok(false);
    }

    all_paths_match(&user_paths(mapping).await?, true).await
}

///
/// Return true only if everything `remove_local_user` recycles for this user is
/// gone. The project directories are not consulted: `remove_user_dirs` does not
/// touch them, and they outlive any one member of the project.
///
async fn are_user_dirs_removed(mapping: &UserMapping) -> Result<bool, Error> {
    all_paths_match(&user_paths(mapping).await?, false).await
}

///
/// Create the project directories and links for a given ProjectMapping,
///
async fn create_project_dirs_and_links(
    mapping: &ProjectMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    let config = cache::get_filesystem_config().await?;

    // The volumes on which a directory was actually created by this call. Only those
    // are candidates for the default quota - see `set_default_project_quotas`.
    let mut created_volumes: HashSet<Volume> = HashSet::new();

    // create all of the project volume directories first
    for (volume, volume_config) in config.get_project_volumes() {
        tracing::info!("Creating project volume: {}", volume);
        for path_config in volume_config.path_configs() {
            match path_config.path(mapping.clone().into()) {
                Ok(path) => {
                    tracing::info!("    - Directory path to create: {}", path.to_string_lossy());
                    if filesystem::create_dir(
                        &path,
                        &config.all_roots(),
                        "root",
                        mapping.local_group(),
                        path_config.permission(),
                    )
                    .await?
                    {
                        created_volumes.insert(volume.clone());
                    }
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
                filesystem::create_link(&dir_path, &link_path, &config.all_roots()).await?;
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
                        &config.all_roots(),
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

    // finally, set the default quota on the volumes whose directories this call
    // actually created, and only where no quota is set already
    set_default_project_quotas(mapping, &created_volumes, expires).await?;

    Ok(())
}

///
/// Apply each project volume's configured default quota, for the volumes named in
/// `created_volumes`.
///
/// The default is a **starting point for a new directory, not a policy that is
/// re-imposed**. `add_local_project` and `add_local_user` are both re-sent for
/// projects that already exist - the cluster agent re-runs them on a retry, and
/// `create_user_dirs` calls `create_project_dirs_and_links` every time any member is
/// added - so applying the default on every call silently undid quotas an operator had
/// raised with `set_local_project_quota`. Two conditions therefore gate it:
///
///  1. this call created the directory on that volume (`created_volumes`), and
///  2. the project has no quota on that volume yet.
///
/// A quota that cannot be read is **not** taken to be absent: an `lfs quota` that fails
/// or times out leaves the existing limit unknown, and overwriting it on that basis is
/// exactly the data loss this guards against. Such a volume is skipped and logged, as
/// is any failure to set the quota itself - as before, neither fails the job.
///
async fn set_default_project_quotas(
    mapping: &ProjectMapping,
    created_volumes: &HashSet<Volume>,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    let config = cache::get_filesystem_config().await?;

    for (volume, volume_config) in config.get_project_volumes() {
        if !volume_config.has_quota_engine() {
            continue;
        }

        let Some(default_quota) = volume_config.default_quota() else {
            continue;
        };

        if !created_volumes.contains(&volume) {
            tracing::info!(
                "Not setting the default quota for project {} on volume {} - the directories \
                 were already there, so this is not a new project directory.",
                mapping.project(),
                volume
            );
            continue;
        }

        match get_project_quota(mapping, &volume, expires).await {
            Ok(quota) => {
                if !quota.is_unlimited() {
                    tracing::info!(
                        "Not setting the default quota for project {} on volume {} - a quota \
                         of {} is already set.",
                        mapping.project(),
                        volume,
                        quota.limit()
                    );
                    continue;
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Not setting the default quota for project {} on volume {} - could not \
                     read the existing quota: {}",
                    mapping.project(),
                    volume,
                    e
                );
                continue;
            }
        }

        tracing::info!(
            "Setting default quota for project {} on volume {}: {}",
            mapping.project(),
            volume,
            default_quota
        );

        match set_project_quota(mapping, &volume, default_quota, expires).await {
            Ok(_) => {
                tracing::info!(
                    "Successfully set default quota for project {} on volume {}",
                    mapping.project(),
                    volume
                );
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to set default quota for project {} on volume {}: {}",
                    mapping.project(),
                    volume,
                    e
                );
            }
        }
    }

    Ok(())
}

///
/// Create the user directories for a given UserMapping,
///
async fn create_user_dirs(
    mapping: &UserMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    create_project_dirs_and_links(&mapping.project(), expires).await?;

    let config = cache::get_filesystem_config().await?;

    // The volumes on which a directory was actually created by this call. Only those
    // are candidates for the default quota - see `set_default_user_quotas`.
    let mut created_volumes: HashSet<Volume> = HashSet::new();

    for (volume, volume_config) in config.get_user_volumes() {
        tracing::info!("Creating user volume: {}", volume);

        for path_config in volume_config.path_configs() {
            match path_config.path(mapping.clone().into()) {
                Ok(path) => {
                    tracing::info!("    - User directory to create: {}", path.to_string_lossy());
                    if filesystem::create_dir(
                        &path,
                        &config.all_roots(),
                        mapping.local_user().unix()?,
                        mapping.local_group(),
                        path_config.permission(),
                    )
                    .await?
                    {
                        created_volumes.insert(volume.clone());
                    }
                }
                Err(error) => {
                    tracing::warn!("Could not get path for creation: {}", error);
                }
            }
        }
    }

    // now we have created all of the directories, set the default quota on the volumes
    // whose directories this call actually created, and only where none is set already
    set_default_user_quotas(mapping, &created_volumes, expires).await?;

    Ok(())
}

///
/// Apply each user volume's configured default quota, for the volumes named in
/// `created_volumes`.
///
/// The same two conditions as `set_default_project_quotas` gate this, and for the same
/// reason - `add_local_user` is re-sent for users who already exist, and the default is
/// a starting point for a new directory rather than a policy to re-impose. See there
/// for the full rationale, including why an unreadable quota is not treated as an
/// absent one.
///
async fn set_default_user_quotas(
    mapping: &UserMapping,
    created_volumes: &HashSet<Volume>,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    let config = cache::get_filesystem_config().await?;

    for (volume, volume_config) in config.get_user_volumes() {
        if !volume_config.has_quota_engine() {
            continue;
        }

        let Some(default_quota) = volume_config.default_quota() else {
            continue;
        };

        if !created_volumes.contains(&volume) {
            tracing::info!(
                "Not setting the default quota for user {} on volume {} - the directories \
                 were already there, so this is not a new user directory.",
                mapping.local_user(),
                volume
            );
            continue;
        }

        match get_user_quota(mapping, &volume, expires).await {
            Ok(quota) => {
                if !quota.is_unlimited() {
                    tracing::info!(
                        "Not setting the default quota for user {} on volume {} - a quota of \
                         {} is already set.",
                        mapping.local_user(),
                        volume,
                        quota.limit()
                    );
                    continue;
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Not setting the default quota for user {} on volume {} - could not read \
                     the existing quota: {}",
                    mapping.local_user(),
                    volume,
                    e
                );
                continue;
            }
        }

        tracing::info!(
            "Setting default quota for user {} on volume {}: {}",
            mapping.local_user(),
            volume,
            default_quota
        );

        match set_user_quota(mapping, &volume, default_quota, expires).await {
            Ok(_) => {
                tracing::info!(
                    "Successfully set default quota for user {} on volume {}",
                    mapping.local_user(),
                    volume
                );
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to set default quota for user {} on volume {}: {}",
                    mapping.local_user(),
                    volume,
                    e
                );
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
                filesystem::remove_link(&link_path, &config.all_roots()).await?;
            }

            match path_config.path(mapping.clone().into()) {
                Ok(path) => {
                    tracing::info!("    - Directory path to remove: {}", path.to_string_lossy());
                    filesystem::recycle_dir(&path, &config.all_roots()).await?;
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
                    filesystem::recycle_dir(&path, &config.all_roots()).await?;
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
                    filesystem::recycle_dir(&path, &config.all_roots()).await?;
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
/// Clear the storage quota for a project on a specific volume
///
pub async fn clear_project_quota(
    mapping: &greatwestern::grammar::ProjectMapping,
    volume: &greatwestern::storage::Volume,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    let config = cache::get_filesystem_config().await?;

    let volume_config = config.get_project_volume(volume)?;

    if !volume_config.has_quota_engine() {
        return Ok(());
    }

    let engine_name = match volume_config.quota_engine_name() {
        Some(engine_name) => engine_name,
        None => {
            return Ok(());
        }
    };

    let engine = config.get_quota_engine(engine_name)?;

    engine
        .clear_project_quota(mapping, volume, &volume_config, expires)
        .await
        .map_err(|e| Error::Failed(e.to_string()))
}

///
/// Set a storage quota for a project on a specific volume
///
pub async fn set_project_quota(
    mapping: &greatwestern::grammar::ProjectMapping,
    volume: &greatwestern::storage::Volume,
    limit: &greatwestern::storage::QuotaLimit,
    expires: &chrono::DateTime<Utc>,
) -> Result<greatwestern::storage::Quota, Error> {
    let config = cache::get_filesystem_config().await?;

    let volume_config = config.get_project_volume(volume)?;

    if !volume_config.has_quota_engine() {
        return Ok(Quota::unlimited());
    }

    let engine_name = match volume_config.quota_engine_name() {
        Some(engine_name) => engine_name,
        None => {
            return Ok(Quota::unlimited());
        }
    };

    let engine = config.get_quota_engine(engine_name)?;

    engine
        .set_project_quota(mapping, volume, &volume_config, limit, expires)
        .await
        .map_err(|e| Error::Failed(e.to_string()))
}

///
/// Get the storage quota for a project on a specific volume
///
pub async fn get_project_quota(
    mapping: &greatwestern::grammar::ProjectMapping,
    volume: &greatwestern::storage::Volume,
    expires: &chrono::DateTime<Utc>,
) -> Result<greatwestern::storage::Quota, Error> {
    let config = cache::get_filesystem_config().await?;

    let volume_config = config.get_project_volume(volume)?;

    if !volume_config.has_quota_engine() {
        return Ok(Quota::unlimited());
    }

    let engine_name = match volume_config.quota_engine_name() {
        Some(engine_name) => engine_name,
        None => {
            return Ok(Quota::unlimited());
        }
    };

    let engine = config.get_quota_engine(engine_name)?;

    engine
        .get_project_quota(mapping, volume, &volume_config, expires)
        .await
        .map_err(|e| Error::Failed(e.to_string()))
}

///
/// Get all storage quotas for a project across all volumes
///
pub async fn get_project_quotas(
    mapping: &greatwestern::grammar::ProjectMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<
    std::collections::HashMap<greatwestern::storage::Volume, greatwestern::storage::Quota>,
    Error,
> {
    let config = cache::get_filesystem_config().await?;

    let mut quotas = std::collections::HashMap::new();

    // Iterate through all configured project volumes and get quotas
    for (volume, volume_config) in config.get_project_volumes() {
        if !volume_config.has_quota_engine() {
            continue;
        }

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

        match engine
            .get_project_quota(mapping, &volume, &volume_config, expires)
            .await
        {
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
/// Clear a user quota for a user on a specific volume
///
pub async fn clear_user_quota(
    mapping: &greatwestern::grammar::UserMapping,
    volume: &greatwestern::storage::Volume,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    let config = cache::get_filesystem_config().await?;

    let volume_config = config.get_user_volume(volume)?;

    if !volume_config.has_quota_engine() {
        return Ok(());
    }

    let engine_name = match volume_config.quota_engine_name() {
        Some(engine_name) => engine_name,
        None => {
            return Ok(());
        }
    };

    let engine = config.get_quota_engine(engine_name)?;

    engine
        .clear_user_quota(mapping, volume, &volume_config, expires)
        .await
        .map_err(|e| Error::Failed(e.to_string()))
}

///
/// Set a storage quota for a user on a specific volume
///
pub async fn set_user_quota(
    mapping: &greatwestern::grammar::UserMapping,
    volume: &greatwestern::storage::Volume,
    limit: &greatwestern::storage::QuotaLimit,
    expires: &chrono::DateTime<Utc>,
) -> Result<greatwestern::storage::Quota, Error> {
    let config = cache::get_filesystem_config().await?;

    let volume_config = config.get_user_volume(volume)?;

    if !volume_config.has_quota_engine() {
        return Ok(Quota::unlimited());
    }

    let engine_name = match volume_config.quota_engine_name() {
        Some(engine_name) => engine_name,
        None => {
            return Ok(Quota::unlimited());
        }
    };

    let engine = config.get_quota_engine(engine_name)?;

    engine
        .set_user_quota(mapping, volume, &volume_config, limit, expires)
        .await
        .map_err(|e| Error::Failed(e.to_string()))
}

///
/// Get the storage quota for a user on a specific volume
///
pub async fn get_user_quota(
    mapping: &greatwestern::grammar::UserMapping,
    volume: &greatwestern::storage::Volume,
    expires: &chrono::DateTime<Utc>,
) -> Result<greatwestern::storage::Quota, Error> {
    let config = cache::get_filesystem_config().await?;

    let volume_config = config.get_user_volume(volume)?;

    if !volume_config.has_quota_engine() {
        return Ok(Quota::unlimited());
    }

    let engine_name = match volume_config.quota_engine_name() {
        Some(engine_name) => engine_name,
        None => {
            return Ok(Quota::unlimited());
        }
    };

    let engine = config.get_quota_engine(engine_name)?;

    engine
        .get_user_quota(mapping, volume, &volume_config, expires)
        .await
        .map_err(|e| Error::Failed(e.to_string()))
}

///
/// Get all storage quotas for a user across all volumes
///
pub async fn get_user_quotas(
    mapping: &greatwestern::grammar::UserMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<
    std::collections::HashMap<greatwestern::storage::Volume, greatwestern::storage::Quota>,
    Error,
> {
    let config = cache::get_filesystem_config().await?;

    let mut quotas = std::collections::HashMap::new();

    // Iterate through all configured user volumes and get quotas
    for (volume, user_config) in config.get_user_volumes() {
        if !user_config.has_quota_engine() {
            continue;
        }

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

        match engine
            .get_user_quota(mapping, &volume, &user_config, expires)
            .await
        {
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

///
/// Build a ProjectStorageReport for the given project mapping.
///
/// The sender (cluster agent) is called back with get_users to retrieve
/// the list of users in the project. All quota queries are then handled
/// locally by this filesystem agent.
///
pub async fn get_local_storage_report(
    me: &str,
    sender: &agent::Peer,
    mapping: &ProjectMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<ProjectStorageReport, Error> {
    let project = mapping.project();
    let mut report = ProjectStorageReport::new(project);

    // Fetch project-level quotas locally
    match get_project_quotas(mapping, expires).await {
        Ok(quotas) => {
            report.set_project_quotas(quotas);
        }
        Err(e) => {
            tracing::warn!("Failed to get project quotas for {}: {}", mapping, e);
        }
    }

    // Call back to the sender (cluster agent) to get the users for this project
    let user_mappings: Vec<UserMapping> = {
        let job = Job::parse(
            &format!("{}.{} get_users {}", me, sender.name(), project),
            false,
        )?
        .put(sender)
        .await?;

        match job.wait().await?.result::<Vec<UserMapping>>()? {
            Some(users) => users,
            None => {
                tracing::warn!("No users returned for project {}", project);
                vec![]
            }
        }
    };

    // Fetch per-user quotas locally for each user in the project
    for user_mapping in &user_mappings {
        match get_user_quotas(user_mapping, expires).await {
            Ok(quotas) => {
                report.add_user_quotas(user_mapping.user(), quotas);
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to get user quotas for {}: {}",
                    user_mapping.local_user(),
                    e
                );
            }
        }
    }

    // Record portal-user → local-username mappings
    report.add_mappings(&user_mappings)?;

    Ok(report)
}
