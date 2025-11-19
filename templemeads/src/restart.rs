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
    } else if destination_parts.len() == 1 {
        // Extract just the name part (before any @zone) and check if it matches
        let target_spec = destination_parts[0];
        let target_name = if target_spec.contains('@') {
            target_spec.split('@').next().unwrap_or(target_spec)
        } else {
            target_spec
        };
        target_name == my_name
    } else {
        false
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
        // Parse the next hop, which may include a zone specifier (name@zone)
        let next_hop = destination_parts[0];
        let (next_peer_name, zone_filter) = if next_hop.contains('@') {
            let parts: Vec<&str> = next_hop.split('@').collect();
            if parts.len() == 2 {
                (parts[0], Some(parts[1]))
            } else {
                tracing::error!("Invalid format for agent specification: {}", next_hop);
                return Err(anyhow::anyhow!(
                    "Invalid format '{}' - use 'name' or 'name@zone'",
                    next_hop
                ));
            }
        } else {
            (next_hop, None)
        };

        let remaining_path = destination_parts[1..].join(".");

        tracing::info!(
            "Forwarding restart request from {} to {} (zone: {}, remaining path: {})",
            sender,
            next_peer_name,
            zone_filter.unwrap_or("any"),
            remaining_path
        );

        // Find the peer to forward to
        let all_peers = agent::all_peers().await;
        let next_peer = if let Some(required_zone) = zone_filter {
            // Find peer with matching name AND zone
            all_peers
                .iter()
                .find(|p| p.name() == next_peer_name && p.zone() == required_zone)
        } else {
            // Find first peer with matching name (any zone)
            all_peers.iter().find(|p| p.name() == next_peer_name)
        };

        if let Some(next_peer) = next_peer {
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

            tracing::debug!(
                "Forwarded restart to {} in zone {}",
                next_peer_name,
                next_peer.zone()
            );
            Ok(())
        } else {
            let error_msg = if let Some(zone) = zone_filter {
                format!(
                    "Cannot find peer {} in zone {} to forward restart to",
                    next_peer_name, zone
                )
            } else {
                format!("Cannot find peer {} to forward restart to", next_peer_name)
            };
            tracing::error!("{}", error_msg);
            Err(anyhow::anyhow!("{}", error_msg))
        }
    }
}
