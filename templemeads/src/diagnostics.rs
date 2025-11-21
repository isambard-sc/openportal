// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Diagnostics tracking and reporting for agents
//!
//! This module provides real-time tracking of job failures, slow executions,
//! expirations, and other diagnostic information useful for remote troubleshooting.

use crate::agent;
use crate::command::Command;
use crate::grammar::NamedType;
use crate::job::Job;
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use tokio::sync::RwLock;

/// Maximum number of failed jobs to track
const MAX_FAILED_JOBS: usize = 200;

/// Maximum number of slow jobs to track
const MAX_SLOW_JOBS: usize = 200;

/// Maximum number of expired jobs to track
const MAX_EXPIRED_JOBS: usize = 200;

/// Diagnostics report containing troubleshooting information
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiagnosticsReport {
    /// Agent name
    pub agent_name: String,
    /// When this report was generated
    pub generated_at: DateTime<Utc>,
    /// Failed jobs (deduplicated, most recent 100)
    pub failed_jobs: Vec<FailedJobEntry>,
    /// Slowest successful jobs (top 100)
    pub slowest_jobs: Vec<SlowJobEntry>,
    /// Expired jobs (most recent 100, deduplicated)
    pub expired_jobs: Vec<ExpiredJobEntry>,
    /// Currently running jobs (deduplicated with count)
    pub running_jobs: Vec<RunningJobEntry>,
    /// System warnings and issues
    pub warnings: Vec<String>,
}

/// Entry for a failed job
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FailedJobEntry {
    /// Job destination
    pub destination: String,
    /// Job instruction
    pub instruction: String,
    /// Error message
    pub error_message: String,
    /// Number of times this failed
    pub count: usize,
    /// First occurrence
    pub first_seen: DateTime<Utc>,
    /// Most recent occurrence
    pub last_seen: DateTime<Utc>,
}

/// Entry for a slow job
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd)]
pub struct SlowJobEntry {
    /// Job destination
    pub destination: String,
    /// Job instruction
    pub instruction: String,
    /// Duration in milliseconds
    pub duration_ms: f64,
    /// When it completed
    pub completed_at: DateTime<Utc>,
}

/// Entry for an expired job
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExpiredJobEntry {
    /// Job destination
    pub destination: String,
    /// Job instruction
    pub instruction: String,
    /// When job was created
    pub created_at: DateTime<Utc>,
    /// When job expired
    pub expired_at: DateTime<Utc>,
    /// Number of times this job type expired
    pub count: usize,
}

/// Entry for a currently running job
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunningJobEntry {
    /// Job destination
    pub destination: String,
    /// Job instruction
    pub instruction: String,
    /// When the job started
    pub started_at: DateTime<Utc>,
    /// Number of instances running
    pub count: usize,
    /// How long it's been running (seconds)
    pub running_for_seconds: i64,
}

impl NamedType for DiagnosticsReport {
    fn type_name() -> &'static str {
        "DiagnosticsReport"
    }
}

impl NamedType for FailedJobEntry {
    fn type_name() -> &'static str {
        "FailedJobEntry"
    }
}

impl NamedType for SlowJobEntry {
    fn type_name() -> &'static str {
        "SlowJobEntry"
    }
}

impl NamedType for ExpiredJobEntry {
    fn type_name() -> &'static str {
        "ExpiredJobEntry"
    }
}

impl NamedType for RunningJobEntry {
    fn type_name() -> &'static str {
        "RunningJobEntry"
    }
}

/// Key for deduplicating jobs
#[derive(Hash, Eq, PartialEq, Clone)]
struct JobKey {
    destination: String,
    instruction: String,
}

impl JobKey {
    fn from_job(job: &Job) -> Self {
        Self {
            destination: job.destination().to_string(),
            instruction: job.instruction().to_string(),
        }
    }
}

/// Internal storage for failed job tracking
struct FailedJobData {
    error_message: String,
    count: usize,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
}

/// Internal storage for expired job tracking
struct ExpiredJobData {
    created_at: DateTime<Utc>,
    expired_at: DateTime<Utc>,
    count: usize,
}

/// Internal storage for running job tracking
struct RunningJobData {
    started_at: DateTime<Utc>,
    count: usize,
}

/// Global diagnostics tracker
struct DiagnosticsTracker {
    /// Failed jobs, deduplicated by destination+instruction
    failed_jobs: HashMap<JobKey, FailedJobData>,
    /// Order of failed job keys (for FIFO eviction)
    failed_jobs_order: VecDeque<JobKey>,
    /// Slowest jobs (sorted, we keep top N)
    slowest_jobs: Vec<SlowJobEntry>,
    /// Expired jobs, deduplicated by destination+instruction
    expired_jobs: HashMap<JobKey, ExpiredJobData>,
    /// Order of expired job keys (for FIFO eviction)
    expired_jobs_order: VecDeque<JobKey>,
    /// Currently running jobs, deduplicated by destination+instruction
    running_jobs: HashMap<JobKey, RunningJobData>,
}

impl DiagnosticsTracker {
    fn new() -> Self {
        Self {
            failed_jobs: HashMap::new(),
            failed_jobs_order: VecDeque::new(),
            slowest_jobs: Vec::new(),
            expired_jobs: HashMap::new(),
            expired_jobs_order: VecDeque::new(),
            running_jobs: HashMap::new(),
        }
    }

    fn record_failed_job(&mut self, job: &Job, error_message: String) {
        let key = JobKey::from_job(job);
        let now = Utc::now();

        if let Some(data) = self.failed_jobs.get_mut(&key) {
            // Update existing entry
            data.count += 1;
            data.last_seen = now;
            data.error_message = error_message; // Keep most recent error message
        } else {
            // New entry
            self.failed_jobs.insert(
                key.clone(),
                FailedJobData {
                    error_message,
                    count: 1,
                    first_seen: now,
                    last_seen: now,
                },
            );
            self.failed_jobs_order.push_back(key.clone());

            // Evict oldest if we're over capacity
            if self.failed_jobs_order.len() > MAX_FAILED_JOBS {
                if let Some(old_key) = self.failed_jobs_order.pop_front() {
                    self.failed_jobs.remove(&old_key);
                }
            }
        }
    }

    fn record_slow_job(&mut self, job: &Job, duration_ms: f64) {
        let entry = SlowJobEntry {
            destination: job.destination().to_string(),
            instruction: job.instruction().to_string(),
            duration_ms,
            completed_at: Utc::now(),
        };

        // Insert and keep sorted by duration (descending)
        self.slowest_jobs.push(entry);
        self.slowest_jobs.sort_by(|a, b| {
            b.duration_ms
                .partial_cmp(&a.duration_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Keep only top N
        if self.slowest_jobs.len() > MAX_SLOW_JOBS {
            self.slowest_jobs.truncate(MAX_SLOW_JOBS);
        }
    }

    fn record_expired_job(&mut self, job: &Job) {
        let key = JobKey::from_job(job);

        if let Some(data) = self.expired_jobs.get_mut(&key) {
            // Update existing entry
            data.count += 1;
            data.expired_at = Utc::now();
        } else {
            // New entry
            self.expired_jobs.insert(
                key.clone(),
                ExpiredJobData {
                    created_at: job.created(),
                    expired_at: Utc::now(),
                    count: 1,
                },
            );
            self.expired_jobs_order.push_back(key.clone());

            // Evict oldest if we're over capacity
            if self.expired_jobs_order.len() > MAX_EXPIRED_JOBS {
                if let Some(old_key) = self.expired_jobs_order.pop_front() {
                    self.expired_jobs.remove(&old_key);
                }
            }
        }
    }

    fn record_job_started(&mut self, job: &Job) {
        let key = JobKey::from_job(job);

        if let Some(data) = self.running_jobs.get_mut(&key) {
            data.count += 1;
        } else {
            self.running_jobs.insert(
                key,
                RunningJobData {
                    started_at: Utc::now(),
                    count: 1,
                },
            );
        }
    }

    fn record_job_finished(&mut self, job: &Job) {
        let key = JobKey::from_job(job);

        if let Some(data) = self.running_jobs.get_mut(&key) {
            if data.count > 1 {
                data.count -= 1;
            } else {
                self.running_jobs.remove(&key);
            }
        }
    }

    fn generate_report(&self, agent_name: &str) -> DiagnosticsReport {
        let now = Utc::now();

        // Convert failed jobs to report entries (most recent 100)
        let failed_jobs: Vec<FailedJobEntry> = self
            .failed_jobs_order
            .iter()
            .rev() // Most recent first
            .take(100)
            .filter_map(|key| {
                self.failed_jobs.get(key).map(|data| FailedJobEntry {
                    destination: key.destination.clone(),
                    instruction: key.instruction.clone(),
                    error_message: data.error_message.clone(),
                    count: data.count,
                    first_seen: data.first_seen,
                    last_seen: data.last_seen,
                })
            })
            .collect();

        // Top 100 slowest jobs
        let slowest_jobs: Vec<SlowJobEntry> = self.slowest_jobs.iter().take(100).cloned().collect();

        // Most recent 100 expired jobs
        let expired_jobs: Vec<ExpiredJobEntry> = self
            .expired_jobs_order
            .iter()
            .rev() // Most recent first
            .take(100)
            .filter_map(|key| {
                self.expired_jobs.get(key).map(|data| ExpiredJobEntry {
                    destination: key.destination.clone(),
                    instruction: key.instruction.clone(),
                    created_at: data.created_at,
                    expired_at: data.expired_at,
                    count: data.count,
                })
            })
            .collect();

        // Currently running jobs
        let mut running_jobs: Vec<RunningJobEntry> = self
            .running_jobs
            .iter()
            .map(|(key, data)| {
                let running_for_seconds = now.signed_duration_since(data.started_at).num_seconds();
                RunningJobEntry {
                    destination: key.destination.clone(),
                    instruction: key.instruction.clone(),
                    started_at: data.started_at,
                    count: data.count,
                    running_for_seconds,
                }
            })
            .collect();

        // Sort running jobs by how long they've been running (longest first)
        running_jobs.sort_by(|a, b| b.running_for_seconds.cmp(&a.running_for_seconds));

        // Generate warnings
        let mut warnings = Vec::new();

        // Warning for high failure rates
        let high_failure_jobs: Vec<_> = failed_jobs.iter().filter(|e| e.count >= 10).collect();
        if !high_failure_jobs.is_empty() {
            warnings.push(format!(
                "High failure rate detected for {} job type(s)",
                high_failure_jobs.len()
            ));
        }

        // Warning for long-running jobs
        let long_running: Vec<_> = running_jobs
            .iter()
            .filter(|e| e.running_for_seconds > 300) // 5 minutes
            .collect();
        if !long_running.is_empty() {
            warnings.push(format!(
                "{} job(s) running longer than 5 minutes",
                long_running.len()
            ));
        }

        // Warning for many expired jobs
        if expired_jobs.len() > 50 {
            warnings.push(format!(
                "High number of expired jobs: {} tracked",
                expired_jobs.len()
            ));
        }

        DiagnosticsReport {
            agent_name: agent_name.to_string(),
            generated_at: now,
            failed_jobs,
            slowest_jobs,
            expired_jobs,
            running_jobs,
            warnings,
        }
    }
}

/// Global diagnostics tracker instance
static DIAGNOSTICS: Lazy<RwLock<DiagnosticsTracker>> =
    Lazy::new(|| RwLock::new(DiagnosticsTracker::new()));

/// Record a failed job
pub async fn record_failed_job(job: &Job, error_message: String) {
    let mut tracker = DIAGNOSTICS.write().await;
    tracker.record_failed_job(job, error_message);
}

/// Record a slow job completion
pub async fn record_slow_job(job: &Job, duration_ms: f64) {
    // Only track jobs that are "slow" (> 1 second)
    if duration_ms > 1000.0 {
        let mut tracker = DIAGNOSTICS.write().await;
        tracker.record_slow_job(job, duration_ms);
    }
}

/// Record an expired job
pub async fn record_expired_job(job: &Job) {
    let mut tracker = DIAGNOSTICS.write().await;
    tracker.record_expired_job(job);
}

/// Record when a job starts running
pub async fn record_job_started(job: &Job) {
    let mut tracker = DIAGNOSTICS.write().await;
    tracker.record_job_started(job);
}

/// Record when a job finishes (successful or failed)
pub async fn record_job_finished(job: &Job) {
    let mut tracker = DIAGNOSTICS.write().await;
    tracker.record_job_finished(job);
}

/// Generate a diagnostics report
pub async fn generate_report(agent_name: &str) -> DiagnosticsReport {
    let tracker = DIAGNOSTICS.read().await;
    tracker.generate_report(agent_name)
}

///
/// Global cache of diagnostics responses from agents
/// Maps agent_name -> DiagnosticsReport
/// The key is the agent name that generated the report
/// This allows intermediate agents to retrieve and forward responses back through the network
///
static DIAGNOSTICS_CACHE: Lazy<RwLock<HashMap<String, DiagnosticsReport>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

///
/// Store a diagnostics response in the global cache
///
/// Parameters:
/// - `agent_name`: The name of the agent that generated the report
/// - `report`: The diagnostics report to cache
///
pub async fn cache_diagnostics_response(agent_name: String, report: DiagnosticsReport) {
    let mut cache = DIAGNOSTICS_CACHE.write().await;
    cache.insert(agent_name.clone(), report);

    tracing::debug!("Cached diagnostics response for agent: {}", agent_name);
}

///
/// Get a cached diagnostics report for a specific agent
///
pub async fn get_cached_diagnostics(agent_name: &str) -> Option<DiagnosticsReport> {
    DIAGNOSTICS_CACHE.read().await.get(agent_name).cloned()
}

///
/// Wait for a diagnostics response from a specific agent
///
/// Polls the cache until the agent has an updated diagnostics report since baseline_time,
/// or until the timeout expires.
///
/// Parameters:
/// - `agent_name`: The name of the agent we're waiting for a response from
/// - `baseline_time`: Only accept reports generated after this time
/// - `timeout`: How long to wait before giving up
///
/// Returns the cached report if found, None otherwise.
///
async fn wait_for_diagnostics_response(
    agent_name: &str,
    baseline_time: DateTime<Utc>,
    timeout: std::time::Duration,
) -> Option<DiagnosticsReport> {
    let start = tokio::time::Instant::now();
    let deadline = start + timeout;

    loop {
        // Check if we have an updated diagnostics report since baseline
        let cache = DIAGNOSTICS_CACHE.read().await;
        if let Some(report) = cache.get(agent_name) {
            if report.generated_at > baseline_time {
                tracing::debug!(
                    "Diagnostics response received from {} in {:?}",
                    agent_name,
                    start.elapsed()
                );
                let result = report.clone();
                drop(cache);
                return Some(result);
            }
        }
        drop(cache);

        // Check if we've exceeded timeout
        if tokio::time::Instant::now() >= deadline {
            tracing::debug!(
                "Diagnostics request timeout: no response from {} within {:?}",
                agent_name,
                timeout
            );
            return None;
        }

        // Sleep briefly before checking again
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

///
/// Collect diagnostics from a specific agent or self
///
/// This function sends a diagnostics request to the specified destination and waits
/// for the response (up to 5 seconds). If no destination is provided, generates a report for self.
///
/// Parameters:
/// - `destination`: Dot-separated path to target agent (e.g., "provider.cluster"), empty means self
///
/// Returns the diagnostics report or an error if the request fails or times out.
///
pub async fn collect_diagnostics(destination: &str) -> Result<DiagnosticsReport, anyhow::Error> {
    let my_name = agent::get_self(None).await.name().to_owned();

    // Parse the destination path
    let destination_parts: Vec<&str> = if destination.is_empty() {
        vec![]
    } else {
        destination.split('.').collect()
    };

    // Check if we are the target for this diagnostics request
    let is_target = if destination_parts.is_empty() {
        // Empty destination means request from the agent that received the request
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
        let report = generate_report(&my_name).await;

        tracing::debug!("Diagnostics report: {}", report);

        Ok(report)
    } else {
        // Check if this agent is allowed to forward diagnostics requests
        // Leaf nodes (like FreeIPA or Filesystem) have cascade_health=false and should not forward
        if !agent::should_cascade_health().await {
            tracing::warn!(
                "Cannot forward diagnostics request - this agent is a leaf node (cascade disabled)"
            );
            return Err(anyhow::anyhow!(
                "Leaf node agents cannot forward diagnostics requests"
            ));
        }

        // We need to forward the diagnostics request to the next peer in the path
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

        tracing::debug!(
            "Forwarding diagnostics request to {} (remaining path: {})",
            next_peer_name,
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
            if agent::my_agent_type().await == agent::Type::Portal {
                if let Some(peer_type) = agent::agent_type(next_peer).await {
                    if peer_type == agent::Type::Portal {
                        tracing::error!(
                            "Cannot forward diagnostics to portal {} - portals do not share diagnostics with other portals",
                            next_peer_name
                        );
                        return Err(anyhow::anyhow!(
                            "Portals cannot forward diagnostics requests to other portals"
                        ));
                    }
                }
            }

            // Wait for the peer to be ready
            agent::wait_for(next_peer, 30).await?;

            // Record baseline time before sending request
            let baseline_time = Utc::now();

            // Forward the diagnostics command with the updated destination
            let diagnostics_cmd = Command::diagnostics_request(&remaining_path);
            diagnostics_cmd.send_to(next_peer).await?;

            tracing::debug!(
                "Forwarded diagnostics request to {} in zone {}, waiting for response...",
                next_peer_name,
                next_peer.zone()
            );

            // Extract the ultimate target agent name from the destination path
            // This is the agent that will actually generate the report
            // For "cluster.filesystem", the ultimate target is "filesystem"
            // For "cluster", the ultimate target is "cluster"
            let ultimate_target = if remaining_path.is_empty() {
                // Next peer is the ultimate target
                next_peer_name
            } else {
                // Find the last component in the remaining path
                remaining_path.split('.').last().unwrap_or(next_peer_name)
            };

            // Extract just the name part if it includes @zone
            let ultimate_target_name = if ultimate_target.contains('@') {
                ultimate_target.split('@').next().unwrap_or(ultimate_target)
            } else {
                ultimate_target
            };

            // Wait for the response (with 500ms timeout, or use cached if available)
            let report = wait_for_diagnostics_response(
                ultimate_target_name,
                baseline_time,
                std::time::Duration::from_millis(500),
            )
            .await;

            if let Some(report) = report {
                tracing::debug!(
                    "Received diagnostics response from {}",
                    ultimate_target_name,
                );
                Ok(report)
            } else {
                // Check if we have any cached report (even if old)
                if let Some(cached_report) = get_cached_diagnostics(ultimate_target_name).await {
                    tracing::warn!(
                        "Timeout waiting for fresh diagnostics from {}, returning cached response (age: {}s)",
                        ultimate_target_name,
                        Utc::now().signed_duration_since(cached_report.generated_at).num_seconds(),
                    );
                    Ok(cached_report)
                } else {
                    tracing::warn!(
                        "No diagnostics response received from {} and no cached data available",
                        ultimate_target_name
                    );
                    Err(anyhow::anyhow!(
                        "No diagnostics response received from {}",
                        ultimate_target_name
                    ))
                }
            }
        } else {
            let error_msg = if let Some(zone) = zone_filter {
                format!(
                    "Cannot find peer {} in zone {} to forward diagnostics request to",
                    next_peer_name, zone
                )
            } else {
                format!(
                    "Cannot find peer {} to forward diagnostics request to",
                    next_peer_name
                )
            };
            tracing::error!("{}", error_msg);
            Err(anyhow::anyhow!("{}", error_msg))
        }
    }
}

impl DiagnosticsReport {
    /// Format diagnostics report as a human-readable string
    pub fn to_pretty_string(&self) -> String {
        let mut output = String::new();

        output.push_str(&format!("┌─ Diagnostics Report: {}\n", self.agent_name));
        output.push_str(&format!(
            "│  Generated: {}\n",
            self.generated_at.format("%Y-%m-%d %H:%M:%S UTC")
        ));
        output.push_str("│\n");

        // Warnings section (if any)
        if !self.warnings.is_empty() {
            output.push_str("│  ┌─ Warnings\n");
            for warning in &self.warnings {
                output.push_str(&format!("│  │  ⚠️  {}\n", warning));
            }
            output.push_str("│  │\n");
        }

        // Failed jobs section
        output.push_str(&format!(
            "│  ┌─ Failed Jobs ({} tracked, showing up to 100)\n",
            self.failed_jobs.len()
        ));
        if self.failed_jobs.is_empty() {
            output.push_str("│  │  No failed jobs recorded\n");
        } else {
            for (idx, job) in self.failed_jobs.iter().enumerate().take(100) {
                let time_range = if job.count > 1 {
                    format!(
                        "First: {}, Last: {}",
                        job.first_seen.format("%Y-%m-%d %H:%M:%S"),
                        job.last_seen.format("%Y-%m-%d %H:%M:%S")
                    )
                } else {
                    format!("At: {}", job.first_seen.format("%Y-%m-%d %H:%M:%S"))
                };

                output.push_str(&format!(
                    "│  │  {}. {} {}: \"{}\" ({} occurrence{})\n",
                    idx + 1,
                    job.destination,
                    job.instruction,
                    job.error_message,
                    job.count,
                    if job.count == 1 { "" } else { "s" }
                ));
                output.push_str(&format!("│  │     {}\n", time_range));
            }
        }
        output.push_str("│  │\n");

        // Slowest jobs section
        output.push_str(&format!(
            "│  ┌─ Slowest Jobs ({} tracked, showing up to 100)\n",
            self.slowest_jobs.len()
        ));
        if self.slowest_jobs.is_empty() {
            output.push_str("│  │  No slow jobs recorded\n");
        } else {
            for (idx, job) in self.slowest_jobs.iter().enumerate().take(100) {
                let duration_str = if job.duration_ms > 60000.0 {
                    format!("{:.1}min", job.duration_ms / 60000.0)
                } else if job.duration_ms > 1000.0 {
                    format!("{:.1}s", job.duration_ms / 1000.0)
                } else {
                    format!("{:.1}ms", job.duration_ms)
                };

                output.push_str(&format!(
                    "│  │  {}. {} {}: {} ({})\n",
                    idx + 1,
                    job.destination,
                    job.instruction,
                    duration_str,
                    job.completed_at.format("%Y-%m-%d %H:%M:%S")
                ));
            }
        }
        output.push_str("│  │\n");

        // Expired jobs section
        output.push_str(&format!(
            "│  ┌─ Expired Jobs ({} tracked, showing up to 100)\n",
            self.expired_jobs.len()
        ));
        if self.expired_jobs.is_empty() {
            output.push_str("│  │  No expired jobs recorded\n");
        } else {
            for (idx, job) in self.expired_jobs.iter().enumerate().take(100) {
                output.push_str(&format!(
                    "│  │  {}. {} {} ({} occurrence{})\n",
                    idx + 1,
                    job.destination,
                    job.instruction,
                    job.count,
                    if job.count == 1 { "" } else { "s" }
                ));
                output.push_str(&format!(
                    "│  │     Created: {}, Expired: {}\n",
                    job.created_at.format("%Y-%m-%d %H:%M:%S"),
                    job.expired_at.format("%Y-%m-%d %H:%M:%S")
                ));
            }
        }
        output.push_str("│  │\n");

        // Running jobs section
        output.push_str(&format!(
            "│  ┌─ Currently Running Jobs ({})\n",
            self.running_jobs.len()
        ));
        if self.running_jobs.is_empty() {
            output.push_str("│  │  No jobs currently running\n");
        } else {
            for (idx, job) in self.running_jobs.iter().enumerate() {
                let duration = format_duration(job.running_for_seconds);
                let warning = if job.running_for_seconds > 300 {
                    " ⚠️"
                } else if job.running_for_seconds > 60 {
                    " ⚡"
                } else {
                    ""
                };

                output.push_str(&format!(
                    "│  │  {}. {} {} ({} instance{}) - running for {}{}\n",
                    idx + 1,
                    job.destination,
                    job.instruction,
                    job.count,
                    if job.count == 1 { "" } else { "s" },
                    duration,
                    warning
                ));
                output.push_str(&format!(
                    "│  │     Started: {}\n",
                    job.started_at.format("%Y-%m-%d %H:%M:%S")
                ));
            }
        }

        output.push_str("└─\n");

        output
    }
}

impl std::fmt::Display for DiagnosticsReport {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "DiagnosticsReport for {} - {} failed, {} slow, {} expired, {} running (generated {})",
            self.agent_name,
            self.failed_jobs.len(),
            self.slowest_jobs.len(),
            self.expired_jobs.len(),
            self.running_jobs.len(),
            self.generated_at.format("%Y-%m-%d %H:%M:%S UTC")
        )
    }
}

impl std::fmt::Display for FailedJobEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{} {}: \"{}\" ({} occurrence{})",
            self.destination,
            self.instruction,
            self.error_message,
            self.count,
            if self.count == 1 { "" } else { "s" }
        )
    }
}

impl std::fmt::Display for SlowJobEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{} {}: {:.1}ms (completed at {})",
            self.destination,
            self.instruction,
            self.duration_ms,
            self.completed_at.format("%Y-%m-%d %H:%M:%S")
        )
    }
}

impl std::fmt::Display for ExpiredJobEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{} {} ({} occurrence{}) - Created: {}, Expired: {}",
            self.destination,
            self.instruction,
            self.count,
            if self.count == 1 { "" } else { "s" },
            self.created_at.format("%Y-%m-%d %H:%M:%S"),
            self.expired_at.format("%Y-%m-%d %H:%M:%S")
        )
    }
}

impl std::fmt::Display for RunningJobEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{} {} ({} instance{}) - started at {}, running for {}",
            self.destination,
            self.instruction,
            self.count,
            if self.count == 1 { "" } else { "s" },
            self.started_at.format("%Y-%m-%d %H:%M:%S"),
            format_duration(self.running_for_seconds)
        )
    }
}

fn format_duration(seconds: i64) -> String {
    let hours = seconds / 3600;
    let mins = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, mins, secs)
    } else if mins > 0 {
        format!("{}m {}s", mins, secs)
    } else {
        format!("{}s", secs)
    }
}
