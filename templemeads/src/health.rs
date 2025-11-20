// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Health check functionality for agents
//!
//! This module provides functions for collecting and cascading health information
//! across the agent network.

use crate::agent::{self, Peer, Type as AgentType};
use crate::command::Command;
use crate::grammar::NamedType;
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::RwLock;

/// Health information for an agent
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthInfo {
    /// Agent name
    pub name: String,
    /// Agent type
    pub agent_type: AgentType,
    /// Whether agent is connected
    pub connected: bool,
    /// Number of active jobs on this agent's boards
    pub active_jobs: usize,
    /// Number of pending jobs
    pub pending_jobs: usize,
    /// Number of running jobs
    pub running_jobs: usize,
    /// Number of completed jobs (on boards)
    pub completed_jobs: usize,
    /// Number of duplicate jobs
    pub duplicate_jobs: usize,
    /// Number of successfully completed jobs
    pub successful_jobs: usize,
    /// Number of jobs that expired
    pub expired_jobs: usize,
    /// Number of jobs that errored (not including expired)
    pub errored_jobs: usize,
    /// Number of active worker tasks processing messages
    pub worker_count: usize,
    /// Memory usage of this agent process in bytes
    pub memory_bytes: u64,
    /// CPU usage of this agent process (percentage, 0.0-100.0)
    pub cpu_percent: f32,
    /// Total system memory in bytes
    pub system_memory_total: u64,
    /// Number of CPU cores on the system
    pub system_cpus: usize,
    /// Minimum job execution time in milliseconds
    pub job_time_min_ms: f64,
    /// Maximum job execution time in milliseconds
    pub job_time_max_ms: f64,
    /// Mean job execution time in milliseconds
    pub job_time_mean_ms: f64,
    /// Median job execution time in milliseconds
    pub job_time_median_ms: f64,
    /// Number of jobs timed
    pub job_time_count: usize,
    /// Time when agent started
    pub start_time: DateTime<Utc>,
    /// Current time on agent
    pub current_time: DateTime<Utc>,
    /// Uptime in seconds
    pub uptime_seconds: i64,
    /// Engine name (e.g., "templemeads")
    pub engine: String,
    /// Engine version
    pub version: String,
    /// Time when this health response was received/cached
    pub last_updated: DateTime<Utc>,
    /// Nested health information from downstream peers
    #[serde(default)]
    pub peers: HashMap<String, Box<HealthInfo>>,
}

impl HealthInfo {
    pub fn new(
        name: &str,
        agent_type: AgentType,
        connected: bool,
        start_time: DateTime<Utc>,
        engine: &str,
        version: &str,
    ) -> Self {
        let current_time = Utc::now();
        let uptime_seconds = current_time.signed_duration_since(start_time).num_seconds();

        Self {
            name: name.to_owned(),
            agent_type,
            connected,
            active_jobs: 0,
            pending_jobs: 0,
            running_jobs: 0,
            completed_jobs: 0,
            duplicate_jobs: 0,
            successful_jobs: 0,
            expired_jobs: 0,
            errored_jobs: 0,
            worker_count: 0,
            memory_bytes: 0,
            cpu_percent: 0.0,
            system_memory_total: 0,
            system_cpus: 0,
            job_time_min_ms: 0.0,
            job_time_max_ms: 0.0,
            job_time_mean_ms: 0.0,
            job_time_median_ms: 0.0,
            job_time_count: 0,
            start_time,
            current_time,
            uptime_seconds,
            engine: engine.to_owned(),
            version: version.to_owned(),
            last_updated: current_time,
            peers: HashMap::new(),
        }
    }

    pub fn add_peer_health(&mut self, peer_health: HealthInfo) {
        self.peers
            .insert(peer_health.name.clone(), Box::new(peer_health));
    }

    pub fn get(&self, peer_name: &str) -> Option<HealthInfo> {
        self.peers.get(peer_name).map(|h| *h.clone())
    }

    pub fn keys(&self) -> Vec<String> {
        self.peers.keys().cloned().collect()
    }

    /// Formats health information as a human-readable string with hierarchical peer information.
    /// Highlights areas of concern such as high memory usage (>80%), high CPU (>80%),
    /// disconnected agents, and high job counts.
    pub fn to_pretty_string(&self) -> String {
        self.format_with_indent(0)
    }

    fn format_with_indent(&self, indent: usize) -> String {
        let prefix = "  ".repeat(indent);
        let mut output = String::new();

        // Format header with agent name and type
        output.push_str(&format!(
            "{}┌─ {} ({})\n",
            prefix, self.name, self.agent_type
        ));

        // Connection status with warning if disconnected
        let connection_status = if self.connected {
            "connected".to_string()
        } else {
            "DISCONNECTED ⚠️".to_string()
        };
        output.push_str(&format!("{}│  Status: {}\n", prefix, connection_status));

        // Uptime
        let uptime_str = Self::format_duration(self.uptime_seconds);
        output.push_str(&format!("{}│  Uptime: {}\n", prefix, uptime_str));

        // Memory usage with percentage and warning if high
        let memory_mb = self.memory_bytes as f64 / 1_048_576.0;
        let system_memory_gb = self.system_memory_total as f64 / 1_073_741_824.0;
        let memory_percent = if self.system_memory_total > 0 {
            (self.memory_bytes as f64 / self.system_memory_total as f64) * 100.0
        } else {
            0.0
        };

        let memory_warning = if memory_percent > 80.0 {
            " ⚠️ HIGH"
        } else if memory_percent > 60.0 {
            " ⚡"
        } else {
            ""
        };

        output.push_str(&format!(
            "{}│  Memory: {:.1} MB / {:.1} GB ({:.1}%){}\n",
            prefix, memory_mb, system_memory_gb, memory_percent, memory_warning
        ));

        // CPU usage with warning if high
        let cpu_warning = if self.cpu_percent > 80.0 {
            " ⚠️ HIGH"
        } else if self.cpu_percent > 60.0 {
            " ⚡"
        } else {
            ""
        };

        output.push_str(&format!(
            "{}│  CPU: {:.1}% ({} cores){}\n",
            prefix, self.cpu_percent, self.system_cpus, cpu_warning
        ));

        // Worker count
        output.push_str(&format!("{}│  Workers: {}\n", prefix, self.worker_count));

        // Job statistics with warnings for high counts
        let pending_warning = if self.pending_jobs > 100 {
            " ⚠️"
        } else if self.pending_jobs > 50 {
            " ⚡"
        } else {
            ""
        };

        let running_warning = if self.running_jobs > 50 {
            " ⚠️"
        } else if self.running_jobs > 20 {
            " ⚡"
        } else {
            ""
        };

        let expired_warning = if self.expired_jobs > 10 {
            " ⚠️"
        } else if self.expired_jobs > 0 {
            " ⚡"
        } else {
            ""
        };

        let errored_warning = if self.errored_jobs > 10 {
            " ⚠️"
        } else if self.errored_jobs > 0 {
            " ⚡"
        } else {
            ""
        };

        output.push_str(&format!(
            "{}│  Jobs: {} active ({} pending{}, {} running{}, {} completed, {} duplicates)\n",
            prefix,
            self.active_jobs,
            self.pending_jobs,
            pending_warning,
            self.running_jobs,
            running_warning,
            self.completed_jobs,
            self.duplicate_jobs
        ));

        output.push_str(&format!(
            "{}│    ├─ Successful: {}\n",
            prefix, self.successful_jobs
        ));

        output.push_str(&format!(
            "{}│    ├─ Expired: {}{}\n",
            prefix, self.expired_jobs, expired_warning
        ));

        output.push_str(&format!(
            "{}│    └─ Errored: {}{}\n",
            prefix, self.errored_jobs, errored_warning
        ));

        // Job timing information if available
        if self.job_time_count > 0 {
            output.push_str(&format!(
                "{}│  Job Timing: min={:.1}ms, max={:.1}ms, mean={:.1}ms, median={:.1}ms (n={})\n",
                prefix,
                self.job_time_min_ms,
                self.job_time_max_ms,
                self.job_time_mean_ms,
                self.job_time_median_ms,
                self.job_time_count
            ));
        }

        // Engine and version
        output.push_str(&format!(
            "{}│  Engine: {} v{}\n",
            prefix, self.engine, self.version
        ));

        // Last updated timestamp
        let age = Utc::now()
            .signed_duration_since(self.last_updated)
            .num_seconds();
        let age_str = if age > 0 {
            format!(" ({}s ago)", age)
        } else {
            String::new()
        };
        output.push_str(&format!(
            "{}│  Last Updated: {}{}\n",
            prefix,
            self.last_updated.format("%Y-%m-%d %H:%M:%S UTC"),
            age_str
        ));

        // Peer health information (recursively formatted)
        if !self.peers.is_empty() {
            output.push_str(&format!("{}│  Peers: {}\n", prefix, self.peers.len()));

            let mut sorted_peers: Vec<_> = self.peers.iter().collect();
            sorted_peers.sort_by(|a, b| a.0.cmp(b.0));

            for (idx, (_name, peer_health)) in sorted_peers.iter().enumerate() {
                let is_last = idx == sorted_peers.len() - 1;
                let connector = if is_last { "└" } else { "├" };
                output.push_str(&format!("{}│  {}\n", prefix, connector));

                // Recursively format peer health with increased indent
                output.push_str(&peer_health.format_with_indent(indent + 2));
            }
        }

        output.push_str(&format!("{}└─\n", prefix));

        output
    }

    fn format_duration(seconds: i64) -> String {
        let days = seconds / 86400;
        let hours = (seconds % 86400) / 3600;
        let mins = (seconds % 3600) / 60;
        let secs = seconds % 60;

        if days > 0 {
            format!("{}d {}h {}m {}s", days, hours, mins, secs)
        } else if hours > 0 {
            format!("{}h {}m {}s", hours, mins, secs)
        } else if mins > 0 {
            format!("{}m {}s", mins, secs)
        } else {
            format!("{}s", secs)
        }
    }
}

impl NamedType for HealthInfo {
    fn type_name() -> &'static str {
        "HealthInfo"
    }
}

impl std::fmt::Display for HealthInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let memory_mb = self.memory_bytes as f64 / 1_048_576.0;
        let system_memory_gb = self.system_memory_total as f64 / 1_073_741_824.0;
        let memory_percent = if self.system_memory_total > 0 {
            (self.memory_bytes as f64 / self.system_memory_total as f64) * 100.0
        } else {
            0.0
        };

        // Build job timing string if we have data
        let timing_str = if self.job_time_count > 0 {
            format!(
                ", timing: min={:.1}ms, max={:.1}ms, mean={:.1}ms, median={:.1}ms (n={})",
                self.job_time_min_ms,
                self.job_time_max_ms,
                self.job_time_mean_ms,
                self.job_time_median_ms,
                self.job_time_count
            )
        } else {
            String::new()
        };

        write!(
            f,
            "{} ({}) - {} - uptime: {}s, workers: {}, mem: {:.1}MB ({:.1}% of {:.1}GB), cpu: {:.1}%, {} cores, jobs: {} active ({} pending, {} running, {} completed [{} successful, {} expired, {} errored], {} duplicates){}",
            self.name,
            self.agent_type,
            if self.connected { "connected" } else { "disconnected" },
            self.uptime_seconds,
            self.worker_count,
            memory_mb,
            memory_percent,
            system_memory_gb,
            self.cpu_percent,
            self.system_cpus,
            self.active_jobs,
            self.pending_jobs,
            self.running_jobs,
            self.completed_jobs,
            self.successful_jobs,
            self.expired_jobs,
            self.errored_jobs,
            self.duplicate_jobs,
            timing_str
        )
    }
}

///
/// Global cache of health responses from agents
/// Maps agent_name -> HealthInfo (with last_updated timestamp inside)
///
static HEALTH_CACHE: Lazy<RwLock<HashMap<String, HealthInfo>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

///
/// Store a health response in the global cache
///
pub async fn cache_health_response(mut health: HealthInfo) {
    let name = health.name.clone();

    // Update the last_updated timestamp to now
    health.last_updated = Utc::now();

    let mut cache = HEALTH_CACHE.write().await;
    cache.insert(name.clone(), health);

    tracing::debug!("Cached health response for agent: {}", name);
}

///
/// Get all cached health responses
/// Returns a HashMap of agent_name -> HealthInfo
///
pub async fn get_cached_health() -> HashMap<String, HealthInfo> {
    HEALTH_CACHE.read().await.clone()
}

///
/// Collect health information for this agent, including cascaded health from downstream peers
///
/// This function:
/// - Collects local health info (job stats, uptime, etc.)
/// - Sends health checks to all downstream peers (excluding the requester and visited chain)
/// - Waits intelligently for responses (up to 500ms or until all respond)
/// - Marks disconnected peers appropriately
///
/// Parameters:
/// - `requester`: The agent that requested this health check
/// - `visited`: Chain of agents already visited in this health check (to prevent circular loops)
///
pub async fn collect_health(
    requester: &str,
    visited: Vec<String>,
) -> Result<HealthInfo, anyhow::Error> {
    tracing::debug!(
        "Collecting health information (visited chain length: {})",
        visited.len()
    );

    // Collect our own health information
    let agent_name = agent::name().await;
    let agent_type = agent::my_agent_type().await;
    let start_time = agent::start_time().await;
    let engine = agent::engine().await;
    let version = agent::version().await;

    let mut health = HealthInfo::new(
        &agent_name,
        agent_type.clone(),
        true, // connected (since we're responding)
        start_time,
        &engine,
        &version,
    );

    // Get aggregated job stats from all boards
    let (active, pending, running, completed, duplicates, successful, expired, errored) =
        crate::state::aggregate_job_stats().await;

    health.active_jobs = active;
    health.pending_jobs = pending;
    health.running_jobs = running;
    health.completed_jobs = completed;
    health.duplicate_jobs = duplicates;
    health.successful_jobs = successful;
    health.expired_jobs = expired;
    health.errored_jobs = errored;

    // Get the worker count from paddington
    health.worker_count = paddington::worker_count();

    // Collect system information (memory, CPU, etc.)
    let sysinfo = crate::systeminfo::collect();
    health.memory_bytes = sysinfo.memory_bytes;
    health.cpu_percent = sysinfo.cpu_percent;
    health.system_memory_total = sysinfo.system_memory_total;
    health.system_cpus = sysinfo.system_cpus;

    // Collect job timing statistics
    let job_stats = crate::jobtiming::get_stats();
    health.job_time_min_ms = job_stats.min_ms;
    health.job_time_max_ms = job_stats.max_ms;
    health.job_time_mean_ms = job_stats.mean_ms;
    health.job_time_median_ms = job_stats.median_ms;
    health.job_time_count = job_stats.count;

    // Cascade health check to downstream peers (if enabled for this agent)
    // Leaf nodes (like FreeIPA or Filesystem) have cascade_health=false
    if agent::should_cascade_health().await {
        // Exclude:
        // - The requester (to avoid immediate loops)
        // - Any agents in the visited chain (to avoid circular loops across zones)
        // - Other portals (security: portals must not query other portals)
        let all_peers = agent::real_peers().await;

        // Filter based on basic rules first
        let mut downstream_peers: Vec<_> = all_peers
            .into_iter()
            .filter(|p| p.name() != requester && !visited.contains(&p.name().to_owned()))
            .collect();

        // Security: If we're a portal, remove other portals from the list
        if agent_type == agent::Type::Portal {
            let mut filtered_peers = Vec::new();
            for peer in downstream_peers {
                if let Some(peer_type) = agent::agent_type(&peer).await {
                    if peer_type == agent::Type::Portal {
                        tracing::debug!(
                            "Skipping portal {} - portals do not query other portals",
                            peer.name()
                        );
                        continue;
                    }
                }
                filtered_peers.push(peer);
            }
            downstream_peers = filtered_peers;
        }

        if !downstream_peers.is_empty() {
            // Build new visited chain: existing visited + this agent
            let mut new_visited = visited.clone();
            new_visited.push(agent_name.clone());

            cascade_health_checks(&mut health, &downstream_peers, new_visited).await;
        }
    } else {
        tracing::debug!("Health cascade disabled for this agent (leaf node)");
    }

    Ok(health)
}

///
/// Cascade health checks to downstream peers and populate the health.peers map
///
/// Parameters:
/// - `visited`: Chain of agents already visited (will be passed to downstream peers)
///
async fn cascade_health_checks(
    health: &mut HealthInfo,
    downstream_peers: &[Peer],
    visited: Vec<String>,
) {
    tracing::debug!(
        "Cascading health check to {} downstream peers (visited chain: {:?})",
        downstream_peers.len(),
        visited
    );

    // Record baseline time before sending health checks
    let baseline_time = Utc::now();

    // Send health checks to all downstream peers, tracking which ones succeed
    let mut successfully_contacted = Vec::new();
    let mut disconnected_peers = Vec::new();

    for peer in downstream_peers.iter() {
        let health_check = Command::health_check_with_visited(visited.clone());
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

    tracing::debug!("Disconnected peers: {:?}", disconnected_peers);

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
    let cached_health = get_cached_health().await;
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
        tracing::debug!(
            "Marking peer {} as disconnected in health response",
            peer.name()
        );
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
        let cache = HEALTH_CACHE.read().await;
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
            let cache = HEALTH_CACHE.read().await;
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
