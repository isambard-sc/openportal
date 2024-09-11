// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;
use anyhow::Result;
use thiserror::Error;

use templemeads::agent;
use templemeads::agent::instance::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{AddUser, RemoveUser};
use templemeads::job::{Envelope, Error as JobError, Job};
use templemeads::runnable::Error as RunnableError;
use templemeads::state;

///
/// Main function for the slurm cluster instance agent
///
/// This purpose of this agent is to manage an individual instance
/// of a slurm batch cluster. It will manage the lifecycle of
/// users and projects on the cluster.
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("slurm".to_owned()),
        Some(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("slurm-config.toml"),
        ),
        Some("ws://localhost:8046".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8046),
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

    // run the agent
    run(config, slurm_runner).await?;

    Ok(())
}

async_runnable! {
    ///
    /// Runnable function that will be called when a job is received
    /// by the agent
    ///
    pub async fn slurm_runner(envelope: Envelope) -> Result<Job, RunnableError>
    {
        match runner(&envelope).await {
            Ok(job) => Ok(job),
            Err(e) => {
                tracing::error!("Error running job: {:?}", e);
                Err(e.into())
            }
        }
    }
}

///
/// Runnable function that will be called when a job is received
/// by the agent
///
async fn runner(envelope: &Envelope) -> Result<Job, Error> {
    tracing::info!("Using the slurm runner");

    let mut job = envelope.job();

    match job.instruction() {
        AddUser(user) => {
            // add the user to the slurm cluster
            tracing::info!("Adding user to slurm cluster: {}", user);

            // find the Account agent
            match agent::account().await {
                Some(account) => {
                    // send the add_job to the account agent
                    let add_job = Job::parse(&format!(
                        "{}.{} add_user {}",
                        envelope.recipient(),
                        account,
                        user
                    ))?
                    .put(&account)
                    .await?;

                    // update the submitted job we are processing to say that the account is being created
                    job = job
                        .running(Some("Account being created".to_owned()))?
                        .updated()
                        .await?;

                    // Wait for the add_job to complete, then set our job as complete
                    match add_job.wait().await?.result::<String>() {
                        Ok(r) => {
                            job = job.completed(&r)?;
                        }
                        Err(e) => {
                            job = job.errored(&format!("Error adding user to account: {:?}", e))?;
                        }
                    }

                    // log the result
                    if job.is_error() {
                        tracing::error!(
                            "Not adding user {} because of error {:?}",
                            user,
                            job.error_message()
                        );
                    }

                    // communicate the change
                    job = job.updated().await?;

                    tracing::info!("User added to slurm cluster: {}", user);
                }
                None => {
                    tracing::error!("No account agent found");
                    return Err(Error::NoAccount(
                        "Cannot run the job because there is no account agent".to_string(),
                    ));
                }
            }
        }
        RemoveUser(user) => {
            // remove the user from the slurm cluster
            tracing::info!("Removing user from slurm cluster: {}", user);
            job = job.completed("User removed")?;
        }
        _ => {
            job = job.execute().await?;
        }
    }

    Ok(job)
}

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    Any(#[from] AnyError),

    #[error("{0}")]
    Job(#[from] JobError),

    #[error("{0}")]
    State(#[from] state::Error),

    #[error("{0}")]
    NoAccount(String),
}

/// convert above error into a RunnableError
impl From<Error> for RunnableError {
    fn from(e: Error) -> RunnableError {
        RunnableError::Any(e.into())
    }
}
