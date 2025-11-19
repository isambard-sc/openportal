// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Restart functionality for agents
//!
//! This module provides functions for handling agent restart requests.

// No imports needed - function just logs and exits

///
/// Handle a restart request from another agent
///
/// This function:
/// - Logs the restart request
/// - Terminates the process (relying on supervisor to restart)
///
/// Fire-and-forget: No acknowledgment is sent back to the requester
///
/// Parameters:
/// - `sender`: The agent that requested the restart
/// - `_zone`: The zone of the sender (unused)
///
pub async fn handle_restart_request(sender: &str, _zone: &str) -> Result<(), anyhow::Error> {
    tracing::warn!("Received restart command from {} - terminating process", sender);

    // Exit the process - supervisor should restart it
    std::process::exit(0);
}
