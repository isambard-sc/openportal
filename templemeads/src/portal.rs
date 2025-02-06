// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::agent::Type::{Bridge, Portal};
use crate::agent_core::Config;
use crate::error::Error;
use crate::grammar::Instruction::Submit;
use crate::job::{Envelope, Job};

use crate::handler::{process_message, set_my_service_details};
use anyhow::Result;

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

        // get information about the agent that sent this job
        // - double check that they are a bridge agent
        // (these are the only agent type that should be submitting
        //  jobs to the portal)
        match agent::agent_type(&envelope.sender()).await {
            Some(Bridge) => {}
            _ => {
                return Err(Error::InvalidInstruction(
                    format!("Invalid instruction: {}. Only bridge agents can submit instructions to the portal", job.instruction()),
                ));
            }
        }

        let sender = envelope.sender();

        match job.instruction() {
            Submit(destination, instruction) => {
                // This is a job that should have been received from
                // the bridge, and which is to be interpreted and passed
                // south-bound to the agents for processing
                tracing::info!("{} : {}", destination, instruction);
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

                Ok(job)
            }
            _ => {
                Err(Error::InvalidInstruction(
                    format!("Invalid instruction: {}. Portals only support 'submit'", job.instruction()),
                ))
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
