// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use std::path::PathBuf;
use templemeads::agent::instance::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{AddUser, RemoveUser};
use templemeads::job::{Envelope, Job};
use templemeads::Error;

#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    templemeads::config::initialise_tracing();

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("example-cluster".to_owned()),
        Some(PathBuf::from("example-cluster.toml")),
        Some("ws://localhost:8091".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8091),
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

    // run the portal agent
    run(config, cluster_runner).await?;

    Ok(())
}

async_runnable! {
    ///
    /// Runnable function that will be called when a job is received
    /// by the cluster agent
    ///
    pub async fn cluster_runner(envelope: Envelope) -> Result<Job, Error>
    {
        let mut job = envelope.job();

        match job.instruction() {
            AddUser(user) => {
                // add the user to the cluster
                tracing::info!("Adding {} to cluster", user);

                tracing::info!("Here we would implement the business logic to add the user to the cluster");

                job = job.completed("account created".to_string())?;
            }
            RemoveUser(user) => {
                // remove the user from the cluster
                tracing::info!("Removing {} from the cluster", user);

                tracing::info!("Here we would implement the business logic to remove the user from the cluster");

                if user.project() == "admin" {
                    job = job.errored(&format!("You are not allowed to remove the account for {:?}",
                                      user.username()))?;
                } else {
                    job = job.completed("account removed".to_string())?;
                }
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
