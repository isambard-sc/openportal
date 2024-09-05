// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Type as AgentType;
use crate::command::Command;
use anyhow::{Error as AnyError, Result};
use paddington::command::Command as ControlCommand;
use thiserror::Error;

pub async fn process_control_message(
    agent_type: &AgentType,
    command: ControlCommand,
) -> Result<(), Error> {
    match command {
        ControlCommand::Connected { agent } => {
            tracing::info!("Connected to agent: {}", agent);
            Command::register(agent_type).send_to(&agent).await?;
        }
        ControlCommand::Disconnected { agent } => {
            tracing::info!("Disconnected from agent: {}", agent);
        }
        ControlCommand::Error { error } => {
            tracing::error!("Received error: {}", error);
        }
    }

    Ok(())
}

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("Any error: {0}")]
    Any(#[from] AnyError),
}

impl From<Error> for paddington::Error {
    fn from(error: Error) -> paddington::Error {
        paddington::Error::Any(error.into())
    }
}
