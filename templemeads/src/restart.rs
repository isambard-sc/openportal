// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Restart functionality for agents
//!
//! This module provides functions for handling agent restart requests.

use crate::agent::{self, Peer};
use crate::command::Command;

///
/// Handle a restart request from another agent
///
/// This function:
/// - Logs the restart request
/// - Sends an acknowledgment back to the requester
/// - Terminates the process (relying on supervisor to restart)
///
/// Parameters:
/// - `sender`: The agent that requested the restart
/// - `zone`: The zone of the sender
///
pub async fn handle_restart_request(sender: &str, zone: &str) -> Result<(), anyhow::Error> {
    tracing::warn!("Received restart command from {}", sender);

    let agent_name = agent::name().await;
    let ack = Command::restart_ack(
        &agent_name,
        "Restart acknowledged - agent will terminate and rely on supervisor to restart",
    );

    // Send acknowledgment before terminating
    if let Err(e) = ack.send_to(&Peer::new(sender, zone)).await {
        tracing::error!("Failed to send restart acknowledgment: {}", e);
        // Continue with restart even if acknowledgment fails
    }

    // Small delay to ensure acknowledgment is sent
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Exit the process - supervisor should restart it
    tracing::warn!("Terminating process for restart...");
    std::process::exit(0);
}

///
/// Handle a restart acknowledgment from another agent
///
/// Parameters:
/// - `agent`: The agent that acknowledged the restart
/// - `message`: The acknowledgment message
///
pub async fn handle_restart_ack(agent: &str, message: &str) {
    tracing::info!("Restart acknowledged by {}: {}", agent, message);
}
