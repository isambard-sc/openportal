// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Restart functionality for agents
//!
//! This module provides functions for handling agent restart requests.

use crate::agent;
use crate::command::Command;
use crate::diagnostics;
use crate::domain::Domain;

///
/// Perform a soft restart by disconnecting all peers and clearing boards
///
/// This function:
/// - Gets all connected peers
/// - Disconnects from each peer
/// - Clears all job boards (cancels in-flight jobs)
///
async fn perform_soft_restart<L: Domain>() -> Result<(), anyhow::Error> {
    // Acquire the RAII guard to block new connections
    // The guard will automatically clear the flag when this function exits (even on panic)
    let _guard = paddington::SoftRestartGuard::new();

    // Get all peers
    let all_peers = agent::real_peers().await;

    tracing::info!(
        "Soft restart: clearing job boards and disconnecting from {} peers",
        all_peers.len()
    );

    // STEP 1: Clear all boards and cancel in-flight jobs BEFORE disconnecting
    // This ensures that cancellation messages are sent back to peers before connections are severed
    tracing::info!("Soft restart: clearing all job boards and cancelling in-flight jobs");

    for peer in all_peers.iter() {
        match crate::state::get::<L>(peer).await {
            Ok(state) => {
                let board = state.board().await;

                // STEP 1: Get all jobs while holding the lock, then release it
                let all_jobs = {
                    let board = board.read().await;
                    let sync_state = board.sync_state();
                    sync_state.jobs().clone()
                };

                tracing::debug!(
                    "Clearing {} jobs from board for peer {}",
                    all_jobs.len(),
                    peer.name()
                );

                // STEP 2: Error jobs and send them back WITHOUT holding the lock
                // This prevents deadlock when job.update() tries to acquire locks
                let mut errored_jobs = Vec::new();
                for mut job in all_jobs {
                    if !job.is_finished() {
                        // Mark the job as errored due to restart
                        match job.errored("Agent soft restart - job cancelled") {
                            Ok(errored_job) => {
                                job = errored_job.clone();

                                // Send the errored job back to the peer
                                if let Err(e) = errored_job.update(peer).await {
                                    tracing::warn!(
                                        "Failed to send cancelled job {} back to {}: {}",
                                        job.id(),
                                        peer.name(),
                                        e
                                    );
                                } else {
                                    tracing::debug!(
                                        "Sent cancelled job {} back to {}",
                                        job.id(),
                                        peer.name()
                                    );
                                }

                                errored_jobs.push(job);
                            }
                            Err(e) => {
                                tracing::warn!("Failed to error job {}: {}", job.id(), e);
                                errored_jobs.push(job);
                            }
                        }
                    } else {
                        errored_jobs.push(job);
                    }
                }

                // STEP 3: Now acquire write lock to remove jobs from the board
                {
                    let mut board = board.write().await;

                    // Remove all jobs from the board
                    for job in errored_jobs {
                        if let Err(e) = board.remove(&job) {
                            tracing::warn!("Failed to remove job {} from board: {}", job.id(), e);
                        }
                    }

                    // Clear any queued commands
                    let _ = board.take_queued();
                }

                tracing::info!("Cleared board for peer {}", peer.name());
            }
            Err(e) => {
                tracing::warn!("Failed to get state for peer {}: {}", peer.name(), e);
            }
        }
    }

    // STEP 2: Clear diagnostics data
    tracing::info!("Soft restart: clearing diagnostics data");
    diagnostics::clear_diagnostics().await;

    // STEP 3: Disconnect from all peers AFTER cancelling jobs
    tracing::info!("Soft restart: disconnecting from all peers");

    for peer in all_peers.iter() {
        tracing::debug!(
            "Disconnecting from peer {} in zone {}",
            peer.name(),
            peer.zone()
        );

        match paddington::disconnect(peer.name(), peer.zone()).await {
            Ok(_) => {
                tracing::info!(
                    "Successfully disconnected from {} ({})",
                    peer.name(),
                    peer.zone()
                );
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to disconnect from {} ({}): {}",
                    peer.name(),
                    peer.zone(),
                    e
                );
                // Continue with other disconnections even if one fails
            }
        }
    }

    tracing::warn!("Soft restart complete - all peers disconnected and boards cleared");

    Ok(())
}

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
/// What to do with an inbound `Restart`.
#[derive(Debug, PartialEq, Eq)]
enum RestartDecision {
    /// This agent is the target - restart ourselves.
    RestartSelf,
    /// Relay it to the next hop in the destination path.
    Forward,
    /// Refuse it, with a reason for the log and the error returned to the sender.
    Refuse(&'static str),
}

/// Decide what an inbound `Restart` means, given who sent it, our own name, the
/// destination path, and whether this agent relays at all.
///
/// Split out from `handle_restart_request` so the two orderings that mattered can
/// be tested. Both were bugs, see
/// `docs/specifications/security-review-2.md` (finding R2):
///
///  * An **empty destination** meant "restart whoever received this", so a single
///    message with no destination killed the agent that received it, repeatably.
///    It is still legitimate when locally originated - the bridge's
///    `POST /restart` with an empty destination injects the command into its own
///    queue via `send_to(self)`, so `sender` is our own name - but a remote peer
///    must name us explicitly.
///  * The **leaf-node check ran after** the target check, so an agent that
///    refused to *forward* a restart would still kill *itself* on request from
///    the very peer it would not relay for. Authorization now precedes the
///    target decision.
fn decide_restart(
    sender: &str,
    my_name: &str,
    destination_parts: &[&str],
    may_cascade: bool,
) -> RestartDecision {
    let locally_originated = sender == my_name;

    let Some((target_spec, rest)) = destination_parts.split_first() else {
        return match locally_originated {
            true => RestartDecision::RestartSelf,
            false => RestartDecision::Refuse(
                "a restart request from a remote peer must name its target agent explicitly",
            ),
        };
    };

    // A hop may be written `name@zone`; only the name identifies the agent.
    let target_name = match target_spec.split_once('@') {
        Some((name, _zone)) => name,
        None => target_spec,
    };

    if rest.is_empty() && target_name == my_name {
        return RestartDecision::RestartSelf;
    }

    match may_cascade {
        true => RestartDecision::Forward,
        false => {
            RestartDecision::Refuse("this agent is a leaf node and cannot relay restart requests")
        }
    }
}

pub async fn handle_restart_request<L: Domain>(
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

    let destination_parts: Vec<&str> = if destination.is_empty() {
        vec![]
    } else {
        destination.split('.').collect()
    };

    let is_target = match decide_restart(
        sender,
        &my_name,
        &destination_parts,
        agent::should_cascade_health().await,
    ) {
        RestartDecision::RestartSelf => true,
        RestartDecision::Forward => false,
        RestartDecision::Refuse(reason) => {
            tracing::warn!("Ignoring restart request from {}: {}", sender, reason);
            return Err(anyhow::anyhow!("{}", reason));
        }
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
                tracing::warn!(
                    "Performing soft restart - disconnecting all peers and clearing boards"
                );
                match perform_soft_restart::<L>().await {
                    Ok(_) => {
                        tracing::info!("Soft restart completed successfully");
                        Ok(())
                    }
                    Err(e) => {
                        tracing::error!(
                            "Soft restart failed: {} - falling back to hard restart",
                            e
                        );
                        tracing::warn!("Performing hard restart due to soft restart failure - terminating process");
                        // Exit the process - supervisor should restart it
                        std::process::exit(1);
                    }
                }
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
        // (the leaf-node check happened before `is_target` above)

        // We need to forward the restart to the next peer in the path
        // Parse the next hop, which may include a zone specifier (name@zone)
        // `first()`/`get(1..)` rather than `[0]`/`[1..]`, so a malformed
        // path is handled rather than aborting the process - see
        // docs/specifications/security-review-2.md (finding R1).
        let next_hop = destination_parts.first().copied().unwrap_or_default();
        let (next_peer_name, zone_filter) = if next_hop.contains('@') {
            let parts: Vec<&str> = next_hop.split('@').collect();
            if parts.len() == 2 {
                (
                    parts.first().copied().unwrap_or_default(),
                    parts.get(1).copied(),
                )
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

        let remaining_path = destination_parts.get(1..).unwrap_or_default().join(".");

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
            let restart_cmd = Command::<L>::restart(restart_type, &remaining_path);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_remote_peer_cannot_restart_us_with_an_empty_destination() {
        // One message, no destination, and the receiving agent died - repeatably,
        // defeating a supervisor. See finding R2.
        assert_eq!(
            decide_restart("aip1", "clusters", &[], true),
            RestartDecision::Refuse(
                "a restart request from a remote peer must name its target agent explicitly"
            )
        );

        // ...and a leaf is refused for the same reason, not the cascade one.
        assert!(matches!(
            decide_restart("shared", "freeipa", &[], false),
            RestartDecision::Refuse(_)
        ));

        // The bridge's own `POST /restart` with no destination arrives via
        // `send_to(self)`, so the sender is us. That must still work.
        assert_eq!(
            decide_restart("thebridge", "thebridge", &[], true),
            RestartDecision::RestartSelf
        );
    }

    #[test]
    fn test_a_leaf_is_not_made_to_kill_itself_by_a_peer_it_will_not_relay_for() {
        // The leaf check used to run *after* the target check, so an agent that
        // refused to forward still honoured a restart aimed at itself. Naming us
        // explicitly is now the only way, and authorization comes first.
        assert_eq!(
            decide_restart("shared", "freeipa", &["freeipa"], false),
            RestartDecision::RestartSelf
        );

        // But it must not relay onwards...
        assert_eq!(
            decide_restart("shared", "freeipa", &["somewhere", "else"], false),
            RestartDecision::Refuse("this agent is a leaf node and cannot relay restart requests")
        );

        // ...whereas a cascading agent does.
        assert_eq!(
            decide_restart("aip1", "clusters", &["shared", "freeipa"], true),
            RestartDecision::Forward
        );
    }

    #[test]
    fn test_the_target_hop_may_carry_a_zone_suffix() {
        assert_eq!(
            decide_restart("aip1", "shared", &["shared@aip1-clusters-shared"], true),
            RestartDecision::RestartSelf
        );

        // A zone suffix on someone else's name is still someone else.
        assert_eq!(
            decide_restart("aip1", "shared", &["other@somezone"], true),
            RestartDecision::Forward
        );

        // A name that merely starts the same is not a match.
        assert_eq!(
            decide_restart("aip1", "shared", &["shared2"], true),
            RestartDecision::Forward
        );
    }
}
