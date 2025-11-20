// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! System information collection using sysinfo crate
//!
//! This module provides functions for collecting system metrics like memory and CPU usage.

use once_cell::sync::Lazy;
use std::sync::Mutex;
use sysinfo::{Pid, ProcessesToUpdate, System};

/// Global System instance for collecting system information
/// We use a Mutex to allow mutable access for refreshing
static SYSTEM: Lazy<Mutex<System>> = Lazy::new(|| Mutex::new(System::new_all()));

/// System information snapshot
#[derive(Debug, Clone)]
pub struct SystemInfo {
    /// Memory usage of this process in bytes
    pub memory_bytes: u64,
    /// CPU usage of this process (percentage, 0.0-100.0)
    pub cpu_percent: f32,
    /// Total system memory in bytes
    pub system_memory_total: u64,
    /// Number of CPU cores on the system
    pub system_cpus: usize,
}

impl Default for SystemInfo {
    fn default() -> Self {
        Self {
            memory_bytes: 0,
            cpu_percent: 0.0,
            system_memory_total: 0,
            system_cpus: 0,
        }
    }
}

/// Collect current system information for this process
///
/// This function refreshes system stats and returns:
/// - Memory usage of the current process
/// - CPU usage of the current process
/// - Total system memory
/// - Number of CPU cores
///
/// Note: The first call to this function may take longer as it initializes
/// the system information. Subsequent calls are faster.
///
/// CPU usage is calculated based on the difference since the last refresh,
/// so you may want to call this periodically (e.g., every 5-10 seconds) for
/// accurate CPU measurements.
pub fn collect() -> SystemInfo {
    let mut system = match SYSTEM.lock() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to lock system info: {}", e);
            return SystemInfo::default();
        }
    };

    // Get current process PID
    let pid = Pid::from_u32(std::process::id());

    // Refresh process-specific information for our process
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), false);

    // Refresh memory information
    system.refresh_memory();

    // Refresh CPU usage
    system.refresh_cpu_all();

    // Get process information
    let (memory_bytes, cpu_percent) = if let Some(process) = system.process(pid) {
        (process.memory(), process.cpu_usage())
    } else {
        tracing::warn!("Could not find current process in system info");
        (0, 0.0)
    };

    // Get system information
    let system_memory_total = system.total_memory();
    let system_cpus = system.cpus().len();

    SystemInfo {
        memory_bytes,
        cpu_percent,
        system_memory_total,
        system_cpus,
    }
}

/// Initialize system info collection
///
/// Call this once at startup to initialize the sysinfo System.
/// This allows the first `collect()` call to have baseline data for CPU usage.
///
/// It's optional to call this - `collect()` will initialize on first use,
/// but calling this at startup gives better CPU usage data from the start.
pub fn initialize() {
    let mut system = match SYSTEM.lock() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to lock system info for initialization: {}", e);
            return;
        }
    };

    // Initial refresh to establish baseline
    system.refresh_all();

    tracing::debug!(
        "System info initialized: {} CPUs, {} total memory",
        system.cpus().len(),
        system.total_memory()
    );
}

/// Spawn a background task that periodically refreshes system info and monitors resource usage
///
/// This task:
/// - Refreshes CPU data every 10 seconds for accurate measurements
/// - Logs warnings if CPU usage exceeds 90%
/// - Logs warnings if process memory usage exceeds 80% of total system memory
/// - Initializes system info on first run
///
/// Call this once at startup. The task runs indefinitely in the background.
pub fn spawn_monitor() {
    tokio::spawn(async {
        // Initialize on first run
        initialize();

        // Wait a bit for the first CPU measurement to stabilize
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        loop {
            // Collect current system info
            let info = collect();

            // Check CPU usage
            if info.cpu_percent > 90.0 {
                tracing::warn!("High CPU usage: {:.1}%", info.cpu_percent);

                // Fetch health info (without cascading) for troubleshooting
                match crate::health::collect_health("", vec![]).await {
                    Ok(health) => {
                        tracing::warn!("Health info at high CPU: {}", health);
                    }
                    Err(e) => {
                        tracing::error!("Failed to collect health info during high CPU: {}", e);
                    }
                }
            }

            // Check process memory usage
            if info.system_memory_total > 0 {
                let process_memory_mb = info.memory_bytes as f64 / 1_048_576.0;
                let process_memory_percent =
                    (info.memory_bytes as f64 / info.system_memory_total as f64) * 100.0;

                if process_memory_percent > 80.0 {
                    tracing::warn!(
                        "High process memory usage: {:.1}MB ({:.1}% of system memory)",
                        process_memory_mb,
                        process_memory_percent
                    );

                    // Fetch health info (without cascading) for troubleshooting
                    match crate::health::collect_health("", vec![]).await {
                        Ok(health) => {
                            tracing::warn!("Health info at high memory: {}", health);
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to collect health info during high memory: {}",
                                e
                            );
                        }
                    }
                } else if process_memory_mb > 0.0 {
                    tracing::debug!(
                        "Process stats: CPU {:.1}%, Memory {:.1}MB ({:.1}% of system)",
                        info.cpu_percent,
                        process_memory_mb,
                        process_memory_percent
                    );
                }
            }

            // Sleep for 10 seconds before next refresh
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });

    tracing::info!("System info monitor started - refreshing every 10 seconds");
}
