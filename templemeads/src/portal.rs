// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Type as AgentType;
use crate::agent_core::Config;
use crate::handler::{process_message, set_service_details};
use anyhow::{Error as AnyError, Result};
use thiserror::Error;

///
/// Run the agent service
///
pub async fn run(config: Config) -> Result<(), AnyError> {
    if config.service.name.is_empty() {
        return Err(Error::Misconfigured("Service name is empty".to_string()).into());
    }

    if config.agent != AgentType::Portal {
        return Err(Error::Misconfigured("Service agent is not a Portal".to_string()).into());
    }

    // pass the service details onto the handler
    set_service_details(&config.service.name, &config.agent, None).await?;

    // run the Provider OpenPortal agent
    paddington::set_handler(process_message).await?;
    paddington::run(config.service).await?;

    Ok(())
}

/// Errors

#[derive(Error, Debug)]
enum Error {
    #[error("{0}")]
    Any(#[from] AnyError),

    #[error("{0}")]
    Misconfigured(String),
}
