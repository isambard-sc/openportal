// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Type as AgentType;
use crate::agent_core::Config;
use crate::error::Error;

use crate::handler::{process_message, set_my_service_details};
use anyhow::Result;

///
/// Run the agent service
///
pub async fn run(config: Config) -> Result<(), Error> {
    if config.service().name().is_empty() {
        return Err(Error::Misconfigured("Service name is empty".to_string()));
    }

    if config.agent() != AgentType::Platform {
        return Err(Error::Misconfigured(
            "Service agent is not a Platform".to_string(),
        ));
    }

    // pass the service details onto the handler
    set_my_service_details(&config.service().name(), &config.agent(), None).await?;

    // run the Provider OpenPortal agent
    paddington::set_handler(process_message).await?;
    paddington::run(config.service()).await?;

    Ok(())
}
