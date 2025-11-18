// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Health check functionality for agents
//!
//! This module provides functions for collecting and cascading health information
//! across the agent network.

use crate::agent::{self, Peer};
use crate::command::{Command, HealthInfo};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

///
/// Collect health information for this agent, including cascaded health from downstream peers
///
/// This function:
/// - Collects local health info (job stats, uptime, etc.)
/// - Sends health checks to all downstream peers (excluding the requester)
/// - Waits intelligently for responses (up to 500ms or until all respond)
/// - Marks disconnected peers appropriately
///
pub async fn collect_health(requester: &str) -> Result<HealthInfo, anyhow::Error> {
    tracing::info!("Collecting health information");

    // Collect our own health information
    let agent_name = agent::name().await;
    let agent_type = agent::my_agent_type().await;
    let start_time = agent::start_time().await;
    let engine = agent::engine().await;
    let version = agent::version().await;

    let mut health = HealthInfo::new(
        &agent_name,
        agent_type,
        true, // connected (since we're responding)
        start_time,
        &engine,
        &version,
    );

    // Get aggregated job stats from all boards
    let (active, pending, running, completed, duplicates) =
        crate::state::aggregate_job_stats().await;

    health.active_jobs = active;
    health.pending_jobs = pending;
    health.running_jobs = running;
    health.completed_jobs = completed;
    health.duplicate_jobs = duplicates;

    // Cascade health check to downstream peers (excluding the requester to avoid loops)
    let all_peers = agent::all_peers().await;
    let downstream_peers: Vec<_> = all_peers
        .into_iter()
        .filter(|p| p.name() != requester)
        .collect();

    if !downstream_peers.is_empty() {
        cascade_health_checks(&mut health, &downstream_peers).await;
    }

    Ok(health)
}

///
/// Cascade health checks to downstream peers and populate the health.peers map
///
async fn cascade_health_checks(health: &mut HealthInfo, downstream_peers: &[Peer]) {
    tracing::debug!(
        "Cascading health check to {} downstream peers",
        downstream_peers.len()
    );

    // Record baseline time before sending health checks
    let baseline_time = Utc::now();

    // Send health checks to all downstream peers, tracking which ones succeed
    let mut successfully_contacted = Vec::new();
    let mut disconnected_peers = Vec::new();

    for peer in downstream_peers.iter() {
        let health_check = Command::health_check();
        match health_check.send_to(peer).await {
            Ok(_) => {
                successfully_contacted.push(peer.name().to_owned());
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to send health check to {} (likely disconnected): {}",
                    peer.name(),
                    e
                );
                disconnected_peers.push(peer.clone());
            }
        }
    }

    // Only wait for peers we successfully contacted
    if !successfully_contacted.is_empty() {
        tracing::debug!(
            "Waiting for health responses from {} connected peers",
            successfully_contacted.len()
        );

        wait_for_health_updates(
            &successfully_contacted,
            baseline_time,
            std::time::Duration::from_millis(500),
        )
        .await;
    }

    // Retrieve cached health responses from downstream peers
    let cached_health = crate::handler::get_cached_health().await;
    for peer in downstream_peers.iter() {
        if let Some(peer_health) = cached_health.get(peer.name()) {
            health
                .peers
                .insert(peer.name().to_owned(), Box::new(peer_health.clone()));
        }
    }

    // Mark disconnected peers in the health response
    mark_disconnected_peers(health, &disconnected_peers, &cached_health).await;
}

///
/// Mark disconnected peers in the health response
///
/// For each disconnected peer, either uses cached health data (if available)
/// or creates minimal health info. Always marks them as connected=false.
///
async fn mark_disconnected_peers(
    health: &mut HealthInfo,
    disconnected_peers: &[Peer],
    cached_health: &HashMap<String, HealthInfo>,
) {
    for peer in disconnected_peers.iter() {
        // Try to get cached health first, otherwise create empty health
        let mut disconnected_health = if let Some(cached) = cached_health.get(peer.name()) {
            // Use cached health but mark as disconnected
            cached.clone()
        } else {
            // No cached health, create minimal health info
            if let Some(peer_type) = agent::agent_type(peer).await {
                HealthInfo::new(
                    peer.name(),
                    peer_type,
                    false,      // connected = false
                    Utc::now(), // start_time (unknown, use current time)
                    "unknown",
                    "unknown",
                )
            } else {
                // Can't find peer info, skip it
                continue;
            }
        };

        // Mark as disconnected
        disconnected_health.connected = false;

        health
            .peers
            .insert(peer.name().to_owned(), Box::new(disconnected_health));
    }
}

///
/// Wait for health responses from specified peers to be updated
///
/// Returns when all peers have responded with last_updated > baseline_time,
/// or when timeout is reached, whichever comes first
///
async fn wait_for_health_updates(
    peer_names: &[String],
    baseline_time: DateTime<Utc>,
    timeout: std::time::Duration,
) {
    let start = tokio::time::Instant::now();
    let deadline = start + timeout;

    loop {
        // Check if all peers have updated health since baseline
        let cache = crate::handler::HEALTH_CACHE.read().await;
        let all_updated = peer_names.iter().all(|name| {
            cache
                .get(name)
                .map(|health| health.last_updated > baseline_time)
                .unwrap_or(false)
        });
        drop(cache);

        if all_updated {
            tracing::debug!(
                "All {} peer health responses received in {:?}",
                peer_names.len(),
                start.elapsed()
            );
            return;
        }

        // Check if we've exceeded timeout
        if tokio::time::Instant::now() >= deadline {
            // Count how many peers did respond
            let cache = crate::handler::HEALTH_CACHE.read().await;
            let responded = peer_names
                .iter()
                .filter(|name| {
                    cache
                        .get(*name)
                        .map(|health| health.last_updated > baseline_time)
                        .unwrap_or(false)
                })
                .count();
            drop(cache);

            tracing::debug!(
                "Health check timeout: {}/{} peers responded within {:?}",
                responded,
                peer_names.len(),
                timeout
            );
            return;
        }

        // Sleep briefly before checking again
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
