// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use std::collections::HashMap;

use greatwestern::grammar::Instruction::{
    AddProject, AddUser, BlockProject, BlockUser, ClearProjectQuota, ClearUserQuota, GetHomeDir,
    GetLimit, GetLocalHomeDir, GetLocalProjectDirs, GetLocalUserDirs, GetProjectDirs,
    GetProjectMapping, GetProjectQuota, GetProjectQuotas, GetProjects, GetStorageReport,
    GetStorageReports, GetUsageReport, GetUsageReports, GetUserDirs, GetUserMapping, GetUserQuota,
    GetUserQuotas, GetUsers, IsBlockedProject, IsBlockedUser, IsProjectAdded, IsProjectRemoved,
    IsProtectedUser, IsUserAdded, IsUserRemoved, RemoveProject, RemoveUser, SetLimit,
    SetProjectQuota, SetUserQuota, UnblockProject, UnblockUser,
};
use greatwestern::grammar::{
    DateRange, ProjectIdentifier, ProjectMapping, UserIdentifier, UserMapping,
};
use greatwestern::storage::{Quota, Volume};
use greatwestern::storagereport::{ProjectStorageReport, StorageReport};
use greatwestern::usagereport::{ProjectUsageReport, Usage, UsageReport};
use greatwestern::{Hpc, NotificationEvent};
use templemeads::agent;
use templemeads::agent::instance::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::notification::{self, default_notify_runner};
use templemeads::portal_identifier::PortalIdentifier;
use templemeads::set_notify_runner;
use templemeads::Error;

type Envelope = templemeads::job::Envelope<Hpc>;
type Job = templemeads::job::Job<Hpc>;

const AGENT_WAIT_TIME: u64 = 10;

///
/// Main function for the cluster instance agent
///
/// This purpose of this agent is to manage an individual instance
/// of a batch cluster. It will manage the lifecycle of
/// users and projects on the cluster.
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    templemeads::config::initialise_tracing();

    // start system monitoring
    templemeads::spawn_system_monitor::<Hpc>();

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("cluster".to_owned()),
        Some(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("cluster-config.toml"),
        ),
        Some("ws://localhost:8046".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8046),
        None,
        None,
        Some(AgentType::Instance),
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
        pub async fn cluster_runner(envelope: Envelope) -> Result<Job, Error>
        {
            let me = envelope.recipient();
            let job = envelope.job();

            match job.instruction() {
                GetProjects(portal) => {
                    // get the list of projects from the cluster
                    tracing::debug!("Getting list of projects for portal {}", portal);

                    let projects = get_projects(me.name(), &portal).await?;

                    job.completed(projects)
                },
                GetUsers(project) => {
                    // get the list of users from the cluster
                    tracing::debug!("Getting list of users in project {}", project);

                    let users = get_accounts(me.name(), &project).await?;

                    job.completed(users)
                },
                AddProject(project) => {
                    assert_agents_connected().await?;

                    match agent::scheduler(AGENT_WAIT_TIME).await {
                        Some(_) => {}
                        None => {
                            tracing::error!("No scheduler agent found");
                            return Err(Error::MissingAgent(
                                "Cannot run the job because there is no scheduler agent".to_string(),
                            ));
                        }
                    }

                    // see if the project already exists
                    let project_exists: bool = is_existing_project(me.name(), &project).await?;

                    // add the project to the cluster
                    let mapping = match add_project_to_cluster(me.name(), &project).await {
                        Ok(mapping) => mapping,
                        Err(e) => {
                            // we cannot leave a dangling project group,
                            // so we need to remove the project from FreeIPA
                            tracing::error!("Error adding project {} to cluster: {:?}", project, e);

                            // only remove the project if it didn't already exist
                            // (this stops us removing an existing group that failed
                            //  an update)
                            if !project_exists {
                                match remove_project_from_cluster(me.name(), &project).await {
                                    Ok(_) => tracing::info!("Removed partially added project {}", project),
                                    Err(e) => tracing::error!("Failed to remove partially added project {}: {:?}", project, e)
                                }
                            }

                            return Err(e);
                        }
                    };

                    notification::send::<Hpc>(&envelope.job().destination().reverse(), NotificationEvent::ProjectAdded(project.clone())).await;
                    job.completed(mapping)
                },
                RemoveProject(project) => {
                    assert_agents_connected().await?;

                    // remove the project from the cluster
                    let mapping = remove_project_from_cluster(me.name(), &project).await?;
                    notification::send::<Hpc>(&envelope.job().destination().reverse(), NotificationEvent::ProjectRemoved(project.clone())).await;
                    job.completed(mapping)
                },
                AddUser(user) => {
                    match assert_agents_connected().await {
                        Ok(_) => {}
                        Err(e) => {
                            // not a problem if the user already exists and is protected
                            match is_protected_user(me.name(), &user).await? {
                                true => {
                                    return job.completed(get_user_mapping(me.name(), &user).await?);
                                }
                                false => {
                                    return Err(e);
                                }
                            }
                        }
                    }

                    // blocked users must not be re-enabled by add_user;
                    // only unblock_user should do that
                    if is_blocked_user(me.name(), &user).await? {
                        tracing::info!(
                            "User {} is blocked - not re-adding. Use unblock_user to unblock.",
                            user
                        );
                        return job.completed(get_user_mapping(me.name(), &user).await?);
                    }

                    // does the user already exist?
                    let user_exists: bool = is_existing_user(me.name(), &user).await?;

                    if user_exists {
                        tracing::info!("User {} already exists on cluster - re-adding them", user);
                    }

                    // add the user to the cluster
                    let mut attempts = 0;

                    let mapping = loop {
                        match add_user_to_cluster(me.name(), &user).await {
                            Ok(mapping) => break mapping,
                            Err(e) => {
                                attempts += 1;

                                if attempts > 5 {
                                    // we cannot leave a dangling user account,
                                    // so we need to remove the user from FreeIPA
                                    tracing::error!("Error adding user {} to cluster: {:?}", user, e);

                                    // only remove the user if they didn't already exist
                                    // (this stops us removing an existing account that failed
                                    //  an update)
                                    if !user_exists {
                                        tracing::warn!("Removing partially added user {}...", user);
                                        match remove_account(me.name(), &user).await {
                                            Ok(_) => tracing::info!("Removed partially added user {}", user),
                                            Err(e) => tracing::error!("Failed to remove partially added user {}: {:?}", user, e)
                                        }
                                    }

                                    return Err(e);
                                }
                                else {
                                    tracing::warn!("Error adding user {} to cluster: {:?}. Trying again...", user, e);
                                }
                            }
                        }
                    };

                    notification::send::<Hpc>(&envelope.job().destination().reverse(), NotificationEvent::UserAdded(user.clone())).await;
                    job.completed(mapping)
                }
                RemoveUser(user) => {
                    match assert_agents_connected().await {
                        Ok(_) => {}
                        Err(e) => {
                            // not a problem if the user already exists and is protected
                            match is_protected_user(me.name(), &user).await? {
                                true => {
                                    return job.completed(get_user_mapping(me.name(), &user).await?);
                                }
                                false => {
                                    return Err(e);
                                }
                            }
                        }
                    }

                    // remove the user from the cluster
                    let mapping = remove_user_from_cluster(me.name(), &user).await?;
                    notification::send::<Hpc>(&envelope.job().destination().reverse(), NotificationEvent::UserRemoved(user.clone())).await;
                    job.completed(mapping)
                }
                BlockUser(user) => {
                    let mapping = block_user_on_cluster(me.name(), &user).await?;
                    notification::send::<Hpc>(&envelope.job().destination().reverse(), NotificationEvent::UserBlocked(user.clone())).await;
                    job.completed(mapping)
                }
                UnblockUser(user) => {
                    let mapping = unblock_user_on_cluster(me.name(), &user).await?;
                    notification::send::<Hpc>(&envelope.job().destination().reverse(), NotificationEvent::UserUnblocked(user.clone())).await;
                    job.completed(mapping)
                }
                IsBlockedUser(user) => {
                    let is_blocked = is_blocked_user(me.name(), &user).await?;
                    job.completed(is_blocked)
                }
                BlockProject(project) => {
                    let mappings = block_project_on_cluster(me.name(), &project).await?;
                    notification::send::<Hpc>(&envelope.job().destination().reverse(), NotificationEvent::ProjectBlocked(project.clone())).await;
                    job.completed(mappings)
                }
                UnblockProject(project) => {
                    let mappings = unblock_project_on_cluster(me.name(), &project).await?;
                    notification::send::<Hpc>(&envelope.job().destination().reverse(), NotificationEvent::ProjectUnblocked(project.clone())).await;
                    job.completed(mappings)
                }
                IsBlockedProject(project) => {
                    let is_blocked = is_blocked_project_on_cluster(me.name(), &project).await?;
                    job.completed(is_blocked)
                }
                IsProtectedUser(user) => {
                    let is_protected = is_protected_user(me.name(), &user).await?;
                    job.completed(is_protected)
                }
                IsUserAdded(user) => {
                    job.completed(is_user_added(me.name(), &user).await?)
                }
                IsUserRemoved(user) => {
                    job.completed(is_user_removed(me.name(), &user).await?)
                }
                IsProjectAdded(project) => {
                    job.completed(is_project_added(me.name(), &project).await?)
                }
                IsProjectRemoved(project) => {
                    job.completed(is_project_removed(me.name(), &project).await?)
                }
                GetProjectMapping(project) => {
                    let mapping = get_project_mapping(me.name(), &project).await?;
                    job.completed(mapping)
                }
                GetUserMapping(user) => {
                    let mapping = get_user_mapping(me.name(), &user).await?;
                    job.completed(mapping)
                }
                GetUsageReport(project, dates) => {
                    let mapping = get_project_mapping(me.name(), &project).await?;
                    let report = get_usage_report(me.name(), &mapping, &dates).await?;
                    job.completed(report)
                }
                GetUsageReports(portal, dates) => {
                    let report = get_usage_reports(me.name(), &portal, &dates).await?;
                    job.completed(report)
                }
                GetStorageReport(project, dates) => {
                    let report = get_storage_report(me.name(), &project, &dates).await?;
                    job.completed(report)
                }
                GetStorageReports(portal, dates) => {
                    let report = get_storage_reports(me.name(), &portal, &dates).await?;
                    job.completed(report)
                }
                GetLimit(project) => {
                    let limit = get_project_limit(me.name(), &project).await?;
                    job.completed(limit)
                }
                SetLimit(project, limit) => {
                    let limit = set_project_limit(me.name(), &project, limit).await?;
                    job.completed(limit)
                }
                GetProjectQuota(project, volume) => {
                    let quota = get_project_quota(me.name(), &project, &volume).await?;
                    job.completed(quota)
                }
                ClearProjectQuota(project, volume) => {
                    clear_project_quota(me.name(), &project, &volume).await?;
                    job.completed_none()
                }
                SetProjectQuota(project, volume, quota) => {
                    let quota = set_project_quota(me.name(), &project, &volume, &quota).await?;
                    job.completed(quota)
                }
                GetProjectQuotas(project) => {
                    let quotas = get_project_quotas(me.name(), &project).await?;
                    job.completed(quotas)
                }
                GetUserQuota(user, volume) => {
                    let quota = get_user_quota(me.name(), &user, &volume).await?;
                    job.completed(quota)
                }
                ClearUserQuota(user, volume) => {
                    clear_user_quota(me.name(), &user, &volume).await?;
                    job.completed_none()
                }
                SetUserQuota(user, volume, quota) => {
                    let quota = set_user_quota(me.name(), &user, &volume, &quota).await?;
                    job.completed(quota)
                }
                GetUserQuotas(user) => {
                    let quotas = get_user_quotas(me.name(), &user).await?;
                    job.completed(quotas)
                }
                GetHomeDir(user) => {
                    let mapping = get_user_mapping(me.name(), &user).await?;
                    let homedir = get_home_dir(me.name(), &mapping).await?;
                    job.completed(homedir)
                }
                GetProjectDirs(project) => {
                    let mapping = get_project_mapping(me.name(), &project).await?;
                    let dirs = get_project_dirs(me.name(), &mapping).await?;
                    job.completed(dirs)
                }
                GetUserDirs(user) => {
                    let mapping = get_user_mapping(me.name(), &user).await?;
                    let dirs = get_user_dirs(me.name(), &mapping).await?;
                    job.completed(dirs)
                }
                GetLocalHomeDir(mapping) => {
                    let homedir = get_home_dir(me.name(), &mapping).await?;
                    job.completed(homedir)
                }
                GetLocalProjectDirs(mapping) => {
                    let dirs = get_project_dirs(me.name(), &mapping).await?;
                    job.completed(dirs)
                }
                GetLocalUserDirs(mapping) => {
                    let dirs = get_user_dirs(me.name(), &mapping).await?;
                    job.completed(dirs)
                }
                _ => {
                    tracing::error!("Unknown instruction: {:?}", job.instruction());
                    Err(Error::UnknownInstruction(
                        format!("Unknown instruction: {:?}", job.instruction()).to_string(),
                    ))
                }
            }
        }
    }

    // run the agent
    set_notify_runner::<Hpc>(default_notify_runner).await?;
    run(config, cluster_runner).await?;

    Ok(())
}

/// Send a fire-and-forget notification back up the path that the triggering
/// job came from. The notification destination is the job destination reversed,
/// e.g. a job addressed to `brics.aip1.clusters.shared` produces a notification
/// addressed to `shared.clusters.aip1.brics`. The notification is forwarded to
/// the platform agent (next hop upward) and routed from there.
async fn assert_agents_connected() -> Result<(), Error> {
    // check that we are connected to the filesystem and scheduler agents.
    // Do nothing if we aren't
    match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(_) => {}
        None => {
            tracing::error!("No filesystem agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ));
        }
    }

    match agent::scheduler(AGENT_WAIT_TIME).await {
        Some(_) => {}
        None => {
            tracing::error!("No scheduler agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no scheduler agent".to_string(),
            ));
        }
    }

    match agent::account(AGENT_WAIT_TIME).await {
        Some(_) => {}
        None => {
            tracing::error!("No account agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no account agent".to_string(),
            ));
        }
    }

    Ok(())
}

///
/// Wait for a step's job to finish, and turn a job that came back failed into
/// an error here. `what` names the step, as a phrase that reads after "Error"
/// and after "Finished" - e.g. "removing the user's directories".
///
/// This exists because `Job::wait` *returns* the finished job rather than
/// updating the one it was called on. Every one of the helpers below used to
/// write `job.wait().await?; if job.is_error() { ... }`, which asks the
/// pre-wait copy - still pending, so never an error - and so reported success
/// for a step that had failed. On the scheduler helpers nothing else caught it,
/// which meant `add_user` and `add_project` could report success with the Slurm
/// account never created. Routing every step through here is what stops that
/// being writable again.
///
async fn wait_for_step(job: Job, what: &str) -> Result<(), Error> {
    // NOT `job.is_error()` - see above. The state must be read from the job
    // that `wait` hands back.
    let job = job.wait().await?;

    if job.is_error() {
        tracing::error!("Error {}: {:?}", what, job);
        return Err(Error::Call(format!("Error {}: {:?}", what, job)));
    }

    job.result_none()?;

    tracing::info!("Finished {}", what);

    Ok(())
}

///
/// Run the filesystem step and then the scheduler step of an add or remove, and
/// fail with everything that went wrong rather than only the first thing.
///
/// The account agent has already been changed by the time either of these runs:
/// that step goes first precisely so that a failure there aborts before
/// anything else is touched, since without it the mapping cannot be trusted.
/// Past that point there is no clean "nothing happened" to return to, so both
/// remaining steps are attempted even if the first fails: they manage separate
/// systems, and abandoning the second would leave its work undone with nothing
/// recorded about it.
///
/// Whatever failed is then reported together, naming each system, so that the
/// caller learns that the operation did not complete *and* which agent to look
/// at - rather than the failure being logged and swallowed, which is what used
/// to happen on both remove paths.
///
async fn run_both_steps(
    what: &str,
    filesystem: impl std::future::Future<Output = Result<(), Error>>,
    scheduler: impl std::future::Future<Output = Result<(), Error>>,
) -> Result<(), Error> {
    let mut failures: Vec<String> = Vec::new();

    if let Err(e) = filesystem.await {
        tracing::error!("{}: the filesystem agent failed: {}", what, e);
        failures.push(format!("filesystem agent: {}", e));
    }

    if let Err(e) = scheduler.await {
        tracing::error!("{}: the scheduler agent failed: {}", what, e);
        failures.push(format!("scheduler agent: {}", e));
    }

    if failures.is_empty() {
        return Ok(());
    }

    Err(Error::Call(format!(
        "{} failed - {}",
        what,
        failures.join("; ")
    )))
}

///
/// Ask every agent behind this cluster - account, filesystem and scheduler -
/// the same `is_local_*` question about the same mapping, and return true only
/// if all three say yes.
///
/// Nothing here is cached, and nothing is inferred from what a previous
/// `add_*`/`remove_*` job reported: each agent is asked afresh and answers by
/// looking at the system it actually manages. That is the whole point of the
/// question - a caller uses it to find out whether an earlier add or remove
/// really ran to completion, and to re-run it if it did not.
///
/// An agent that cannot be reached is an error rather than a "no". Not knowing
/// whether the work finished is not the same as knowing it did not, and
/// answering "no" here would have a caller re-running an add or remove against
/// a cluster that is only half connected.
///
async fn ask_all_agents(me: &str, instruction: &str, mapping: &str) -> Result<bool, Error> {
    // Fail up front, and with the same message the add/remove paths use, if any
    // of the three is missing - rather than part-way through the loop below.
    assert_agents_connected().await?;

    let agents = [
        ("account", agent::account(AGENT_WAIT_TIME).await),
        ("filesystem", agent::filesystem(AGENT_WAIT_TIME).await),
        ("scheduler", agent::scheduler(AGENT_WAIT_TIME).await),
    ];

    for (role, peer) in agents {
        let Some(peer) = peer else {
            tracing::error!("No {} agent found", role);
            return Err(Error::MissingAgent(format!(
                "Cannot run the job because there is no {} agent",
                role
            )));
        };

        let job = Job::parse(
            &format!("{}.{} {} {}", me, peer.name(), instruction, mapping),
            false,
        )?
        .put(&peer)
        .await?;

        let answer = match job.wait().await?.result::<bool>()? {
            Some(answer) => answer,
            None => {
                tracing::error!("No answer from {} agent {}?", role, peer.name());
                return Err(Error::Call(format!(
                    "The {} agent {} gave no answer to '{} {}'",
                    role,
                    peer.name(),
                    instruction,
                    mapping
                )));
            }
        };

        if !answer {
            tracing::info!(
                "The {} agent {} reports '{} {}' as false",
                role,
                peer.name(),
                instruction,
                mapping
            );
            return Ok(false);
        }
    }

    Ok(true)
}

///
/// Look up the mapping for `user`, distinguishing "there is no such user here"
/// (`Ok(None)`) from "the account agent could not tell us" (`Err`).
///
/// The distinction matters: a lookup that fails to answer is not a user who is
/// not there, and treating an identity service that is merely unreachable as
/// proof of absence would let `is_user_removed` report a user as fully removed
/// during an outage. So the mapping is asked for first, and only if that fails
/// is `is_existing_user` - which answers with a plain bool, and is itself
/// allowed to fail - used to decide which of the two happened.
///
async fn lookup_user_mapping(
    me: &str,
    user: &UserIdentifier,
) -> Result<Option<UserMapping>, Error> {
    match get_user_mapping(me, user).await {
        Ok(mapping) => Ok(Some(mapping)),
        Err(e) => {
            if is_existing_user(me, user).await? {
                Err(e)
            } else {
                tracing::info!("The account agent has no record of user {}", user);
                Ok(None)
            }
        }
    }
}

///
/// Look up the mapping for `project`. As `lookup_user_mapping`, `Ok(None)`
/// means the account agent has no record of the project, and an error means it
/// could not tell us.
///
async fn lookup_project_mapping(
    me: &str,
    project: &ProjectIdentifier,
) -> Result<Option<ProjectMapping>, Error> {
    match get_project_mapping(me, project).await {
        Ok(mapping) => Ok(Some(mapping)),
        Err(e) => {
            if is_existing_project(me, project).await? {
                Err(e)
            } else {
                tracing::info!("The account agent has no record of project {}", project);
                Ok(None)
            }
        }
    }
}

///
/// Return whether `user` has been fully added to this cluster - that is,
/// whether the account, filesystem and scheduler agents all report every part
/// of `add_user` as done.
///
/// Deliberately stronger than `is_existing_user`, which only asks the account
/// agent whether the account is there. An `add_user` that created the account
/// and then failed to create the home directory leaves `is_existing_user` true
/// and this false, which is exactly the case a caller needs to find so that it
/// can re-run the add.
///
async fn is_user_added(me: &str, user: &UserIdentifier) -> Result<bool, Error> {
    // A protected user is one `add_user` deliberately leaves exactly as it
    // found them, returning their existing mapping and reporting success - so
    // there is never anything outstanding for a caller to re-run.
    match is_protected_user(me, user).await {
        Ok(true) => {
            tracing::info!(
                "User {} is not managed by OpenPortal - nothing for add_user to do",
                user
            );
            return Ok(true);
        }
        Err(Error::MissingUser(_)) => {}
        Err(e) => return Err(e),
        _ => {}
    }

    let Some(mapping) = lookup_user_mapping(me, user).await? else {
        return Ok(false);
    };

    ask_all_agents(me, "is_local_user_added", &mapping.to_string()).await
}

///
/// Return whether `user` has been fully removed from this cluster - that is,
/// whether the account, filesystem and scheduler agents all report every part
/// of `remove_user` as done.
///
/// Note that neither account agent deletes the account: `remove_user` disables
/// it and strips its groups, keeping the uid so that the files the filesystem
/// agent recycled rather than deleted still belong to their owner. So a removed
/// user still has a mapping, which is what lets the filesystem and scheduler
/// agents be asked about them at all.
///
async fn is_user_removed(me: &str, user: &UserIdentifier) -> Result<bool, Error> {
    // As in `is_user_added`: `remove_user` is a no-op that reports success for
    // a user OpenPortal does not manage.
    match is_protected_user(me, user).await {
        Ok(true) => {
            tracing::info!(
                "User {} is not managed by OpenPortal - nothing for remove_user to do",
                user
            );
            return Ok(true);
        }
        Err(Error::MissingUser(_)) => {}
        Err(e) => return Err(e),
        _ => {}
    }

    let Some(mapping) = lookup_user_mapping(me, user).await? else {
        // No account record at all, so nothing was ever added here for a
        // removal to leave behind.
        return Ok(true);
    };

    ask_all_agents(me, "is_local_user_removed", &mapping.to_string()).await
}

///
/// Return whether `project` has been fully added to this cluster - that is,
/// whether the account, filesystem and scheduler agents all report every part
/// of `add_project` as done.
///
async fn is_project_added(me: &str, project: &ProjectIdentifier) -> Result<bool, Error> {
    let Some(mapping) = lookup_project_mapping(me, project).await? else {
        return Ok(false);
    };

    ask_all_agents(me, "is_local_project_added", &mapping.to_string()).await
}

///
/// Return whether `project` has been fully removed from this cluster - that is,
/// whether the account, filesystem and scheduler agents all report every part
/// of `remove_project` as done.
///
/// A project is only removed once its users are: `remove_project_from_cluster`
/// leaves the directories and the scheduler account alone while any protected
/// user remains, and the account agent's own check walks the project's members.
///
async fn is_project_removed(me: &str, project: &ProjectIdentifier) -> Result<bool, Error> {
    let Some(mapping) = lookup_project_mapping(me, project).await? else {
        return Ok(true);
    };

    ask_all_agents(me, "is_local_project_removed", &mapping.to_string()).await
}

async fn add_project_to_cluster(
    me: &str,
    project: &ProjectIdentifier,
) -> Result<ProjectMapping, Error> {
    tracing::info!("Adding project to cluster: {}", project);

    // The account agent goes first: if it fails there is no mapping to be
    // trusted, and nothing else has been touched yet.
    let mapping = create_project(me, project).await?;

    // The project directories and the scheduler account are then both
    // attempted, and any failure of either is reported - see `run_both_steps`.
    run_both_steps(
        &format!("Adding project {} to the cluster", project),
        create_project_directories(me, &mapping),
        add_project_to_scheduler(me, project, &mapping),
    )
    .await?;

    Ok(mapping)
}

async fn remove_project_from_cluster(
    me: &str,
    project: &ProjectIdentifier,
) -> Result<ProjectMapping, Error> {
    tracing::info!("Removing project from cluster: {}", project);

    // The account agent goes first, and a failure there aborts before anything
    // else is touched - the mapping the rest of this needs comes from it.
    let mapping = remove_project(me, project).await?;

    // now get the users who remain - if any do, then there
    // are protected users left
    let users = get_accounts(me, project).await?;

    if !users.is_empty() {
        tracing::warn!(
            "Protected users found in project: {:?} - NOT REMOVING!",
            users
        );
        return Ok(mapping);
    }

    run_both_steps(
        &format!("Removing project {} from the cluster", project),
        delete_project_directories(me, &mapping),
        remove_project_from_scheduler(me, project, &mapping),
    )
    .await?;

    Ok(mapping)
}

async fn add_user_to_cluster(me: &str, user: &UserIdentifier) -> Result<UserMapping, Error> {
    match is_protected_user(me, user).await {
        Ok(true) => {
            // get and return the existing user mapping
            return get_user_mapping(me, user).await;
        }
        Err(Error::MissingUser(_)) => {}
        Err(e) => {
            return Err(e);
        }
        _ => {}
    }

    tracing::info!("Adding user to cluster: {}", user);

    // The account agent goes first: if it fails there is no mapping to be
    // trusted, and nothing else has been touched yet.
    let mapping = create_account(me, user).await?;

    // Creating the directories, asking where the home one ended up, and
    // recording that path on the account are one filesystem step: they are a
    // single chain, and a failure anywhere in it leaves the home directory not
    // properly set up. It stops at the first failure for that reason, unlike
    // the filesystem and scheduler steps themselves.
    let filesystem = async {
        create_user_directories(me, &mapping).await?;
        let homedir = get_home_dir(me, &mapping).await?;
        update_homedir(me, user, &homedir).await?;
        Ok::<(), Error>(())
    };

    run_both_steps(
        &format!("Adding user {} to the cluster", user),
        filesystem,
        add_user_to_scheduler(me, user, &mapping),
    )
    .await?;

    Ok(mapping)
}

async fn remove_user_from_cluster(me: &str, user: &UserIdentifier) -> Result<UserMapping, Error> {
    match is_protected_user(me, user).await {
        Ok(true) => {
            // get and return the existing user mapping
            return get_user_mapping(me, user).await;
        }
        Err(Error::MissingUser(_)) => {}
        Err(e) => {
            return Err(e);
        }
        _ => {}
    }

    tracing::info!("Removing user from cluster: {}", user);

    // The account agent goes first, and a failure there aborts before anything
    // else is touched - the mapping the rest of this needs comes from it.
    let mapping = remove_account(me, user).await?;

    run_both_steps(
        &format!("Removing user {} from the cluster", user),
        delete_user_directories(me, &mapping),
        remove_user_from_scheduler(me, user, &mapping),
    )
    .await?;

    Ok(mapping)
}

async fn get_projects(me: &str, portal: &PortalIdentifier) -> Result<Vec<ProjectMapping>, Error> {
    // find the Account agent
    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            // send the add_job to the account agent
            let job = Job::parse(
                &format!("{}.{} get_projects {}", me, account.name(), portal),
                false,
            )?
            .put(&account)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<Vec<ProjectMapping>>()?;

            match result {
                Some(projects) => {
                    tracing::debug!("Projects retrieved from account agent: {:?}", projects);
                    Ok(projects)
                }
                None => {
                    tracing::warn!("No projects found?");
                    Ok(vec![])
                }
            }
        }
        None => {
            tracing::error!("No account agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no account agent".to_string(),
            ))
        }
    }
}

async fn create_project(me: &str, project: &ProjectIdentifier) -> Result<ProjectMapping, Error> {
    // find the Account agent
    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            // send the add_job to the account agent
            let job = Job::parse(
                &format!("{}.{} add_project {}", me, account.name(), project),
                false,
            )?
            .put(&account)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<ProjectMapping>()?;

            match result {
                Some(mapping) => {
                    tracing::info!("Project added to account agent: {:?}", mapping);
                    Ok(mapping)
                }
                None => {
                    tracing::error!("Error creating the project group: {:?}", job);
                    Err(Error::Call(
                        format!("Error creating the project group: {:?}", job).to_string(),
                    ))
                }
            }
        }
        None => {
            tracing::error!("No account agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no account agent".to_string(),
            ))
        }
    }
}

async fn remove_project(me: &str, project: &ProjectIdentifier) -> Result<ProjectMapping, Error> {
    // find the Account agent
    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            // send the add_job to the account agent
            let job = Job::parse(
                &format!("{}.{} remove_project {}", me, account.name(), project),
                false,
            )?
            .put(&account)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<ProjectMapping>()?;

            match result {
                Some(mapping) => {
                    tracing::info!("Project removed from account agent: {:?}", mapping);
                    Ok(mapping)
                }
                None => {
                    tracing::error!("Error removing the project group: {:?}", job);
                    Err(Error::Call(
                        format!("Error removing the project group: {:?}", job).to_string(),
                    ))
                }
            }
        }
        None => {
            tracing::error!("No account agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no account agent".to_string(),
            ))
        }
    }
}

async fn get_accounts(me: &str, project: &ProjectIdentifier) -> Result<Vec<UserMapping>, Error> {
    // find the Account agent
    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            // send the add_job to the account agent
            let job = Job::parse(
                &format!("{}.{} get_users {}", me, account.name(), project),
                false,
            )?
            .put(&account)
            .await?;

            let result = job.wait().await?.result::<Vec<UserMapping>>()?;

            match result {
                Some(users) => {
                    tracing::debug!("Users retrieved from account agent: {:?}", users);
                    Ok(users)
                }
                None => {
                    tracing::warn!("No users found?");
                    Ok(vec![])
                }
            }
        }
        None => {
            tracing::error!("No account agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no account agent".to_string(),
            ))
        }
    }
}

async fn create_account(me: &str, user: &UserIdentifier) -> Result<UserMapping, Error> {
    // find the Account agent
    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            // send the add_job to the account agent
            let job = Job::parse(
                &format!("{}.{} add_user {}", me, account.name(), user),
                false,
            )?
            .put(&account)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<UserMapping>()?;

            match result {
                Some(mapping) => {
                    tracing::info!("User added to account agent: {:?}", mapping);
                    Ok(mapping)
                }
                None => {
                    tracing::error!("Error creating the user's account: {:?}", job);
                    Err(Error::Call(
                        format!("Error creating the user's account: {:?}", job).to_string(),
                    ))
                }
            }
        }
        None => {
            tracing::error!("No account agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no account agent".to_string(),
            ))
        }
    }
}

async fn remove_account(me: &str, user: &UserIdentifier) -> Result<UserMapping, Error> {
    // find the Account agent
    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            // send the add_job to the account agent
            let job = Job::parse(
                &format!("{}.{} remove_user {}", me, account.name(), user),
                false,
            )?
            .put(&account)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<UserMapping>()?;

            match result {
                Some(mapping) => {
                    tracing::info!("User removed from account agent: {:?}", mapping);
                    Ok(mapping)
                }
                None => {
                    tracing::error!("Error removing the user's account: {:?}", job);
                    Err(Error::Call(
                        format!("Error removing the user's account: {:?}", job).to_string(),
                    ))
                }
            }
        }
        None => {
            tracing::error!("No account agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no account agent".to_string(),
            ))
        }
    }
}

async fn get_project_mapping(
    me: &str,
    project: &ProjectIdentifier,
) -> Result<ProjectMapping, Error> {
    // find the Account agent
    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            // send the add_job to the account agent
            let job = Job::parse(
                &format!("{}.{} get_project_mapping {}", me, account.name(), project),
                false,
            )?
            .put(&account)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<ProjectMapping>()?;

            match result {
                Some(mapping) => {
                    tracing::debug!(
                        "Project mapping retrieved from account agent: {:?}",
                        mapping
                    );
                    Ok(mapping)
                }
                None => {
                    tracing::error!("No project mapping found?");
                    Err(Error::MissingProject(format!(
                        "Could not find a mapping for project {}",
                        project
                    )))
                }
            }
        }
        None => {
            tracing::error!("No account agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no account agent".to_string(),
            ))
        }
    }
}

async fn get_user_mapping(me: &str, user: &UserIdentifier) -> Result<UserMapping, Error> {
    // find the Account agent
    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            // send the add_job to the account agent
            let job = Job::parse(
                &format!("{}.{} get_user_mapping {}", me, account.name(), user),
                false,
            )?
            .put(&account)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<UserMapping>()?;

            match result {
                Some(mapping) => {
                    tracing::info!("User mapping retrieved from account agent: {:?}", mapping);
                    Ok(mapping)
                }
                None => {
                    tracing::error!("No user mapping found?");
                    Err(Error::MissingUser(format!(
                        "Could not find a mapping for user {}",
                        user
                    )))
                }
            }
        }
        None => {
            tracing::error!("No account agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no account agent".to_string(),
            ))
        }
    }
}

async fn create_project_directories(me: &str, mapping: &ProjectMapping) -> Result<(), Error> {
    // find the Filesystem agent
    match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => {
            // send the add_job to the filesystem agent
            let job = Job::parse(
                &format!("{}.{} add_local_project {}", me, filesystem.name(), mapping),
                false,
            )?
            .put(&filesystem)
            .await?;

            wait_for_step(job, "creating the project directories").await
        }
        None => {
            tracing::error!("No filesystem agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ))
        }
    }
}

async fn delete_project_directories(me: &str, mapping: &ProjectMapping) -> Result<(), Error> {
    // find the Filesystem agent
    match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => {
            // send the add_job to the filesystem agent
            let job = Job::parse(
                &format!(
                    "{}.{} remove_local_project {}",
                    me,
                    filesystem.name(),
                    mapping
                ),
                false,
            )?
            .put(&filesystem)
            .await?;

            wait_for_step(job, "removing the project directories").await
        }
        None => {
            tracing::error!("No filesystem agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ))
        }
    }
}

async fn create_user_directories(me: &str, mapping: &UserMapping) -> Result<(), Error> {
    // find the Filesystem agent
    match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => {
            // send the add_job to the filesystem agent
            let job = Job::parse(
                &format!("{}.{} add_local_user {}", me, filesystem.name(), mapping),
                false,
            )?
            .put(&filesystem)
            .await?;

            wait_for_step(job, "creating the user's directories").await
        }
        None => {
            tracing::error!("No filesystem agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ))
        }
    }
}

async fn delete_user_directories(me: &str, mapping: &UserMapping) -> Result<(), Error> {
    // find the Filesystem agent
    match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => {
            // send the add_job to the filesystem agent
            let job = Job::parse(
                &format!("{}.{} remove_local_user {}", me, filesystem.name(), mapping),
                false,
            )?
            .put(&filesystem)
            .await?;

            wait_for_step(job, "removing the user's directories").await
        }
        None => {
            tracing::error!("No filesystem agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ))
        }
    }
}

async fn update_homedir(me: &str, user: &UserIdentifier, homedir: &str) -> Result<String, Error> {
    // find the Account agent
    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            // send the add_job to the account agent
            let job = Job::parse(
                &format!(
                    "{}.{} update_homedir {} {}",
                    me,
                    account.name(),
                    user,
                    homedir
                ),
                false,
            )?
            .put(&account)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<String>()?;

            match result {
                Some(homedir) => {
                    tracing::info!("User {} homedir updated: {:?}", user, homedir);
                    Ok(homedir)
                }
                None => {
                    tracing::error!("Error updating the user's homedir: {:?}", job);
                    Err(Error::Call(
                        format!("Error updating the user's homedir: {:?}", job).to_string(),
                    ))
                }
            }
        }
        None => {
            tracing::error!("No account agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no account agent".to_string(),
            ))
        }
    }
}

async fn add_project_to_scheduler(
    me: &str,
    project: &ProjectIdentifier,
    mapping: &ProjectMapping,
) -> Result<(), Error> {
    // find the Scheduler agent
    match agent::scheduler(AGENT_WAIT_TIME).await {
        Some(scheduler) => {
            // send the add_job to the scheduler agent
            let job = Job::parse(
                &format!("{}.{} add_local_project {}", me, scheduler.name(), mapping),
                false,
            )?
            .put(&scheduler)
            .await?;

            wait_for_step(job, &format!("adding project {} to the scheduler", project)).await
        }
        None => {
            tracing::error!("No scheduler agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no scheduler agent".to_string(),
            ))
        }
    }
}

async fn remove_project_from_scheduler(
    me: &str,
    project: &ProjectIdentifier,
    mapping: &ProjectMapping,
) -> Result<(), Error> {
    // find the Scheduler agent
    match agent::scheduler(AGENT_WAIT_TIME).await {
        Some(scheduler) => {
            // send the add_job to the scheduler agent
            let job = Job::parse(
                &format!(
                    "{}.{} remove_local_project {}",
                    me,
                    scheduler.name(),
                    mapping
                ),
                false,
            )?
            .put(&scheduler)
            .await?;

            wait_for_step(
                job,
                &format!("removing project {} from the scheduler", project),
            )
            .await
        }
        None => {
            tracing::error!("No scheduler agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no scheduler agent".to_string(),
            ))
        }
    }
}

async fn add_user_to_scheduler(
    me: &str,
    user: &UserIdentifier,
    mapping: &UserMapping,
) -> Result<(), Error> {
    // find the Scheduler agent
    match agent::scheduler(AGENT_WAIT_TIME).await {
        Some(scheduler) => {
            // send the add_job to the scheduler agent
            let job = Job::parse(
                &format!("{}.{} add_local_user {}", me, scheduler.name(), mapping),
                false,
            )?
            .put(&scheduler)
            .await?;

            wait_for_step(job, &format!("adding user {} to the scheduler", user)).await
        }
        None => {
            tracing::error!("No scheduler agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no scheduler agent".to_string(),
            ))
        }
    }
}

async fn remove_user_from_scheduler(
    me: &str,
    user: &UserIdentifier,
    mapping: &UserMapping,
) -> Result<(), Error> {
    // find the Scheduler agent
    match agent::scheduler(AGENT_WAIT_TIME).await {
        Some(scheduler) => {
            // send the add_job to the scheduler agent
            let job = Job::parse(
                &format!("{}.{} remove_local_user {}", me, scheduler.name(), mapping),
                false,
            )?
            .put(&scheduler)
            .await?;

            wait_for_step(job, &format!("removing user {} from the scheduler", user)).await
        }
        None => {
            tracing::error!("No scheduler agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no scheduler agent".to_string(),
            ))
        }
    }
}

async fn get_usage_report(
    me: &str,
    project: &ProjectMapping,
    dates: &DateRange,
) -> Result<ProjectUsageReport, Error> {
    // get the schedule agent
    let scheduler = match agent::scheduler(AGENT_WAIT_TIME).await {
        Some(scheduler) => scheduler,
        None => {
            tracing::error!("No scheduler agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no scheduler agent".to_string(),
            ));
        }
    };

    // ask the scheduler for the usage report of this project
    let job = Job::parse(
        &format!(
            "{}.{} get_local_usage_report {} {}",
            me,
            scheduler.name(),
            project,
            dates
        ),
        false,
    )?
    .put(&scheduler)
    .await?;

    // Wait for the job to complete... - get the resulting ProjectUsageReport
    let mut report = match job.wait().await?.result::<ProjectUsageReport>()? {
        Some(report) => report,
        None => ProjectUsageReport::new(project.project()),
    };

    // now add in all of the mappings that we know about
    report.add_mappings(&get_accounts(me, project.project()).await?)?;

    Ok(report)
}

async fn get_usage_reports(
    me: &str,
    portal: &PortalIdentifier,
    dates: &DateRange,
) -> Result<UsageReport, Error> {
    // get the list of all projects associated with this portal
    let projects = get_projects(me, portal).await?;

    let mut report = UsageReport::new(portal);

    for project in projects {
        let project_report = get_usage_report(me, &project, dates).await?;
        report.set_report(project_report)?;
    }

    Ok(report)
}

async fn get_storage_report(
    me: &str,
    project: &ProjectIdentifier,
    dates: &DateRange,
) -> Result<ProjectStorageReport, Error> {
    let mapping = get_project_mapping(me, project).await?;

    let filesystem = match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => filesystem,
        None => {
            tracing::error!("No filesystem agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ));
        }
    };

    // Delegate to the filesystem agent, which will call back with get_users
    let job = Job::parse(
        &format!(
            "{}.{} get_local_storage_report {} {}",
            me,
            filesystem.name(),
            mapping,
            dates
        ),
        false,
    )?
    .put(&filesystem)
    .await?;

    match job.wait().await?.result::<ProjectStorageReport>()? {
        Some(report) => Ok(report),
        None => Ok(ProjectStorageReport::new(project)),
    }
}

async fn get_storage_reports(
    me: &str,
    portal: &PortalIdentifier,
    dates: &DateRange,
) -> Result<StorageReport, Error> {
    let projects = get_projects(me, portal).await?;

    let mut report = StorageReport::new(portal);

    for project in projects {
        let project_report = get_storage_report(me, project.project(), dates).await?;
        report.set_report(project_report)?;
    }

    Ok(report)
}

async fn get_project_limit(me: &str, project: &ProjectIdentifier) -> Result<Usage, Error> {
    // get the mapping for this project
    let mapping = get_project_mapping(me, project).await?;

    // find the scheduler agent
    let scheduler = match agent::scheduler(AGENT_WAIT_TIME).await {
        Some(scheduler) => scheduler,
        None => {
            tracing::error!("No scheduler agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no scheduler agent".to_string(),
            ));
        }
    };

    // ask the scheduler for the project limit
    let job = Job::parse(
        &format!("{}.{} get_local_limit {}", me, scheduler.name(), mapping),
        false,
    )?;

    let job = job.put(&scheduler).await?;

    // Wait for the job to complete... - get the resulting Usage
    let limit = match job.wait().await?.result::<Usage>()? {
        Some(usage) => usage,
        None => Usage::new(0),
    };

    Ok(limit)
}

pub async fn set_project_limit(
    me: &str,
    project: &ProjectIdentifier,
    limit: Usage,
) -> Result<Usage, Error> {
    // get the mapping for this project
    let mapping = get_project_mapping(me, project).await?;

    // find the scheduler agent
    let scheduler = match agent::scheduler(AGENT_WAIT_TIME).await {
        Some(scheduler) => scheduler,
        None => {
            tracing::error!("No scheduler agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no scheduler agent".to_string(),
            ));
        }
    };

    // ask the scheduler to set the project limit
    let job = Job::parse(
        &format!(
            "{}.{} set_local_limit {} {}",
            me,
            scheduler.name(),
            mapping,
            limit.seconds()
        ),
        false,
    )?;

    let job = job.put(&scheduler).await?;

    // Wait for the job to complete... - get the resulting Usage
    let limit = match job.wait().await?.result::<Usage>()? {
        Some(usage) => usage,
        None => Usage::new(0),
    };

    Ok(limit)
}

async fn clear_project_quota(
    me: &str,
    project: &ProjectIdentifier,
    volume: &Volume,
) -> Result<Quota, Error> {
    // get the mapping for this project
    let mapping = get_project_mapping(me, project).await?;

    // find the filesystem agent
    let filesystem = match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => filesystem,
        None => {
            tracing::error!("No filesystem agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ));
        }
    };

    // ask the filesystem to clear the project quota
    let job = Job::parse(
        &format!(
            "{}.{} clear_local_project_quota {} {}",
            me,
            filesystem.name(),
            mapping,
            volume
        ),
        false,
    )?
    .put(&filesystem)
    .await?;

    // Wait for the job to complete... - get the resulting Quota
    match job.wait().await?.result::<Quota>() {
        Ok(Some(quota)) => Ok(quota),
        Ok(None) => {
            tracing::error!(
                "Error clearing quota for project {} on volume {}",
                project,
                volume
            );
            Err(Error::Call(format!(
                "Error clearing quota for project {} on volume {}",
                project, volume
            )))
        }
        Err(e) => Err(e),
    }
}

async fn set_project_quota(
    me: &str,
    project: &ProjectIdentifier,
    volume: &Volume,
    limit: &greatwestern::storage::QuotaLimit,
) -> Result<Quota, Error> {
    // get the mapping for this project
    let mapping = get_project_mapping(me, project).await?;

    // find the filesystem agent
    let filesystem = match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => filesystem,
        None => {
            tracing::error!("No filesystem agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ));
        }
    };

    // ask the filesystem to set the project quota
    let job = Job::parse(
        &format!(
            "{}.{} set_local_project_quota {} {} {}",
            me,
            filesystem.name(),
            mapping,
            volume,
            limit
        ),
        false,
    )?
    .put(&filesystem)
    .await?;

    // Wait for the job to complete... - get the resulting Quota
    match job.wait().await?.result::<Quota>() {
        Ok(Some(quota)) => Ok(quota),
        Ok(None) => {
            tracing::error!(
                "Error setting quota for project {} on volume {}",
                project,
                volume
            );
            Err(Error::Call(format!(
                "Error setting quota for project {} on volume {}",
                project, volume
            )))
        }
        Err(e) => Err(e),
    }
}

async fn get_project_quota(
    me: &str,
    project: &ProjectIdentifier,
    volume: &Volume,
) -> Result<Quota, Error> {
    // get the mapping for this project
    let mapping = get_project_mapping(me, project).await?;

    // find the filesystem agent
    let filesystem = match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => filesystem,
        None => {
            tracing::error!("No filesystem agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ));
        }
    };

    // ask the filesystem for the project quota
    let job = Job::parse(
        &format!(
            "{}.{} get_local_project_quota {} {}",
            me,
            filesystem.name(),
            mapping,
            volume
        ),
        false,
    )?
    .put(&filesystem)
    .await?;

    // Wait for the job to complete... - get the resulting Quota
    match job.wait().await?.result::<Quota>() {
        Ok(Some(quota)) => Ok(quota),
        Ok(None) => {
            tracing::warn!(
                "No quota found for project {} on volume {}",
                project,
                volume
            );
            Err(Error::NotFound(format!(
                "No quota found for project {} on volume {}",
                project, volume
            )))
        }
        Err(e) => Err(e),
    }
}

async fn get_project_quotas(
    me: &str,
    project: &ProjectIdentifier,
) -> Result<HashMap<Volume, Quota>, Error> {
    // get the mapping for this project
    let mapping = get_project_mapping(me, project).await?;

    // find the filesystem agent
    let filesystem = match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => filesystem,
        None => {
            tracing::error!("No filesystem agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ));
        }
    };

    // ask the filesystem for the project quota
    let job = Job::parse(
        &format!(
            "{}.{} get_local_project_quotas {}",
            me,
            filesystem.name(),
            mapping
        ),
        false,
    )?
    .put(&filesystem)
    .await?;

    // Wait for the job to complete... - get the resulting Quota
    match job.wait().await?.result::<HashMap<Volume, Quota>>() {
        Ok(Some(quotas)) => Ok(quotas),
        Ok(None) => {
            tracing::warn!("No quotas found for project {}", project);
            Ok(HashMap::new())
        }
        Err(e) => Err(e),
    }
}

async fn clear_user_quota(
    me: &str,
    user: &UserIdentifier,
    volume: &Volume,
) -> Result<Quota, Error> {
    // get the mapping for this user
    let mapping = get_user_mapping(me, user).await?;

    // find the filesystem agent
    let filesystem = match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => filesystem,
        None => {
            tracing::error!("No filesystem agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ));
        }
    };

    // ask the filesystem to clear the user quota
    let job = Job::parse(
        &format!(
            "{}.{} clear_local_user_quota {} {}",
            me,
            filesystem.name(),
            mapping,
            volume
        ),
        false,
    )?
    .put(&filesystem)
    .await?;

    // Wait for the job to complete... - get the resulting Quota
    match job.wait().await?.result::<Quota>() {
        Ok(Some(quota)) => Ok(quota),
        Ok(None) => {
            tracing::error!(
                "Error clearing quota for user {} on volume {}",
                user,
                volume
            );
            Err(Error::Call(format!(
                "Error clearing quota for user {} on volume {}",
                user, volume
            )))
        }
        Err(e) => Err(e),
    }
}

async fn set_user_quota(
    me: &str,
    user: &UserIdentifier,
    volume: &Volume,
    limit: &greatwestern::storage::QuotaLimit,
) -> Result<Quota, Error> {
    // get the mapping for this user
    let mapping = get_user_mapping(me, user).await?;

    // find the filesystem agent
    let filesystem = match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => filesystem,
        None => {
            tracing::error!("No filesystem agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ));
        }
    };

    // ask the filesystem to set the user quota
    let job = Job::parse(
        &format!(
            "{}.{} set_local_user_quota {} {} {}",
            me,
            filesystem.name(),
            mapping,
            volume,
            limit
        ),
        false,
    )?
    .put(&filesystem)
    .await?;

    // Wait for the job to complete... - get the resulting Quota
    match job.wait().await?.result::<Quota>() {
        Ok(Some(quota)) => Ok(quota),
        Ok(None) => {
            tracing::error!("Error setting quota for user {} on volume {}", user, volume);
            Err(Error::Call(format!(
                "Error setting quota for user {} on volume {}",
                user, volume
            )))
        }
        Err(e) => Err(e),
    }
}

async fn get_user_quota(me: &str, user: &UserIdentifier, volume: &Volume) -> Result<Quota, Error> {
    // get the mapping for this user
    let mapping = get_user_mapping(me, user).await?;

    // find the filesystem agent
    let filesystem = match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => filesystem,
        None => {
            tracing::error!("No filesystem agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ));
        }
    };

    // ask the filesystem for the user quota
    let job = Job::parse(
        &format!(
            "{}.{} get_local_user_quota {} {}",
            me,
            filesystem.name(),
            mapping,
            volume
        ),
        false,
    )?
    .put(&filesystem)
    .await?;

    // Wait for the job to complete... - get the resulting Quota
    match job.wait().await?.result::<Quota>() {
        Ok(Some(quota)) => Ok(quota),
        Ok(None) => {
            tracing::warn!("No quota found for user {} on volume {}", user, volume);
            Err(Error::NotFound(format!(
                "No quota found for user {} on volume {}",
                user, volume
            )))
        }
        Err(e) => Err(e),
    }
}

async fn get_user_quotas(me: &str, user: &UserIdentifier) -> Result<HashMap<Volume, Quota>, Error> {
    // get the mapping for this user
    let mapping = get_user_mapping(me, user).await?;

    // find the filesystem agent
    let filesystem = match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => filesystem,
        None => {
            tracing::error!("No filesystem agent found");
            return Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ));
        }
    };

    // ask the filesystem for the user quotas
    let job = Job::parse(
        &format!(
            "{}.{} get_local_user_quotas {}",
            me,
            filesystem.name(),
            mapping
        ),
        false,
    )?
    .put(&filesystem)
    .await?;

    // Wait for the job to complete... - get the resulting Quotas
    match job.wait().await?.result::<HashMap<Volume, Quota>>() {
        Ok(Some(quotas)) => Ok(quotas),
        Ok(None) => {
            tracing::warn!("No quotas found for user {}", user);
            Ok(HashMap::new())
        }
        Err(e) => Err(e),
    }
}

async fn is_protected_user(me: &str, user: &UserIdentifier) -> Result<bool, Error> {
    // find the Account agent
    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            // send the add_job to the account agent
            let job = Job::parse(
                &format!("{}.{} is_protected_user {}", me, account.name(), user),
                false,
            )?
            .put(&account)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<bool>()?;

            match result {
                Some(is_protected) => {
                    tracing::debug!("User is protected: {}", is_protected);
                    Ok(is_protected)
                }
                None => {
                    tracing::error!("No information found?");
                    Err(Error::MissingUser(format!(
                        "Could not find information for user {}",
                        user
                    )))
                }
            }
        }
        None => {
            tracing::error!("No account agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no account agent".to_string(),
            ))
        }
    }
}

async fn is_existing_user(me: &str, user: &UserIdentifier) -> Result<bool, Error> {
    // find the Account agent
    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            // send the add_job to the account agent
            let job = Job::parse(
                &format!("{}.{} is_existing_user {}", me, account.name(), user),
                false,
            )?
            .put(&account)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<bool>()?;

            match result {
                Some(exists) => {
                    tracing::debug!("User exists: {}", exists);
                    Ok(exists)
                }
                None => {
                    tracing::error!("No information found?");
                    Err(Error::MissingUser(format!(
                        "Could not find information for user {}",
                        user
                    )))
                }
            }
        }
        None => {
            tracing::error!("No account agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no account agent".to_string(),
            ))
        }
    }
}

async fn is_blocked_user(me: &str, user: &UserIdentifier) -> Result<bool, Error> {
    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            let job = Job::parse(
                &format!("{}.{} is_blocked_user {}", me, account.name(), user),
                false,
            )?
            .put(&account)
            .await?;

            let result = job.wait().await?.result::<bool>()?;

            match result {
                Some(is_blocked) => {
                    tracing::debug!("User is blocked: {}", is_blocked);
                    Ok(is_blocked)
                }
                None => Err(Error::MissingUser(format!(
                    "Could not find information for user {}",
                    user
                ))),
            }
        }
        None => Err(Error::MissingAgent(
            "Cannot run the job because there is no account agent".to_string(),
        )),
    }
}

async fn block_user_on_cluster(me: &str, user: &UserIdentifier) -> Result<UserMapping, Error> {
    match is_protected_user(me, user).await {
        Ok(true) => return get_user_mapping(me, user).await,
        Err(Error::MissingUser(_)) => {}
        Err(e) => return Err(e),
        _ => {}
    }

    tracing::info!("Blocking user on cluster: {}", user);

    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            let job = Job::parse(
                &format!("{}.{} block_user {}", me, account.name(), user),
                false,
            )?
            .put(&account)
            .await?;

            let result = job.wait().await?.result::<UserMapping>()?;

            match result {
                Some(mapping) => {
                    tracing::info!("User blocked: {:?}", mapping);
                    Ok(mapping)
                }
                None => Err(Error::Call(
                    format!("Error blocking user: {:?}", job).to_string(),
                )),
            }
        }
        None => Err(Error::MissingAgent(
            "Cannot run the job because there is no account agent".to_string(),
        )),
    }
}

async fn block_project_on_cluster(
    me: &str,
    project: &ProjectIdentifier,
) -> Result<Vec<UserMapping>, Error> {
    tracing::info!("Blocking all users in project: {}", project);

    let users = get_accounts(me, project).await?;

    let mut mappings = Vec::new();

    for user_mapping in &users {
        match block_user_on_cluster(me, user_mapping.user()).await {
            Ok(mapping) => mappings.push(mapping),
            Err(e) => tracing::error!(
                "Error blocking user {} in project {}: {:?}",
                user_mapping.user(),
                project,
                e
            ),
        }
    }

    Ok(mappings)
}

async fn unblock_project_on_cluster(
    me: &str,
    project: &ProjectIdentifier,
) -> Result<Vec<UserMapping>, Error> {
    tracing::info!("Unblocking all users in project: {}", project);

    let users = get_accounts(me, project).await?;

    let mut mappings = Vec::new();

    for user_mapping in &users {
        match unblock_user_on_cluster(me, user_mapping.user()).await {
            Ok(mapping) => mappings.push(mapping),
            Err(e) => tracing::error!(
                "Error unblocking user {} in project {}: {:?}",
                user_mapping.user(),
                project,
                e
            ),
        }
    }

    Ok(mappings)
}

async fn is_blocked_project_on_cluster(
    me: &str,
    project: &ProjectIdentifier,
) -> Result<bool, Error> {
    let users = get_accounts(me, project).await?;

    if users.is_empty() {
        return Ok(false);
    }

    for user_mapping in &users {
        if !is_blocked_user(me, user_mapping.user()).await? {
            return Ok(false);
        }
    }

    Ok(true)
}

async fn unblock_user_on_cluster(me: &str, user: &UserIdentifier) -> Result<UserMapping, Error> {
    match is_protected_user(me, user).await {
        Ok(true) => return get_user_mapping(me, user).await,
        Err(Error::MissingUser(_)) => {}
        Err(e) => return Err(e),
        _ => {}
    }

    tracing::info!("Unblocking user on cluster: {}", user);

    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            let job = Job::parse(
                &format!("{}.{} unblock_user {}", me, account.name(), user),
                false,
            )?
            .put(&account)
            .await?;

            let result = job.wait().await?.result::<UserMapping>()?;

            match result {
                Some(mapping) => {
                    tracing::info!("User unblocked: {:?}", mapping);
                    Ok(mapping)
                }
                None => Err(Error::Call(
                    format!("Error unblocking user: {:?}", job).to_string(),
                )),
            }
        }
        None => Err(Error::MissingAgent(
            "Cannot run the job because there is no account agent".to_string(),
        )),
    }
}

async fn is_existing_project(me: &str, project: &ProjectIdentifier) -> Result<bool, Error> {
    // find the Account agent
    match agent::account(AGENT_WAIT_TIME).await {
        Some(account) => {
            // send the add_job to the account agent
            let job = Job::parse(
                &format!("{}.{} is_existing_project {}", me, account.name(), project),
                false,
            )?
            .put(&account)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<bool>()?;

            match result {
                Some(exists) => {
                    tracing::debug!("Project exists: {}", exists);
                    Ok(exists)
                }
                None => {
                    tracing::error!("No information found?");
                    Err(Error::MissingProject(format!(
                        "Could not find information for project {}",
                        project
                    )))
                }
            }
        }
        None => {
            tracing::error!("No account agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no account agent".to_string(),
            ))
        }
    }
}

async fn get_home_dir(me: &str, mapping: &UserMapping) -> Result<String, Error> {
    // find the Filesystem agent
    match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => {
            // send the add_job to the filesystem agent
            let job = Job::parse(
                &format!(
                    "{}.{} get_local_home_dir {}",
                    me,
                    filesystem.name(),
                    mapping
                ),
                false,
            )?
            .put(&filesystem)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<String>()?;

            match result {
                Some(homedir) => {
                    tracing::debug!("User homedir retrieved: {:?}", homedir);
                    Ok(homedir)
                }
                None => {
                    tracing::error!("No homedir found?");
                    Err(Error::MissingUser(format!(
                        "Could not find homedir for user {}",
                        mapping
                    )))
                }
            }
        }
        None => {
            tracing::error!("No filesystem agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ))
        }
    }
}

async fn get_project_dirs(me: &str, mapping: &ProjectMapping) -> Result<Vec<String>, Error> {
    // find the Filesystem agent
    match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => {
            // send the add_job to the filesystem agent
            let job = Job::parse(
                &format!(
                    "{}.{} get_local_project_dirs {}",
                    me,
                    filesystem.name(),
                    mapping
                ),
                false,
            )?
            .put(&filesystem)
            .await?;

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<Vec<String>>()?;

            match result {
                Some(dirs) => {
                    tracing::debug!("Project directories retrieved: {:?}", dirs);
                    Ok(dirs)
                }
                None => {
                    tracing::error!("No directories found?");
                    Err(Error::MissingProject(format!(
                        "Could not find directories for project {}",
                        mapping
                    )))
                }
            }
        }
        None => {
            tracing::error!("No filesystem agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ))
        }
    }
}

async fn get_user_dirs(me: &str, mapping: &UserMapping) -> Result<Vec<String>, Error> {
    // find the Filesystem agent
    match agent::filesystem(AGENT_WAIT_TIME).await {
        Some(filesystem) => {
            // send the job to the filesystem agent
            let job = Job::parse(
                &format!(
                    "{}.{} get_local_user_dirs {}",
                    me,
                    filesystem.name(),
                    mapping
                ),
                false,
            )?
            .put(&filesystem)
            .await?;

            // Wait for the job to complete
            let result = job.wait().await?.result::<Vec<String>>()?;

            match result {
                Some(dirs) => {
                    tracing::debug!("User directories retrieved: {:?}", dirs);
                    Ok(dirs)
                }
                None => {
                    tracing::error!("No directories found?");
                    Err(Error::MissingUser(format!(
                        "Could not find directories for user {}",
                        mapping
                    )))
                }
            }
        }
        None => {
            tracing::error!("No filesystem agent found");
            Err(Error::MissingAgent(
                "Cannot run the job because there is no filesystem agent".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greatwestern::grammar::UserIdentifier;
    use templemeads::job::Job;

    /// Every delegation in this file builds its Job by formatting identifiers
    /// into a space-separated command string - e.g.
    /// `format!("{}.{} is_protected_user {}", me, account.name(), user)` - and
    /// hands the result to `Job::parse`. That is only safe because an
    /// identifier cannot contain whitespace or extra dots: if one could, a
    /// peer-supplied user or project name would inject an extra argument, or
    /// extend the destination path past the agent we meant to address.
    ///
    /// The identifier parsers enforce that, but they live in another crate, so
    /// pin the composition here - this is the invariant `op-cluster` actually
    /// depends on.
    /// `Job::wait` returns the finished job rather than updating the one it was
    /// called on. Every step helper in this file used to ignore that and ask
    /// its own pre-wait binding whether the job had failed - which it never
    /// had, because that copy is still pending - so a failed step reported
    /// success. On the scheduler helpers nothing else caught it, and `add_user`
    /// could report success with the Slurm account never created.
    ///
    /// `wait_for_step` is the one place that reads the state now. This pins the
    /// property it depends on: a job's own error state does not propagate to
    /// the value it was produced from, so the returned job is the only thing
    /// worth asking.
    #[test]
    fn test_a_jobs_error_state_is_not_visible_on_the_value_it_came_from() {
        let job: Job<Hpc> = match Job::parse(
            "cluster1.slurm add_local_user bob.proj.brics:bob:proj",
            false,
        ) {
            Ok(job) => job,
            Err(e) => unreachable!("job: {:?}", e),
        };

        // Only a job that has been put on a board can fail, so move it to the
        // state `put` leaves it in - which is exactly the state the stale
        // binding was stuck in.
        let job = match job.pending() {
            Ok(job) => job,
            Err(e) => unreachable!("pending: {:?}", e),
        };

        assert!(!job.is_error(), "a job waiting to run is not an error");

        let errored = match job.errored("the scheduler said no") {
            Ok(errored) => errored,
            Err(e) => unreachable!("errored: {:?}", e),
        };

        assert!(errored.is_error(), "the returned job carries the failure");
        assert!(
            !job.is_error(),
            "the original binding must stay clean - this is exactly why \
             `job.wait().await?; if job.is_error()` never saw a failure, and \
             why every step must read the job that `wait` hands back"
        );
    }

    #[test]
    fn test_delegated_commands_cannot_be_extended_by_a_peer_supplied_identifier() {
        let job: Job<Hpc> =
            match Job::parse("cluster1.freeipa is_protected_user bob.proj.brics", false) {
                Ok(job) => job,
                Err(e) => unreachable!("job: {:?}", e),
            };

        assert_eq!(job.destination().to_string(), "cluster1.freeipa");
        assert_eq!(
            job.instruction().to_string(),
            "is_protected_user bob.proj.brics"
        );

        // An identifier that would break that command apart must not parse in
        // the first place.
        for bad in [
            "bob.proj.brics extra_argument",
            "bob.proj.brics is_protected_user carol.proj.brics",
            "bob.proj.brics\tx",
            "bob.proj.brics\nx",
            "bob.proj.brics.extra",
            "bob.proj",
        ] {
            assert!(
                UserIdentifier::parse(bad).is_err(),
                "{:?} must not parse as a user identifier - it would change \
                 the meaning of every command built by formatting it in",
                bad
            );
        }
    }

    /// The same property for the destination half: `me` is this agent's own
    /// configured name, but the agent it delegates to is looked up at runtime,
    /// and the two are joined with a `.`.
    #[test]
    fn test_delegation_addresses_exactly_one_hop() {
        let job: Job<Hpc> = match Job::parse("cluster1.slurm get_limit proj.brics", false) {
            Ok(job) => job,
            Err(e) => unreachable!("job: {:?}", e),
        };

        let destination = job.destination();
        assert_eq!(destination.agents().len(), 2);
        assert_eq!(destination.first(), "cluster1");
        assert_eq!(destination.last(), "slurm");
    }
}
