// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Job timing statistics collection
//!
//! This module tracks execution times for jobs and provides statistics like
//! min, max, mean, and median execution times.

use once_cell::sync::Lazy;
use std::sync::Mutex;

/// Statistics about job execution times
#[derive(Debug, Clone, Default)]
pub struct JobTimingStats {
    /// Minimum job execution time in milliseconds
    pub min_ms: f64,
    /// Maximum job execution time in milliseconds
    pub max_ms: f64,
    /// Mean (average) job execution time in milliseconds
    pub mean_ms: f64,
    /// Median job execution time in milliseconds
    pub median_ms: f64,
    /// Total number of jobs timed
    pub count: usize,
}

/// Global storage for job execution times
/// We keep a rolling window of the last 1000 job times
static JOB_TIMES: Lazy<Mutex<Vec<f64>>> = Lazy::new(|| Mutex::new(Vec::new()));

/// Maximum number of job times to keep in memory
const MAX_JOB_TIMES: usize = 1000;

/// Record a job execution time
///
/// Records the execution time in milliseconds. If the buffer exceeds MAX_JOB_TIMES,
/// the oldest times are removed to maintain a rolling window.
pub fn record_job_time(duration_ms: f64) {
    match JOB_TIMES.lock() {
        Ok(mut times) => {
            times.push(duration_ms);

            // Keep only the most recent MAX_JOB_TIMES entries
            if times.len() > MAX_JOB_TIMES {
                let excess = times.len() - MAX_JOB_TIMES;
                times.drain(0..excess);
            }
        }
        Err(e) => {
            tracing::error!("Failed to lock job times for recording: {}", e);
        }
    }
}

/// Get current job timing statistics
///
/// Calculates min, max, mean, and median from the recorded job times.
/// Returns default (zero) statistics if no jobs have been recorded.
pub fn get_stats() -> JobTimingStats {
    match JOB_TIMES.lock() {
        Ok(times) => {
            if times.is_empty() {
                return JobTimingStats::default();
            }

            // Calculate min and max
            let min_ms = times.iter().copied().fold(f64::INFINITY, f64::min);
            let max_ms = times.iter().copied().fold(f64::NEG_INFINITY, f64::max);

            // Calculate mean
            let sum: f64 = times.iter().sum();
            let mean_ms = sum / times.len() as f64;

            // Calculate median
            let mut sorted = times.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let median_ms = if sorted.len() % 2 == 0 {
                let mid = sorted.len() / 2;
                (sorted[mid - 1] + sorted[mid]) / 2.0
            } else {
                sorted[sorted.len() / 2]
            };

            JobTimingStats {
                min_ms,
                max_ms,
                mean_ms,
                median_ms,
                count: times.len(),
            }
        }
        Err(e) => {
            tracing::error!("Failed to lock job times for stats: {}", e);
            JobTimingStats::default()
        }
    }
}
