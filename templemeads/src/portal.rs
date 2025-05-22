// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::agent::Type::{Bridge, Portal};
use crate::agent_core::Config;
use crate::error::Error;
use crate::grammar::Instruction::{CreateProject, Submit, UpdateProject};
use crate::job::{Envelope, Job};

use crate::handler::{process_message, set_my_service_details};
use anyhow::Result;

///
/// Return the zone that should be used for a portal to portal
/// connection where the sender has the ability to send jobs
/// to the recipient (but the recipient cannot send jobs to the sender)
///
fn portal_to_portal_zone(sender: &agent::Peer, recipient: &agent::Peer) -> String {
    format!("{}>{}", sender.name(), recipient.name())
}

///
/// Return whether or not the sender has permission to send jobs
/// to the recipient, assuming they are both portals
///
fn portal_to_portal_allowed(sender: &agent::Peer, recipient: &agent::Peer) -> bool {
    (sender.zone() == recipient.zone())
        && (sender.zone() == portal_to_portal_zone(sender, recipient))
}

crate::async_runnable! {
///
/// Runnable function that will be called when a job is received
/// by the portal. This creates a firewall between the agents
/// south of the portal (which e.g. actually create accounts etc)
/// the agents north of the portal (which e.g. create or query
/// allocations) and the bridge agent to the east/west of the portal,
/// which connects to the graphical portal user interface.
///
pub async fn portal_runner(envelope: Envelope) -> Result<Job, Error>
{
    let mut job = envelope.job();

    let mut agent_is_bridge = false;
    let mut agent_is_portal = false;

    // Get information about the agent that sent this job
    // The only agents that can send jobs to a portal are
    // bridge agents, and other portal agents that have
    // expressly be configured to be given permission.
    // This permission is based on the zone of the portal to portal
    // connection
    match agent::agent_type(&envelope.sender()).await {
        Some(Bridge) => {
            agent_is_bridge = true;
        }
        Some(Portal) => {
            if !portal_to_portal_allowed(&envelope.sender(), &envelope.recipient()) {
                return Err(Error::InvalidInstruction(
                    format!("Invalid instruction: {}. Portal {} is not allowed to send jobs to portal {}", job.instruction(), envelope.sender(), envelope.recipient()),
                ));
            }
            agent_is_portal = true;
        }
        _ => {
            return Err(Error::InvalidInstruction(
                format!("Invalid instruction: {}. Only bridge agents can submit instructions to the portal", job.instruction()),
            ));
        }
    }

    let sender = envelope.sender();

    // match instructions that can only be sent by bridge agents
    match job.instruction() {
        Submit(destination, instruction) => {
            if !agent_is_bridge {
                return Err(Error::InvalidInstruction(
                    format!("Invalid instruction: {}. Only bridge agents can submit instructions to the portal", job.instruction()),
                ));
            }

            // This is a job that should have been received from
            // the bridge, and which is to be interpreted and passed
            // south-bound to the agents for processing
            tracing::debug!("{} : {}", destination, instruction);
            tracing::debug!("This was from {:?}", envelope);

            if destination.agents().len() < 2 {
                tracing::error!("Invalid instruction: {}. Destination must have at least two agents", job.instruction());
                return Err(Error::InvalidInstruction(
                    format!("Invalid instruction: {}. Destination must have at least two agents", job.instruction()),
                ));
            }

            // the first agent in the destination is the agent should be this portal
            let first_agent = destination.agents()[0].clone();

            if first_agent != envelope.recipient().name() {
                tracing::error!("Invalid instruction: {}. First agent in destination should be this portal ({})", job.instruction(), envelope.recipient().name());
                return Err(Error::InvalidInstruction(
                    format!("Invalid instruction: {}. First agent in destination should be this portal ({})",
                                job.instruction(),
                                envelope.recipient().name())
                ));
            }

            // who is next in line to receive this job? - find it, and its zone
            let next_agent = agent::find(&destination.agents()[1], 5).await.ok_or_else(|| {
                tracing::error!("Invalid instruction: {}. Cannot find next agent in destination {}", job.instruction(), destination);
                Error::InvalidInstruction(
                    format!("Invalid instruction: {}. Cannot find next agent in destination {}",
                            job.instruction(), destination),
                )
            })?;

            // create the job and send it to the board for the next agent
            let southbound_job = Job::parse(&format!("{} {}", destination, instruction), true)?.put(&next_agent).await?;

            job = job.running(Some("Job registered - processing...".to_string()))?;
            job = job.update(&sender).await?;

            // Wait for the submitted job to complete
            let southbound_job = southbound_job.wait().await?;

            if southbound_job.is_expired() {
                tracing::error!("{} : {} : Error - job expired!", destination, instruction);
                job = job.errored("ExpirationError{{}}")?;
             } else if (southbound_job.is_error()) {
                if let Some(message) = southbound_job.error_message() {
                    tracing::error!("{} : {} : Error - {}", destination, instruction, message);
                    job = job.errored(&format!("RuntimeError{{{}}}", message))?;
                }
                else {
                    tracing::error!("{} : {} : Error - unknown error", destination, instruction);
                    job = job.errored("UnknownError{{}}")?;
                }
             }
             else {
                tracing::info!("{} : {} : Success", destination, instruction);
                job = job.copy_result_from(&southbound_job)?;
            }

            return Ok(job);
        }
        _ => {
            if !agent_is_portal {
                return Err(Error::InvalidInstruction(
                    format!("Invalid instruction: {}. Only portal agents can send instructions to the portal", job.instruction()),
                ));
            }
        }
    }

    // match instructions that can be sent by portal agents
    match job.instruction() {
        CreateProject(project, details) => {
            tracing::debug!("{} : {}", project, details);
            tracing::debug!("This was from {:?}", envelope);

            // do the work to create the project
            tracing::info!("Creating project {} with details {}", project, details);

            job = job.completed("Project created".to_string())?;

            return Ok(job);
        }
        UpdateProject(project, details) => {
            tracing::debug!("{} : {}", project, details);
            tracing::debug!("This was from {:?}", envelope);

            // do the work to update the project
            tracing::info!("Updating project {} with details {}", project, details);

            job = job.completed("Project updated".to_string())?;

            return Ok(job);
        }
        _ => {
            tracing::error!("Invalid instruction: {}. Only portal agents can send instructions to the portal", job.instruction());
            return Err(Error::InvalidInstruction(
                format!("Invalid instruction: {}. Only portal agents can send instructions to the portal", job.instruction()),
            ));
        }
    }
}
}

///
/// Run the agent service
///
pub async fn run(config: Config) -> Result<(), Error> {
    if config.service().name().is_empty() {
        return Err(Error::Misconfigured("Service name is empty".to_string()));
    }

    if config.agent() != Portal {
        return Err(Error::Misconfigured(
            "Service agent is not a Portal".to_string(),
        ));
    }

    // pass the service details onto the handler
    set_my_service_details(
        &config.service().name(),
        &config.agent(),
        Some(portal_runner),
    )
    .await?;

    // run the Provider OpenPortal agent
    paddington::set_handler(process_message).await?;
    paddington::run(config.service()).await?;

    Ok(())
}
