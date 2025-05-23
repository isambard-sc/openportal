// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent;
use templemeads::agent::bridge::{process_args, run, Defaults};
use templemeads::agent::Type::Portal;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{CreateProject, UpdateProject};
use templemeads::grammar::{ProjectDetails, ProjectIdentifier, ProjectMapping};
use templemeads::job::{Envelope, Job};
use templemeads::Error;

///
/// Main function for the bridge application
///
/// The purpose of this application is to bridge between the user portal
/// (e.g. Waldur) and OpenPortal.
///
/// It does this by providing a "Client" agent in OpenPortal that can be
/// used to make requests over the OpenPortal protocol.
///
/// It also provides a web API that can be called by the user portal to
/// submit and get information about those requests. This API is designed
/// to be called via, e.g. the openportal Python client.
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    templemeads::config::initialise_tracing();

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("bridge".to_owned()),
        Some(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("bridge-config.toml"),
        ),
        Some("ws://localhost:8044".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8044),
        None,
        None,
        Some("http://localhost:3000".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(3000),
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
        pub async fn bridge_runner(envelope: Envelope) -> Result<Job, Error>
        {
            let job = envelope.job();

            // Get information about the agent that sent this job
            // The only agents that can send jobs to a portal are
            // bridge agents, and other portal agents that have
            // expressly be configured to be given permission.
            // This permission is based on the zone of the portal to portal
            // connection
            match agent::agent_type(&envelope.sender()).await {
                Some(Portal) => {}
                _ => {
                    return Err(Error::InvalidInstruction(
                        format!("Invalid instruction: {}. Only portal agents can submit instructions to a bridge", job.instruction()),
                    ));
                }
            }

            match job.instruction() {
                CreateProject(project, details) => {
                    // create a new project in the cluster
                    tracing::debug!("Creating project {} with details {:?}", project, details);
                    job.completed(create_project(&project, &details).await?)
                }
                UpdateProject(project, details) => {
                    // update the project in the cluster
                    tracing::debug!("Updating project {} with details {:?}", project, details);
                    job.completed(update_project(&project, &details).await?)
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

    // run the Bridge agent
    run(config, bridge_runner).await?;

    Ok(())
}

async fn create_project(
    project: &ProjectIdentifier,
    details: &ProjectDetails,
) -> Result<ProjectMapping, Error> {
    tracing::info!("Creating project {} with details {:?}", project, details);

    Err(Error::IncompleteCode(
        "Create project not implemented".to_string(),
    ))
}

async fn update_project(
    project: &ProjectIdentifier,
    details: &ProjectDetails,
) -> Result<ProjectMapping, Error> {
    tracing::info!("Updating project {} with details {:?}", project, details);

    Err(Error::IncompleteCode(
        "Update project not implemented".to_string(),
    ))
}
