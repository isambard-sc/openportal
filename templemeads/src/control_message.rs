// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Type as AgentType;
use crate::command::Command;
use crate::error::Error;

use anyhow::Result;
use paddington::command::Command as ControlCommand;

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
