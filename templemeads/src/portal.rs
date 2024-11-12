// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Type as AgentType;
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
        let job = envelope.job();

        match job.instruction() {
            Submit(destination, instruction) => {
                // This is a job that should have been received from
                // the bridge, and which is to be interpreted and passed
                // south-bound to the agents for processing
                tracing::info!("Received instruction: {:?}", instruction);
                tracing::info!("This is for destination: {:?}", destination);
                tracing::info!("This was from {:?}", envelope);

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

    if config.agent() != AgentType::Portal {
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
