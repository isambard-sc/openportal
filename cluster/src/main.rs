// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent;
use templemeads::agent::instance::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{
    AddProject, AddUser, GetProjects, GetUsers, RemoveProject, RemoveUser,
};
use templemeads::grammar::{
    PortalIdentifier, ProjectIdentifier, ProjectMapping, UserIdentifier, UserMapping,
};
use templemeads::job::{Envelope, Job};
use templemeads::Error;

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
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

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
            tracing::info!("Using the cluster runner");

            let me = envelope.recipient();
            let mut job = envelope.job();

            match job.instruction() {
                GetProjects(portal) => {
                    // get the list of projects from the cluster
                    tracing::info!("Getting list of projects for portal {}", portal);

                    let projects = get_projects(me.name(), &portal).await?;

                    job = job.completed(projects)?;
                },
                GetUsers(project) => {
                    // get the list of users from the cluster
                    tracing::info!("Getting list of users in project {}", project);

                    let users = get_accounts(me.name(), &project).await?;

                    job = job.completed(users)?;
                },
                AddProject(project) => {
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

                    job = job.completed(mapping)?;
                },
                RemoveProject(project) => {
                    // remove the project from the cluster
                    let mapping = remove_project_from_cluster(me.name(), &project).await?;
                    job = job.completed(mapping)?;
                },
                AddUser(user) => {
                    // add the user to the cluster
                    let mapping = match add_user_to_cluster(me.name(), &user).await {
                        Ok(mapping) => mapping,
                        Err(e) => {
                            // we cannot leave a dangling user account,
                            // so we need to remove the user from FreeIPA
                            tracing::error!("Error adding user {} to cluster: {:?}", user, e);

                            match remove_user_from_cluster(me.name(), &user).await {
                                Ok(_) => tracing::info!("Removed partially added user {}", user),
                                Err(e) => tracing::error!("Failed to remove partially added user {}: {:?}", user, e)
                            }

                            return Err(e);
                        }
                    };

                    job = job.completed(mapping)?;
                }
                RemoveUser(user) => {
                    // remove the user from the cluster
                    let mapping = remove_user_from_cluster(me.name(), &user).await?;
                    job = job.completed(mapping)?;
                }
                _ => {
                    tracing::error!("Unknown instruction: {:?}", job.instruction());
                    return Err(Error::UnknownInstruction(
                        format!("Unknown instruction: {:?}", job.instruction()).to_string(),
                    ));
                }
            }

            Ok(job)
        }
    }

    // run the agent
    run(config, cluster_runner).await?;

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

    let mapping = remove_project(me, project).await?;

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
    match agent::account(30).await {
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
    match agent::account(30).await {
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
    match agent::account(30).await {
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
    match agent::account(30).await {
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
    match agent::account(30).await {
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
    match agent::account(30).await {
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

async fn create_project_directories(me: &str, mapping: &ProjectMapping) -> Result<String, Error> {
    // find the Filesystem agent
    match agent::filesystem(30).await {
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
    match agent::filesystem(30).await {
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
    match agent::filesystem(30).await {
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
    match agent::filesystem(30).await {
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
    match agent::account(30).await {
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
    match agent::scheduler(30).await {
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
    match agent::scheduler(30).await {
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
    match agent::scheduler(30).await {
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
    match agent::scheduler(30).await {
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
