// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent;
use templemeads::agent::instance::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{
    AddProject, AddUser, GetHomeDir, GetLimit, GetLocalHomeDir, GetLocalProjectDirs,
    GetProjectDirs, GetProjectMapping, GetProjects, GetUsageReport, GetUsageReports,
    GetUserMapping, GetUsers, IsProtectedUser, RemoveProject, RemoveUser, SetLimit,
};
use templemeads::grammar::{
    DateRange, PortalIdentifier, ProjectIdentifier, ProjectMapping, UserIdentifier, UserMapping,
};
use templemeads::job::{Envelope, Job};
use templemeads::usagereport::{ProjectUsageReport, Usage, UsageReport};
use templemeads::Error;

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
                    tracing::info!("Getting list of projects for portal {}", portal);

                    let projects = get_projects(me.name(), &portal).await?;

                    job.completed(projects)
                },
                GetUsers(project) => {
                    // get the list of users from the cluster
                    tracing::info!("Getting list of users in project {}", project);

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

                    // add the project to the cluster
                    let mapping = match add_project_to_cluster(me.name(), &project).await {
                        Ok(mapping) => mapping,
                        Err(e) => {
                            // we cannot leave a dangling project group,
                            // so we need to remove the project from FreeIPA
                            tracing::error!("Error adding project {} to cluster: {:?}", project, e);

                            match remove_project_from_cluster(me.name(), &project).await {
                                Ok(_) => tracing::info!("Removed partially added project {}", project),
                                Err(e) => tracing::error!("Failed to remove partially added project {}: {:?}", project, e)
                            }

                            return Err(e);
                        }
                    };

                    job.completed(mapping)
                },
                RemoveProject(project) => {
                    assert_agents_connected().await?;

                    // remove the project from the cluster
                    let mapping = remove_project_from_cluster(me.name(), &project).await?;
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

                                    match remove_account(me.name(), &user).await {
                                        Ok(_) => tracing::info!("Removed partially added user {}", user),
                                        Err(e) => tracing::error!("Failed to remove partially added user {}: {:?}", user, e)
                                    }

                                    return Err(e);
                                }
                                else {
                                    tracing::warn!("Error adding user {} to cluster: {:?}. Trying again...", user, e);
                                }
                            }
                        }
                    };

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
                    job.completed(mapping)
                }
                IsProtectedUser(user) => {
                    let is_protected = is_protected_user(me.name(), &user).await?;
                    job.completed(is_protected)
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
                GetLimit(project) => {
                    let limit = get_project_limit(me.name(), &project).await?;
                    job.completed(limit)
                }
                SetLimit(project, limit) => {
                    let limit = set_project_limit(me.name(), &project, limit).await?;
                    job.completed(limit)
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
                GetLocalHomeDir(mapping) => {
                    let homedir = get_home_dir(me.name(), &mapping).await?;
                    job.completed(homedir)
                }
                GetLocalProjectDirs(mapping) => {
                    let dirs = get_project_dirs(me.name(), &mapping).await?;
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
    run(config, cluster_runner).await?;

    Ok(())
}

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

async fn add_project_to_cluster(
    me: &str,
    project: &ProjectIdentifier,
) -> Result<ProjectMapping, Error> {
    tracing::info!("Adding project to cluster: {}", project);
    let mapping = create_project(me, project).await?;

    // now create the project directories
    create_project_directories(me, &mapping).await?;

    // and finally add the project to the job scheduler
    add_project_to_scheduler(me, project, &mapping).await?;

    Ok(mapping)
}

async fn remove_project_from_cluster(
    me: &str,
    project: &ProjectIdentifier,
) -> Result<ProjectMapping, Error> {
    tracing::info!("Removing project from cluster: {}", project);

    // remove the users
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

    match delete_project_directories(me, &mapping).await {
        Ok(_) => {
            tracing::info!("Project directories removed: {:?}", mapping);
        }
        Err(e) => {
            tracing::error!(
                "Error removing directories for project {}: {:?}",
                mapping,
                e
            );
        }
    }

    match remove_project_from_scheduler(me, project, &mapping).await {
        Ok(_) => {
            tracing::info!("Project removed from scheduler: {:?}", mapping);
        }
        Err(e) => {
            tracing::error!("Error removing project from scheduler {}: {:?}", mapping, e);
        }
    }

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

    let mapping = create_account(me, user).await?;

    // now create their home directories
    let homedir = create_user_directories(me, &mapping).await?;

    // update the home directory in the account
    update_homedir(me, user, &homedir).await?;

    // and finally add the user to the job scheduler
    add_user_to_scheduler(me, user, &mapping).await?;

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

    let mapping = remove_account(me, user).await?;

    match delete_user_directories(me, &mapping).await {
        Ok(_) => {
            tracing::info!("User directories removed: {:?}", mapping);
        }
        Err(e) => {
            tracing::error!("Error removing directories for user {}: {:?}", mapping, e);
        }
    }

    match remove_user_from_scheduler(me, user, &mapping).await {
        Ok(_) => {
            tracing::info!("User removed from scheduler: {:?}", mapping);
        }
        Err(e) => {
            tracing::error!("Error removing user from scheduler {}: {:?}", mapping, e);
        }
    }

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
                    tracing::info!("Projects retrieved from account agent: {:?}", projects);
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

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<Vec<UserMapping>>()?;

            match result {
                Some(users) => {
                    tracing::info!("Users retrieved from account agent: {:?}", users);
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
                    tracing::info!(
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

async fn create_project_directories(me: &str, mapping: &ProjectMapping) -> Result<String, Error> {
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

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<String>()?;

            match result {
                Some(homedir) => {
                    tracing::info!("Directories created for project: {:?}", mapping);
                    Ok(homedir)
                }
                None => {
                    tracing::error!("Error creating the project directories: {:?}", job);
                    Err(Error::Call(
                        format!("Error creating the project directories: {:?}", job).to_string(),
                    ))
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

            // Wait for the add_job to complete
            job.wait().await?;

            if job.is_error() {
                tracing::error!("Error removing the project directories: {:?}", job);
                Err(Error::Call(
                    format!("Error removing the project directories: {:?}", job).to_string(),
                ))
            } else {
                tracing::info!("Directories removed for project: {:?}", mapping);
                Ok(())
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

async fn create_user_directories(me: &str, mapping: &UserMapping) -> Result<String, Error> {
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

            // Wait for the add_job to complete
            let result = job.wait().await?.result::<String>()?;

            match result {
                Some(homedir) => {
                    tracing::info!("Directories created for user: {:?}", mapping);
                    Ok(homedir)
                }
                None => {
                    tracing::error!("Error creating the user's directories: {:?}", job);
                    Err(Error::Call(
                        format!("Error creating the user's directories: {:?}", job).to_string(),
                    ))
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

            // Wait for the add_job to complete
            job.wait().await?;

            if job.is_error() {
                tracing::error!("Error removing the user's directories: {:?}", job);
                Err(Error::Call(
                    format!("Error removing the user's directories: {:?}", job).to_string(),
                ))
            } else {
                tracing::info!("Directories removed for user: {:?}", mapping);
                Ok(())
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

            // Wait for the add_job to complete
            job.wait().await?;

            if job.is_error() {
                tracing::error!("Error adding the project to the scheduler: {:?}", job);
                Err(Error::Call(
                    format!("Error adding the project to the scheduler: {:?}", job).to_string(),
                ))
            } else {
                tracing::info!("Project {} added to scheduler", project);
                Ok(())
            }
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

            // Wait for the add_job to complete
            job.wait().await?;

            if job.is_error() {
                tracing::error!("Error removing the project from the scheduler: {:?}", job);
                Err(Error::Call(
                    format!("Error removing the project from the scheduler: {:?}", job).to_string(),
                ))
            } else {
                tracing::info!("Project {} removed from scheduler", project);
                Ok(())
            }
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

            // Wait for the add_job to complete
            job.wait().await?;

            if job.is_error() {
                tracing::error!("Error adding the user to the scheduler: {:?}", job);
                Err(Error::Call(
                    format!("Error adding the user to the scheduler: {:?}", job).to_string(),
                ))
            } else {
                tracing::info!("User {} added to scheduler", user);
                Ok(())
            }
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

            // Wait for the add_job to complete
            job.wait().await?;

            if job.is_error() {
                tracing::error!("Error removing the user from the scheduler: {:?}", job);
                Err(Error::Call(
                    format!("Error removing the user from the scheduler: {:?}", job).to_string(),
                ))
            } else {
                tracing::info!("User {} removed from scheduler", user);
                Ok(())
            }
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
                    tracing::info!("User is protected: {}", is_protected);
                    Ok(is_protected)
                }
                None => {
                    tracing::error!("No user found?");
                    Err(Error::MissingUser(format!("Could not find user {}", user)))
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
                    tracing::info!("User homedir retrieved: {:?}", homedir);
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
                    tracing::info!("Project directories retrieved: {:?}", dirs);
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
