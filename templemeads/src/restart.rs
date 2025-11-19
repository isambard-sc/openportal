// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Restart functionality for agents
//!
//! This module provides functions for handling agent restart requests.

use crate::agent;
use crate::command::Command;

///
/// Handle a restart request from another agent
///
/// This function:
/// - Checks if this agent is the destination for the restart
/// - If destination matches, performs the restart (soft or hard)
/// - If destination doesn't match, forwards to the next peer in the path
///
/// Fire-and-forget: No acknowledgment is sent back to the requester
///
/// Parameters:
/// - `sender`: The agent that requested the restart
/// - `zone`: The zone of the sender
/// - `restart_type`: Type of restart ("soft", "hard", etc.)
/// - `destination`: Dot-separated path (e.g., "brics.aip2.clusters"), empty means restart self
///
pub async fn handle_restart_request(
    sender: &str,
    restart_type: &str,
    destination: &str,
) -> Result<(), anyhow::Error> {
    let my_name = agent::name().await;
    let my_type = agent::my_agent_type().await;

    // Security: Portals must not accept restart requests from other portals
    // to prevent cross-site control
    if my_type == agent::Type::Portal {
        // Check all peers to see if the sender is a portal
        let all_peers = agent::all_peers().await;
        if let Some(sender_peer) = all_peers.iter().find(|p| p.name() == sender) {
            if let Some(sender_type) = agent::agent_type(sender_peer).await {
                if sender_type == agent::Type::Portal {
                    tracing::warn!(
                        "Ignoring restart request from portal {} - portals do not restart other portals",
                        sender
                    );
                    return Ok(());
                }
            }
        }
    }

    // Parse the destination path
    let destination_parts: Vec<&str> = if destination.is_empty() {
        vec![]
    } else {
        destination.split('.').collect()
    };

    // Check if we are the target for this restart
    let is_target = if destination_parts.is_empty() {
        // Empty destination means restart the agent that received the request
        true
    } else {
        // Check if there are no more parts in the path (we're the final destination)
        destination_parts.len() == 1 && destination_parts[0] == my_name
    };

    if is_target {
        // We are the target - perform the restart
        tracing::warn!(
            "Received restart command from {} (type: {}) - this agent is the target",
            sender,
            restart_type
        );

        match restart_type {
            "soft" => {
                tracing::info!("Performing soft restart - disconnecting all peers");
                // TODO: Implement soft restart (disconnect and reconnect networking)
                // For now, just log that this would happen
                tracing::warn!(
                    "Soft restart not yet implemented - would disconnect/reconnect all peers"
                );
                Ok(())
            }
            "hard" => {
                tracing::warn!("Performing hard restart - terminating process");
                // Exit the process - supervisor should restart it
                std::process::exit(0);
            }
            _ => {
                tracing::error!("Unknown restart type: {}", restart_type);
                Err(anyhow::anyhow!("Unknown restart type: {}", restart_type))
            }
        }
    } else {
        // Check if this agent is allowed to forward restart requests
        // Leaf nodes (like FreeIPA or Filesystem) have cascade_health=false and should not forward
        if !agent::should_cascade_health().await {
            tracing::warn!(
                "Cannot forward restart request - this agent is a leaf node (cascade disabled)"
            );
            return Err(anyhow::anyhow!(
                "Leaf node agents cannot forward restart requests"
            ));
        }

        // We need to forward the restart to the next peer in the path
        let next_peer_name = destination_parts[0];
        let remaining_path = destination_parts[1..].join(".");

        tracing::info!(
            "Forwarding restart request from {} to {} (remaining path: {})",
            sender,
            next_peer_name,
            remaining_path
        );

        // Find the peer to forward to
        let all_peers = agent::all_peers().await;
        if let Some(next_peer) = all_peers.iter().find(|p| p.name() == next_peer_name) {
            // Security: If we're a portal, don't forward to other portals
            if my_type == agent::Type::Portal {
                if let Some(peer_type) = agent::agent_type(next_peer).await {
                    if peer_type == agent::Type::Portal {
                        tracing::error!(
                            "Cannot forward restart to portal {} - portals do not restart other portals",
                            next_peer_name
                        );
                        return Err(anyhow::anyhow!(
                            "Portals cannot forward restart requests to other portals"
                        ));
                    }
                }
            }

            // Forward the restart command with the updated destination
            let restart_cmd = Command::restart(restart_type, &remaining_path);
            restart_cmd.send_to(next_peer).await?;

            tracing::debug!("Forwarded restart to {}", next_peer_name);
            Ok(())
        } else {
            tracing::error!("Cannot find peer {} to forward restart to", next_peer_name);
            Err(anyhow::anyhow!(
                "Cannot find peer {} in destination path",
                next_peer_name
            ))
        }
    }
}
