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
use templemeads::grammar::{ProjectIdentifier, ProjectMapping, UserIdentifier, UserMapping};
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
            let sender = envelope.sender();
            let mut job = envelope.job();

            match job.instruction() {
                GetProjects() => {
                    // get the list of projects from the cluster
                    tracing::info!("Getting list of projects");
                    job = job.completed("Projects retrieved".to_string())?;
                },
                GetUsers(project) => {
                    // get the list of users from the cluster
                    tracing::info!("Getting list of users in project {}", project);
                    job = job.completed("Users retrieved".to_string())?;
                },
                AddProject(project) => {
                    // add the project to the cluster
                    tracing::info!("Adding project to cluster: {}", project);
                    let mapping = create_project(me.name(), &project).await?;

                    job = job.running(Some("Step 1/3: Project created".to_string()))?;
                    job = job.update(&sender).await?;

                    // now create the project directories
                    create_project_directories(me.name(), &mapping).await?;

                    job = job.running(Some("Step 2/3: Directories created".to_string()))?;
                    job = job.update(&sender).await?;

                    // and finally add the project to the job scheduler
                    add_project_to_scheduler(me.name(), &project, &mapping).await?;

                    job = job.completed(mapping)?;
                },
                RemoveProject(project) => {
                    // remove the project from the cluster
                    tracing::info!("Removing project from cluster: {}", project);
                    job = job.completed("Project removed".to_string())?;
                },
                AddUser(user) => {
                    // add the user to the cluster
                    tracing::info!("Adding user to cluster: {}", user);
                    let mapping = create_account(me.name(), &user).await?;

                    job = job.running(Some("Step 1/3: Account created".to_string()))?;
                    job = job.update(&sender).await?;

                    // now create their home directories
                    let homedir = create_user_directories(me.name(), &mapping).await?;

                    job = job.running(Some("Step 2/3: Directories created".to_string()))?;
                    job = job.update(&sender).await?;

                    // update the home directory in the account
                    update_homedir(me.name(), &user, &homedir).await?;

                    // and finally add the user to the job scheduler
                    add_user_to_scheduler(me.name(), &user, &mapping).await?;

                    job = job.completed(mapping)?;
                }
                RemoveUser(user) => {
                    // remove the user from the cluster
                    tracing::info!("Removing user from cluster: {}", user);
                    job = job.completed("User removed".to_string())?;
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
            let result = job.wait().await?.result::<String>()?;

            match result {
                Some(_) => {
                    tracing::info!("Project {} added to scheduler", project);
                    Ok(())
                }
                None => {
                    tracing::error!("Error adding the project to the scheduler: {:?}", project);
                    Err(Error::Call(
                        format!("Error adding the project to the scheduler: {:?}", project)
                            .to_string(),
                    ))
                }
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
            let result = job.wait().await?.result::<String>()?;

            match result {
                Some(_) => {
                    tracing::info!("User {} added to scheduler", user);
                    Ok(())
                }
                None => {
                    tracing::error!("Error adding the user to the scheduler: {:?}", job);
                    Err(Error::Call(
                        format!("Error adding the user to the scheduler: {:?}", job).to_string(),
                    ))
                }
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
