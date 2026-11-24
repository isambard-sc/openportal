// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Concrete implementation of the Lustre quota engine.

use anyhow::Result;
use bradshaw::Utc;
use greatwestern::grammar::{ProjectMapping, UserMapping};
use greatwestern::storage::{Quota, QuotaLimit, StorageSize, StorageUsage, Volume};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use templemeads::job::assert_not_expired;
use templemeads::Error;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::quotamarker::{MarkerLocation, MarkerRead, QuotaMarker};

use crate::volumeconfig::{ProjectVolumeConfig, UserVolumeConfig};

/// Quota ID strategy for generating unique lustre quota identifiers.
///
/// This defines how to compute the numeric quota ID that Lustre uses internally.
/// The strategy uses a format string where:
/// 1. Variable names (UID, GID) are replaced with their numeric values
/// 2. Expressions in curly braces `{...}` are evaluated as arithmetic operations
/// 3. The final result is parsed as the quota ID
///
/// ## Supported Variables:
/// - `UID` - User ID (must be provided if used in format)
/// - `GID` - Group ID (must be provided if used in format)
///
/// ## Expression Evaluation:
/// Expressions in `{...}` support:
/// - Simple numbers: `{5000}` → 5000
/// - Addition: `{GID+1000}` → evaluates GID + 1000
/// - Subtraction: `{UID-1483800000}` → evaluates UID - 1483800000
/// - Nested evaluation: Variables replaced first, then expressions evaluated
///
/// ## Examples:
/// - `"{GID}"` - Use the group ID directly for project quotas
/// - `"{UID-100000}01"` - Subtract offset from UID and append "01"
///   - For UID=100125: {100125-100000}01 → 12501
/// - `"{UID-100000}02"` - Subtract offset from UID and append "02"
///   - For UID=100125: {100125-100000}02 → 12502
/// - `"{GID+1000}"` - Add offset to GID
///   - For GID=5000: {5000+1000} → 6000
#[derive(Debug, Clone, Serialize)]
pub struct LustreIdStrategy {
    format: String,
}

// Custom deserialization to allow both forms:
// 1. Simple string: home = "{UID-100000}01"
// 2. Explicit format: home = { format = "{UID-100000}01" }
impl<'de> Deserialize<'de> for LustreIdStrategy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        struct LustreIdStrategyVisitor;

        impl<'de> Visitor<'de> for LustreIdStrategyVisitor {
            type Value = LustreIdStrategy;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string or a map with a 'format' field")
            }

            // Handle direct string form: "{UID-100000}01"
            fn visit_str<E>(self, value: &str) -> Result<LustreIdStrategy, E>
            where
                E: de::Error,
            {
                let strategy = LustreIdStrategy {
                    format: value.to_string(),
                };
                strategy.validate().map_err(de::Error::custom)?;
                Ok(strategy)
            }

            // Handle map form: { format = "{UID-100000}01" }
            fn visit_map<M>(self, mut map: M) -> Result<LustreIdStrategy, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut format = None;

                while let Some(key) = map.next_key::<String>()? {
                    if key == "format" {
                        if format.is_some() {
                            return Err(de::Error::duplicate_field("format"));
                        }
                        format = Some(map.next_value()?);
                    } else {
                        return Err(de::Error::unknown_field(&key, &["format"]));
                    }
                }

                let format = format.ok_or_else(|| de::Error::missing_field("format"))?;
                let strategy = LustreIdStrategy { format };
                strategy.validate().map_err(de::Error::custom)?;
                Ok(strategy)
            }
        }

        deserializer.deserialize_any(LustreIdStrategyVisitor)
    }
}

impl LustreIdStrategy {
    #[allow(dead_code)] // used by the tests
    pub fn new(format: &str) -> Self {
        Self {
            format: format.to_string(),
        }
    }

    /// Validate the format string to catch common mistakes
    ///
    /// Checks for:
    /// - Empty format strings
    /// - Mismatched braces
    /// - Invalid variable names (warns about common typos like $UID, uid, etc.)
    /// - Format strings that can't possibly produce a valid quota ID
    fn validate(&self) -> Result<(), Error> {
        let format = &self.format;

        // Check for empty format
        if format.trim().is_empty() {
            return Err(Error::Parse("Format string cannot be empty".to_string()));
        }

        // Check for mismatched braces
        let open_count = format.chars().filter(|&c| c == '{').count();
        let close_count = format.chars().filter(|&c| c == '}').count();
        if open_count != close_count {
            return Err(Error::Parse(format!(
                "Mismatched braces in format string '{}': {} opening, {} closing",
                format, open_count, close_count
            )));
        }

        // Check for common typos in variable names
        if format.contains("$UID") || format.contains("$GID") {
            return Err(Error::Parse(format!(
                "Invalid variable in format string '{}': use 'UID' or 'GID' without the $ prefix",
                format
            )));
        }

        if format.contains("uid") || format.contains("gid") {
            return Err(Error::Parse(format!(
                "Invalid variable in format string '{}': variables must be uppercase (UID, GID)",
                format
            )));
        }

        // Warn about spaces inside braces (likely a typo)
        if let Some(start) = format.find('{') {
            // Offsets come from `find`, so they are on character boundaries;
            // read via `get` anyway so a future change cannot turn into a
            // panic. See docs/specifications/security-review-2.md (R1).
            if let Some(end) = format.get(start..).and_then(|tail| tail.find('}')) {
                let expr = format.get(start + 1..start + end).unwrap_or_default();
                if expr.starts_with(' ') || expr.ends_with(' ') {
                    return Err(Error::Parse(format!(
                        "Expression in braces contains leading/trailing spaces: '{{{}}}'",
                        expr
                    )));
                }
            }
        }

        Ok(())
    }

    /// Compute the lustre quota ID for a user based on their UID and GID
    ///
    /// # Arguments
    /// * `uid` - The user's numeric ID
    /// * `gid` - The user's primary group numeric ID
    ///
    /// # Returns
    /// The computed quota ID as a u64
    ///
    /// # Errors
    /// Returns an error if:
    /// - The format string is invalid
    /// - Arithmetic operations result in negative or zero numbers
    /// - The expression required a UID or GID that was not provided
    /// - The resulting ID cannot be parsed as a number
    pub fn compute_id(&self, uid: Option<u32>, gid: Option<u32>) -> Result<u64, Error> {
        let mut format = self.format.clone();

        // Check if required variables are provided
        if format.contains("UID") && uid.is_none() {
            return Err(Error::Misconfigured(
                "UID is required for this quota ID strategy".to_string(),
            ));
        }

        if format.contains("GID") && gid.is_none() {
            return Err(Error::Misconfigured(
                "GID is required for this quota ID strategy".to_string(),
            ));
        }

        // Substitute all UID and GID with provided values
        let uid_str = uid.unwrap_or(0).to_string();
        let gid_str = gid.unwrap_or(0).to_string();

        format = format.replace("UID", &uid_str).replace("GID", &gid_str);

        // Find and evaluate all expressions in curly braces
        format = Self::evaluate_expressions(&format)?;

        // Convert final format to a number
        let lustre_id: u64 = format.parse().map_err(|e| {
            Error::Misconfigured(format!("Invalid quota ID format '{}': {}", format, e))
        })?;

        // Check for zero result
        if lustre_id == 0 {
            Err(Error::Failed(format!(
                "Quota ID computation resulted in zero value: {}",
                lustre_id
            )))
        } else {
            Ok(lustre_id)
        }
    }

    /// Find and evaluate all arithmetic expressions in curly braces
    ///
    /// Expressions are in the form {num+num}, {num-num}, or just {num}
    /// This function iteratively finds expressions, evaluates them, and replaces them
    /// with their results until no more expressions remain.
    fn evaluate_expressions(input: &str) -> Result<String, Error> {
        let mut result = input.to_string();

        // Keep evaluating until there are no more curly brace expressions
        while let Some(start) = result.find('{') {
            // Find the matching closing brace
            let end = match result.get(start..).and_then(|tail| tail.find('}')) {
                Some(pos) => start + pos,
                None => {
                    return Err(Error::Misconfigured(format!(
                        "Unmatched opening brace in expression: {}",
                        result
                    )))
                }
            };

            // Extract the expression content (without braces)
            let expr = result.get(start + 1..end).unwrap_or_default().to_string();
            let expr = expr.as_str();

            // Evaluate the expression
            let value = Self::evaluate_arithmetic(expr)?;

            // Replace the expression (including braces) with the result
            result.replace_range(start..=end, &value.to_string());
        }

        Ok(result)
    }

    /// Evaluate a simple arithmetic expression
    ///
    /// Supports addition (+) and subtraction (-) operations on integers
    /// This must not result in negative numbers
    fn evaluate_arithmetic(expr: &str) -> Result<u64, Error> {
        let expr = expr.trim();

        // Look for operators (+ or -)
        // We need to find the last operator to handle negative numbers correctly
        // e.g., "-100+50" should split as ["-100", "+", "50"]
        let mut operator_pos = None;
        let mut operator = ' ';

        // Scan from left to right, skipping the first character (which might
        // be a negative sign). `char_indices` (byte offsets), not
        // `chars().enumerate()` (character counts) - the two differ as soon as
        // the expression contains any multi-byte character, and slicing a
        // `&str` at an offset that is not a character boundary panics. See
        // docs/specifications/security-review-2.md (finding R1).
        for (i, ch) in expr.char_indices().skip(1) {
            if ch == '+' || ch == '-' {
                operator_pos = Some(i);
                operator = ch;
                // Don't break - keep looking for the rightmost operator
            }
        }

        if let Some(pos) = operator_pos {
            // Split at the operator. `get` rather than `[..]`, so a offset
            // that is somehow not on a character boundary is an error rather
            // than an abort.
            let (Some(left), Some(right)) = (expr.get(..pos), expr.get(pos + 1..)) else {
                return Err(Error::Misconfigured(format!(
                    "Could not split expression '{}' at its operator",
                    expr
                )));
            };
            let left = left.trim();
            let right = right.trim();

            // Parse both sides as integers
            let left_val = left.parse::<i64>().map_err(|e| {
                Error::Misconfigured(format!(
                    "Failed to parse left operand '{}' in expression '{}': {}",
                    left, expr, e
                ))
            })?;

            let right_val = right.parse::<i64>().map_err(|e| {
                Error::Misconfigured(format!(
                    "Failed to parse right operand '{}' in expression '{}': {}",
                    right, expr, e
                ))
            })?;

            // Perform the operation
            let result = match operator {
                '+' => left_val.checked_add(right_val).ok_or_else(|| {
                    Error::Failed(format!("Arithmetic overflow in expression '{}'", expr))
                })?,
                '-' => left_val.checked_sub(right_val).ok_or_else(|| {
                    Error::Failed(format!("Arithmetic underflow in expression '{}'", expr))
                })?,
                _ => unreachable!(),
            };

            if result < 0 {
                return Err(Error::Incompatible(format!(
                    "Expression '{}' evaluated to negative value {}",
                    expr, result
                )));
            }

            Ok(result as u64)
        } else {
            // No operator found, just parse as a number
            expr.parse::<u64>().map_err(|e| {
                Error::Misconfigured(format!(
                    "Failed to parse expression '{}' as number: {}",
                    expr, e
                ))
            })
        }
    }
}

/// Configuration for the Lustre quota engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LustreEngineConfig {
    /// Command to run for lfs operations (default: "lfs")
    /// Can include full path, sudo, or container exec as needed
    /// Examples: "lfs", "sudo lfs", "/usr/bin/lfs", "docker exec lustre lfs"
    #[serde(default = "default_lfs_command")]
    lfs_command: String,

    /// Maximum number of concurrent lfs commands to run (excluding setting
    /// Lustre IDs)
    #[serde(default = "default_max_runners")]
    max_runners: usize,

    /// Timeout in seconds for lfs commands (default: 30)
    #[serde(default = "default_command_timeout")]
    command_timeout_secs: u64,

    /// Timeout in seconds for recursive lfs project operations (default: 18,000 seconds / 5 hours)
    /// The `lfs project -srp` command can take a long time on large directory trees
    #[serde(default = "default_recursive_timeout")]
    recursive_timeout_secs: u64,

    /// Lustre project IDs that mean "no project ID has been assigned to this
    /// directory yet". There is no default: an operator must say which IDs these
    /// are, because guessing wrong either overwrites a quota or never applies one.
    ///
    /// A new directory inherits the project ID of its parent when the parent has
    /// the inherit (`P`) flag set. Sites tag the volume roots (e.g. `/scratch`,
    /// `/projects`) with a catch-all project such as `1`
    /// (`lfs project -p 1 -s /projects`), so every newly created user or project
    /// directory starts out as `1 P`. A directory still carrying one of these IDs
    /// has never been given its own quota ID, so it is treated as uninitialised:
    /// it has no quota of its own, and the volume's default quota may be applied
    /// to it.
    ///
    /// Only IDs read together with the inherit flag count. A directory under an
    /// untagged root reads as `0 -`, which `get_lustre_id_from_dir` reports as
    /// `None` - the same as a directory whose quota was explicitly cleared - so it
    /// is never unassigned, whatever is listed here.
    ///
    /// Unset, no ID counts as unassigned. That is valid, but a volume that uses
    /// this engine with a `default_quota` then refuses to start, since the default
    /// could never be applied; and at start-up each such volume's roots must carry
    /// one of these IDs with the inherit flag. See `check_default_quota_root`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unassigned_project_ids: Option<Vec<u64>>,

    /// Map from volume name to quota ID generation strategy
    ///
    /// Each volume that uses this Lustre engine must have an ID strategy defined.
    /// The strategy determines how to compute the numeric quota ID from user/group IDs.
    ///
    /// Example:
    /// ```toml
    /// [quota_engines.lustre]
    /// type = "lustre"
    /// lfs_command = "lfs"
    ///
    /// [quota_engines.lustre.id_strategies]
    /// home = "{UID-1483800000}01"
    /// scratch = "{UID-1483800000}02"
    /// projects = "{GID}"
    /// ```
    ///
    /// Or with the legacy explicit format:
    /// ```toml
    /// [quota_engines.lustre.id_strategies]
    /// home = { format = "{UID-1483800000}01" }
    /// ```
    #[serde(default, flatten)]
    id_strategies: HashMap<Volume, LustreIdStrategy>,
}

fn default_max_runners() -> usize {
    4
}

fn default_lfs_command() -> String {
    "lfs".to_string()
}

fn default_command_timeout() -> u64 {
    30
}

fn default_recursive_timeout() -> u64 {
    18000 // 5 hours
}

impl Default for LustreEngineConfig {
    fn default() -> Self {
        Self {
            lfs_command: default_lfs_command(),
            max_runners: default_max_runners(),
            command_timeout_secs: default_command_timeout(),
            recursive_timeout_secs: default_recursive_timeout(),
            unassigned_project_ids: None,
            id_strategies: HashMap::new(),
        }
    }
}

impl LustreEngineConfig {
    pub fn lfs_command(&self) -> &str {
        &self.lfs_command
    }

    pub fn command_timeout(&self) -> Duration {
        Duration::from_secs(self.command_timeout_secs)
    }

    pub fn recursive_timeout(&self) -> Duration {
        // use the maximum of the command timeout and recursive timeout to avoid premature timeouts
        Duration::from_secs(std::cmp::max(
            self.command_timeout_secs,
            self.recursive_timeout_secs,
        ))
    }

    pub fn max_runners(&self) -> usize {
        self.max_runners
    }

    pub fn get_id_strategy(&self, volume: &Volume) -> Option<&LustreIdStrategy> {
        self.id_strategies.get(volume)
    }

    /// The Lustre project IDs that mean "not assigned yet" - see the field docs.
    /// Empty when the operator has not set any.
    pub fn unassigned_project_ids(&self) -> &[u64] {
        self.unassigned_project_ids.as_deref().unwrap_or_default()
    }

    /// Whether the operator has set `unassigned_project_ids` at all.
    pub fn has_unassigned_project_ids(&self) -> bool {
        self.unassigned_project_ids.is_some()
    }
}

/// What the Lustre project ID read from a directory says about it, relative to
/// the quota ID it is expected to carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LustreIdState {
    /// The directory carries the expected quota ID.
    Assigned,

    /// The directory carries one of the "unassigned" IDs (by default 0, or an ID
    /// such as 1 inherited from its parent), so it has never been given its own.
    Unassigned(u64),

    /// The directory carries some other ID, or no project ID with the inherit flag
    /// (`None`) - e.g. it was explicitly cleared, or belongs to someone else.
    Mismatch(Option<u64>),
}

/// Classify the project ID read from a directory. The expected ID is checked
/// first, so a quota ID that happens to also be in `unassigned` still counts as
/// assigned.
pub fn classify_lustre_id(
    current: Option<u64>,
    expected: u64,
    unassigned: &[u64],
) -> LustreIdState {
    match current {
        Some(id) if id == expected => LustreIdState::Assigned,
        Some(id) if unassigned.contains(&id) => LustreIdState::Unassigned(id),
        other => LustreIdState::Mismatch(other),
    }
}

/// Whether or not to run in "dry_run" mode - in this mode, no lustre
/// commands will actually be run
fn is_dry_run() -> bool {
    match std::env::var("OPENPORTAL_LUSTRE_DRY_RUN") {
        Ok(val) => val == "1" || val.to_lowercase() == "true",
        Err(_) => false,
    }
}

/// Whether or not to run in Lustre ID mode only - in this mode,
/// only lustre IDs will be set - quotas will not be applied
fn is_lustre_id_only() -> bool {
    match std::env::var("OPENPORTAL_LUSTRE_ID_ONLY") {
        Ok(val) => val == "1" || val.to_lowercase() == "true",
        Err(_) => false,
    }
}

/// How many `lfs` commands may run at once, for each `lfs` command (that is,
/// each Lustre engine, or each set of engines that share a command), sized by
/// the engine's `max_runners`. This keeps the Lustre servers from being
/// overloaded. Recursive walks are not counted here - they run one at a time
/// on their own worker (see `WALKS`).
///
/// Each entry is keyed by the command, so an engine's commands always run its
/// own `lfs_command` - with more than one Lustre engine configured, one engine
/// can no longer end up running another's.
static LFS_LIMITS: Lazy<std::sync::Mutex<HashMap<String, LfsLimit>>> =
    Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
struct LfsLimit {
    max_runners: usize,
    slots: Arc<tokio::sync::Semaphore>,
}

/// Permission to run one `lfs` command, held while it runs.
#[derive(Debug)]
pub struct LockedRunner {
    lfs_command: String,
    _slot: tokio::sync::OwnedSemaphorePermit,
}

/// Allow `lfs` to run up to `max_lfs_runners` commands at once. Engines that
/// share a command share the limit; if they ask for different limits, the
/// larger is used.
pub fn set_command(lfs: &str, max_lfs_runners: usize) -> Result<(), Error> {
    // make sure we have at least one runner
    let max_lfs_runners = max_lfs_runners.max(1);

    let mut limits = LFS_LIMITS
        .lock()
        .map_err(|_| Error::Failed("The lfs runner lock is poisoned".to_string()))?;

    match limits.get_mut(lfs) {
        Some(limit) => {
            if max_lfs_runners > limit.max_runners {
                limit
                    .slots
                    .add_permits(max_lfs_runners.saturating_sub(limit.max_runners));
                limit.max_runners = max_lfs_runners;
            }

            tracing::debug!(
                "lfs command '{}' is shared by more than one engine, running at most {} at once",
                lfs,
                limit.max_runners
            );
        }
        None => {
            tracing::debug!(
                "Using command line lfs commands: {}, max_lfs_runners: {}",
                lfs,
                max_lfs_runners
            );

            limits.insert(
                lfs.to_string(),
                LfsLimit {
                    max_runners: max_lfs_runners,
                    slots: Arc::new(tokio::sync::Semaphore::new(max_lfs_runners)),
                },
            );
        }
    }

    Ok(())
}

impl LockedRunner {
    pub fn lfs(&self) -> &str {
        &self.lfs_command
    }

    async fn run_command(
        &self,
        args: &[&str],
        timeout_duration: Duration,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<String, Error> {
        if is_dry_run() {
            let cmd_string = format!("{} {}", self.lfs(), args.join(" "));
            tracing::info!("DRY RUN: {}", cmd_string);
            return Ok(String::new());
        }

        assert_not_expired(expires)?;

        let cmd_parts: Vec<&str> = self.lfs().split_whitespace().collect();
        let (program, initial_args) = cmd_parts
            .split_first()
            .ok_or_else(|| Error::Misconfigured("lfs_command is empty".to_string()))?;

        let command_string = format!("{} {}", program, args.join(" "));

        let mut command = Command::new(program);
        command.args(initial_args);
        command.args(args);

        tracing::info!("Executing lfs command: {}", command_string);

        let result = timeout(timeout_duration, command.output())
            .await
            .map_err(|_| {
                Error::Timeout(format!(
                    "lfs command timed out after {} seconds",
                    timeout_duration.as_secs()
                ))
            })?
            .map_err(|e| Error::Failed(format!("Failed to execute lfs command: {}", e)))?;

        if !result.status.success() {
            let stdout = String::from_utf8_lossy(&result.stdout);
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(Error::Failed(format!(
                "lfs command failed: stdout={} stderr={}",
                stdout.trim(),
                stderr.trim()
            )));
        }

        Ok(String::from_utf8_lossy(&result.stdout).to_string())
    }
}

/// Wait for a free slot to run `lfs`, for as long as the job has left.
pub async fn runner(lfs: &str, expires: &bradshaw::DateTime<Utc>) -> Result<LockedRunner, Error> {
    let slots = LFS_LIMITS
        .lock()
        .map_err(|_| Error::Failed("The lfs runner lock is poisoned".to_string()))?
        .get(lfs)
        .map(|limit| limit.slots.clone())
        .ok_or_else(|| Error::Call(format!("No LFS runners have been configured for '{}'", lfs)))?;

    assert_not_expired(expires)?;

    let remaining = (*expires - Utc::now()).to_std().unwrap_or(Duration::ZERO);

    match timeout(remaining, slots.acquire_owned()).await {
        Ok(Ok(slot)) => Ok(LockedRunner {
            lfs_command: lfs.to_string(),
            _slot: slot,
        }),
        Ok(Err(_)) => Err(Error::Failed(format!(
            "The lfs runners for '{}' have been shut down",
            lfs
        ))),
        Err(_) => Err(Error::Timeout(format!(
            "Timed out waiting for a free lfs runner for '{}'",
            lfs
        ))),
    }
}

/// How often a walk that is still running says so in the log.
const WALK_PROGRESS_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// How long before a job expires a caller stops waiting for a walk, so that it
/// can still report back that the walk is not finished.
const WALK_WAIT_MARGIN: Duration = Duration::from_secs(15);

/// What a recursive walk of a directory tree does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WalkKind {
    /// `lfs project -srp <id> <dir>`: give the tree the quota ID. Recorded in
    /// the directory's `.openportal_quota` marker - see `crate::quotamarker`.
    Assign(u64),

    /// `lfs project -Cr <dir>`: clear the tree's quota ID. Removes the marker.
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WalkStatus {
    Queued,
    Running,
    Finished,
    Failed(String),
}

impl WalkStatus {
    fn is_done(&self) -> bool {
        matches!(self, WalkStatus::Finished | WalkStatus::Failed(_))
    }
}

/// A recursive walk, queued or running.
#[derive(Debug)]
struct Walk {
    lfs_command: String,
    args: Vec<String>,
    directory: PathBuf,
    marker: MarkerLocation,
    kind: WalkKind,
    timeout: Duration,
    status: tokio::sync::watch::Sender<WalkStatus>,
}

impl Walk {
    fn describe(&self) -> String {
        match self.kind {
            WalkKind::Assign(id) => {
                format!("assigning quota ID {} to {}", id, self.directory.display())
            }
            WalkKind::Clear => format!("clearing the quota ID from {}", self.directory.display()),
        }
    }
}

/// Every walk that is queued or running, oldest first. A single worker runs
/// them one at a time, because each one scans a whole directory tree. A walk
/// stays here, with its status, until it has finished, so that a request for
/// the same walk - a resubmitted job - joins it rather than queueing another.
///
/// This lives only in memory. What survives a restart is the marker each walk
/// writes - see `crate::quotamarker`.
static WALKS: Lazy<Mutex<Vec<Arc<Walk>>>> = Lazy::new(|| Mutex::new(Vec::new()));

/// Wakes the worker when a walk is queued.
static WALK_QUEUED: Lazy<tokio::sync::Notify> = Lazy::new(tokio::sync::Notify::new);

/// Whether the worker has been started. It runs on a thread of its own, with
/// its own runtime, so that it lives as long as the process does whatever
/// runtime the job that first queued a walk was running on.
static WALK_WORKER_STARTED: Lazy<std::sync::Mutex<bool>> =
    Lazy::new(|| std::sync::Mutex::new(false));

fn start_walk_worker() -> Result<(), Error> {
    let mut started = WALK_WORKER_STARTED
        .lock()
        .map_err(|_| Error::Failed("The walk worker lock is poisoned".to_string()))?;

    if *started {
        return Ok(());
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Failed(format!("Could not start the walk worker: {}", e)))?;

    std::thread::Builder::new()
        .name("lustre-walks".to_string())
        .spawn(move || runtime.block_on(walk_worker()))
        .map_err(|e| Error::Failed(format!("Could not start the walk worker: {}", e)))?;

    *started = true;

    tracing::info!("Started the Lustre directory walk worker");

    Ok(())
}

async fn walk_worker() {
    loop {
        let next = WALKS
            .lock()
            .await
            .iter()
            .find(|walk| *walk.status.borrow() == WalkStatus::Queued)
            .cloned();

        let Some(walk) = next else {
            WALK_QUEUED.notified().await;
            continue;
        };

        walk.status.send_replace(WalkStatus::Running);

        let status = match run_walk(&walk).await {
            Ok(()) => WalkStatus::Finished,
            Err(e) => WalkStatus::Failed(e.to_string()),
        };

        // Publish the result before leaving the registry, so that anyone
        // who joins in between sees it rather than queueing a repeat.
        walk.status.send_replace(status);

        WALKS
            .lock()
            .await
            .retain(|queued| !Arc::ptr_eq(queued, &walk));
    }
}

/// Run one walk to completion, logging its start, its progress every
/// `WALK_PROGRESS_INTERVAL`, and how it ended and how long it took.
async fn run_walk(walk: &Walk) -> Result<(), Error> {
    let started_at = Utc::now();
    let clock = tokio::time::Instant::now();
    let description = walk.describe();

    tracing::info!(
        "Walk started: {} ({} {}), timeout {}s",
        description,
        walk.lfs_command,
        walk.args.join(" "),
        walk.timeout.as_secs()
    );

    if let WalkKind::Assign(id) = walk.kind {
        // Without this, an interrupted walk would later look finished - so
        // the walk does not start unless it can be recorded.
        crate::quotamarker::write_marker(&walk.marker, &QuotaMarker::started(id, started_at))
            .await
            .map_err(|e| {
                tracing::error!("Walk failed before it started: {}: {}", description, e);
                e
            })?;
    }

    let result = run_walk_command(walk, &description, clock).await;

    let elapsed = clock.elapsed().as_secs();

    match &result {
        Ok(()) => {
            let recorded = match walk.kind {
                WalkKind::Assign(id) => {
                    crate::quotamarker::write_marker(
                        &walk.marker,
                        &QuotaMarker::complete(id, Some(started_at), Utc::now()),
                    )
                    .await
                }
                WalkKind::Clear => crate::quotamarker::remove_marker(&walk.marker).await,
            };

            if let Err(e) = recorded {
                // The walk itself succeeded. The marker still says "started",
                // so the worst case is that the walk is repeated.
                tracing::warn!("Walk finished, but could not update its marker: {}", e);
            }

            tracing::info!("Walk finished after {}s: {}", elapsed, description);
        }
        Err(e) => {
            tracing::error!("Walk failed after {}s: {}: {}", elapsed, description, e);
        }
    }

    result
}

async fn run_walk_command(
    walk: &Walk,
    description: &str,
    clock: tokio::time::Instant,
) -> Result<(), Error> {
    let cmd_parts: Vec<&str> = walk.lfs_command.split_whitespace().collect();
    let (program, initial_args) = cmd_parts
        .split_first()
        .ok_or_else(|| Error::Misconfigured("lfs_command is empty".to_string()))?;

    let mut command = Command::new(program);
    command.args(initial_args);
    command.args(&walk.args);
    // so that a walk that times out is stopped, not left running unseen
    command.kill_on_drop(true);

    let output = command.output();
    tokio::pin!(output);

    let deadline = tokio::time::sleep(walk.timeout);
    tokio::pin!(deadline);

    let mut progress =
        tokio::time::interval_at(clock + WALK_PROGRESS_INTERVAL, WALK_PROGRESS_INTERVAL);

    let output = loop {
        tokio::select! {
            output = &mut output => break output,
            _ = &mut deadline => {
                return Err(Error::Timeout(format!(
                    "lfs did not finish within the recursive timeout of {}s",
                    walk.timeout.as_secs()
                )));
            }
            _ = progress.tick() => {
                tracing::info!(
                    "Walk still running after {} minutes: {}",
                    clock.elapsed().as_secs() / 60,
                    description
                );
            }
        }
    };

    let output = output.map_err(|e| Error::Failed(format!("Failed to execute lfs: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();

        // a walk of a large tree can report a line per file - keep the end
        let tail: String = {
            let chars: Vec<char> = stderr.chars().collect();
            let skip = chars.len().saturating_sub(2000);
            chars.into_iter().skip(skip).collect()
        };

        return Err(Error::Failed(format!(
            "lfs {} failed ({}): {}",
            walk.args.join(" "),
            output.status,
            tail
        )));
    }

    Ok(())
}

/// Queue a walk, or join the identical one that is already queued or running,
/// and return a handle on its status.
async fn enqueue_walk(
    lfs_command: &str,
    directory: &Path,
    marker: MarkerLocation,
    kind: WalkKind,
    timeout: Duration,
) -> Result<tokio::sync::watch::Receiver<WalkStatus>, Error> {
    let directory_str = directory
        .to_str()
        .ok_or_else(|| Error::Incompatible("Directory path contains invalid UTF-8".to_string()))?;

    let args: Vec<String> = match kind {
        WalkKind::Assign(id) => vec![
            "project".to_string(),
            "-srp".to_string(),
            id.to_string(),
            directory_str.to_string(),
        ],
        WalkKind::Clear => vec![
            "project".to_string(),
            "-Cr".to_string(),
            directory_str.to_string(),
        ],
    };

    start_walk_worker()?;

    let mut walks = WALKS.lock().await;

    if let Some(walk) = walks
        .iter()
        .find(|walk| walk.lfs_command == lfs_command && walk.args == args)
    {
        tracing::info!(
            "Joining the walk already {}: {}",
            match *walk.status.borrow() {
                WalkStatus::Queued => "queued",
                WalkStatus::Running => "running",
                _ => "finishing",
            },
            walk.describe()
        );
        return Ok(walk.status.subscribe());
    }

    let (status, receiver) = tokio::sync::watch::channel(WalkStatus::Queued);

    let walk = Arc::new(Walk {
        lfs_command: lfs_command.to_string(),
        args,
        directory: directory.to_path_buf(),
        marker,
        kind,
        timeout,
        status,
    });

    tracing::info!(
        "Walk queued behind {} other(s): {}",
        walks.len(),
        walk.describe()
    );

    walks.push(walk);
    drop(walks);

    WALK_QUEUED.notify_one();

    Ok(receiver)
}

/// Wait for a walk until shortly before `expires`. Returns `Ok(true)` if it
/// finished, `Ok(false)` if it is still queued or running (it carries on in the
/// background), and the error if it failed.
async fn wait_for_walk(
    mut status: tokio::sync::watch::Receiver<WalkStatus>,
    expires: &bradshaw::DateTime<Utc>,
) -> Result<bool, Error> {
    let remaining = (*expires - Utc::now())
        .to_std()
        .unwrap_or(Duration::ZERO)
        .saturating_sub(WALK_WAIT_MARGIN);

    let waited = tokio::time::timeout(remaining, async {
        status
            .wait_for(WalkStatus::is_done)
            .await
            .map(|status| status.clone())
    })
    .await;

    match waited {
        Ok(Ok(WalkStatus::Failed(e))) => Err(Error::Failed(e)),
        Ok(Ok(_)) => Ok(true),
        Ok(Err(_)) => Err(Error::Failed(
            "The directory walk ended without reporting a result".to_string(),
        )),
        Err(_) => Ok(false),
    }
}

/// Lustre filesystem quota engine implementation.
///
/// This engine uses the `lfs` command-line tool to manage quotas on Lustre filesystems.
/// Lustre supports both user and project quotas with separate tracking for block (storage)
/// and inode (file count) quotas.
pub struct LustreEngine {
    config: LustreEngineConfig,
}

impl LustreEngine {
    /// Create a new Lustre quota engine with the given configuration
    pub fn new(config: LustreEngineConfig) -> Result<Self> {
        Ok(Self { config })
    }

    /// Initialise the engine
    pub async fn initialize(&self) -> Result<(), Error> {
        set_command(self.config.lfs_command(), self.config.max_runners())?;
        tracing::info!(
            "LustreEngine initialized with command '{}' and max runners {}",
            self.config.lfs_command(),
            self.config.max_runners()
        );
        Ok(())
    }

    /// Check, at start-up, that a default quota on `volume` can ever be applied
    /// to the directories created under `root`.
    ///
    /// The default is applied only to a directory that still carries one of the
    /// `unassigned_project_ids`, and a new directory carries an ID only by
    /// inheriting it from its parent. So the root must carry a listed ID with the
    /// inherit flag (e.g. `1 P`). An untagged root (`0 -`) or one tagged with an
    /// unlisted ID would make every new directory look as if its quota had been
    /// set deliberately, and the default would silently never be applied - so the
    /// agent refuses to start instead, naming the fix.
    ///
    /// A root whose ID cannot be read is also refused: the check has not passed.
    pub async fn check_default_quota_root(
        &self,
        volume: &Volume,
        root: &Path,
    ) -> Result<(), Error> {
        if !self.config.has_unassigned_project_ids() {
            return Err(Error::Misconfigured(format!(
                "Volume '{}' has a default_quota on a Lustre quota engine, but the engine \
                 does not set unassigned_project_ids. The default can only be applied to a \
                 directory carrying one of those IDs, so say which they are - typically the \
                 ID the volume roots are tagged with, e.g. unassigned_project_ids = [1].",
                volume
            )));
        }

        if is_dry_run() {
            tracing::info!(
                "DRY RUN: not checking the Lustre project ID of root {} of volume '{}'",
                root.display(),
                volume
            );
            return Ok(());
        }

        let expires = Utc::now()
            + bradshaw::Duration::seconds(
                i64::try_from(self.config.command_timeout_secs)
                    .unwrap_or(i64::MAX / 2)
                    .saturating_add(30)
                    .min(86400),
            );

        let unassigned = self.config.unassigned_project_ids();

        let current = self
            .get_lustre_id_from_dir(root, &expires)
            .await
            .map_err(|e| {
                Error::Misconfigured(format!(
                    "Could not read the Lustre project ID of root {} of volume '{}', which \
                     must be checked because the volume has a default_quota: {}",
                    root.display(),
                    volume,
                    e
                ))
            })?;

        let fix_id = unassigned.first().copied().unwrap_or(1);

        match current {
            Some(id) if unassigned.contains(&id) => {
                tracing::info!(
                    "Root {} of volume '{}' carries unassigned project ID {} with the inherit \
                     flag, so new directories under it can be given the default quota.",
                    root.display(),
                    volume,
                    id
                );
                Ok(())
            }
            Some(id) => Err(Error::Misconfigured(format!(
                "Root {} of volume '{}' carries Lustre project ID {}, which is not in \
                 unassigned_project_ids {:?}. Every new directory under it inherits {}, so the \
                 volume's default_quota would never be applied. Either add {} to \
                 unassigned_project_ids, or retag the root with \
                 'lfs project -p {} -s {}'.",
                root.display(),
                volume,
                id,
                unassigned,
                id,
                id,
                fix_id,
                root.display()
            ))),
            None => Err(Error::Misconfigured(format!(
                "Root {} of volume '{}' has no Lustre project ID with the inherit flag \
                 ('lfs project -d' reports '0 -'). New directories under it would carry no \
                 project ID, which cannot be told apart from one whose quota was cleared, so \
                 the volume's default_quota would never be applied. Tag the root with \
                 'lfs project -p {} -s {}'.",
                root.display(),
                volume,
                fix_id,
                root.display()
            ))),
        }
    }

    fn get_id_strategy(&self, volume: &Volume) -> Result<&LustreIdStrategy, Error> {
        self.config.id_strategies.get(volume).ok_or_else(|| {
            Error::Misconfigured(format!(
                "No Lustre quota ID strategy defined for volume '{}'",
                volume
            ))
        })
    }

    /// Get the UID for a username.
    ///
    /// Delegates to `crate::nameservice`, which is the workspace's single name-lookup
    /// path - the comments there explain why it must not be libc, and why it
    /// distinguishes a name that does not exist from one it could not look up. This
    /// previously ran `id -u` and parsed its output, which could not tell those apart.
    async fn get_uid(&self, username: &str) -> Result<u32, Error> {
        crate::nameservice::resolve_uid(username).await
    }

    /// Get the GID for a group name.
    ///
    /// Delegates to `crate::nameservice` - see `get_uid`.
    async fn get_gid(&self, groupname: &str) -> Result<u32, Error> {
        crate::nameservice::resolve_gid(groupname).await
    }

    /// Compute the quota ID for a user on a specific volume
    async fn compute_user_quota_id(
        &self,
        volume: &Volume,
        mapping: &UserMapping,
    ) -> Result<u64, Error> {
        let strategy = self.get_id_strategy(volume)?;

        let uid = self.get_uid(mapping.local_user().unix()?).await?;
        let gid = self.get_gid(mapping.local_group()).await?;

        strategy.compute_id(Some(uid), Some(gid))
    }

    /// Compute the quota ID for a project on a specific volume
    async fn compute_project_quota_id(
        &self,
        volume: &Volume,
        mapping: &ProjectMapping,
    ) -> Result<u64, Error> {
        let strategy = self.get_id_strategy(volume)?;

        let gid = self.get_gid(mapping.local_group()).await?;

        strategy.compute_id(None, Some(gid))
    }

    /// Execute a lfs command with timeout
    async fn run_lfs_command(
        &self,
        args: &[&str],
        timeout_duration: Duration,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<String, Error> {
        if is_dry_run() {
            let cmd_string = format!("{} {}", self.config.lfs_command(), args.join(" "));
            tracing::info!("DRY RUN: {}", cmd_string);
            return Ok(String::new());
        }

        let locked_runner = runner(self.config.lfs_command(), expires).await?;
        let output = locked_runner
            .run_command(args, timeout_duration, expires)
            .await?;
        Ok(output)
    }

    /// Walk `directory`'s tree in the background (see `WALKS`), joining the
    /// same walk if it is already queued or running, and wait for it until
    /// shortly before `expires`.
    ///
    /// Returns `Ok(true)` once the walk has finished, and `Ok(false)` if it is
    /// still queued or running - it carries on, and a resubmitted job joins it.
    async fn queue_walk(
        &self,
        root: &Path,
        directory: &Path,
        kind: WalkKind,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<bool, Error> {
        assert_not_expired(expires)?;

        if is_dry_run() {
            tracing::info!(
                "DRY RUN: {} project {:?} {}",
                self.config.lfs_command(),
                kind,
                directory.display()
            );
            return Ok(true);
        }

        let status = enqueue_walk(
            self.config.lfs_command(),
            directory,
            MarkerLocation::new(root, directory)?,
            kind,
            self.config.recursive_timeout(),
        )
        .await?;

        let finished = wait_for_walk(status, expires).await?;

        if !finished {
            tracing::info!(
                "The walk of {} has not finished yet - it continues in the background.",
                directory.display()
            );
        }

        Ok(finished)
    }

    /// Get the current Lustre ID assigned to a directory
    ///
    /// Uses `lfs project -d <lustre_id>` to query the Lustre ID
    /// Returns None if no Lustre ID is set
    async fn get_lustre_id_from_dir(
        &self,
        directory: &Path,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<Option<u64>, Error> {
        let output = self
            .run_lfs_command(
                &[
                    "project",
                    "-d",
                    directory.to_str().ok_or_else(|| {
                        Error::Incompatible("Directory path contains invalid UTF-8".to_string())
                    })?,
                ],
                self.config.command_timeout(),
                expires,
            )
            .await?;

        // Parse output like "1234 P /path/to/directory"
        // or just "0 - /path/to/directory" if no project ID is set
        for line in output.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let (Some(id_str), Some(flag)) = (parts.first(), parts.get(1)) {
                let (id_str, flag) = (*id_str, *flag);

                // If flag is 'P', a project ID is set
                if flag == "P" {
                    let id = id_str.parse::<u64>().map_err(|e| {
                        Error::Parse(format!("Failed to parse project ID '{}': {}", id_str, e))
                    })?;
                    return Ok(Some(id));
                }
            }
        }

        Ok(None)
    }

    /// Clear the Lustre ID from a directory recursively
    ///
    /// Uses `lfs project -Cr <directory>` to recursively clear the Lustre ID
    /// This operation can be very slow on large directory trees
    async fn clear_lustre_id(
        &self,
        root: &Path,
        directory: &Path,
        lustre_id: u64,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<bool, Error> {
        tracing::info!(
            "Clearing Lustre ID from directory {} (this may take a while)",
            directory.display()
        );

        let current_id = self.get_lustre_id_from_dir(directory, expires).await?;

        if current_id.is_none()
            || matches!(
                classify_lustre_id(current_id, lustre_id, self.config.unassigned_project_ids()),
                LustreIdState::Unassigned(_)
            )
        {
            tracing::info!(
                "Directory {} does not have its own Lustre ID set ({:?}), skipping clear",
                directory.display(),
                current_id
            );
            return Ok(true);
        }

        if current_id != Some(lustre_id) {
            tracing::info!(
                "Directory {} has Lustre ID {}, which does not match expected ID {}, skipping clear",
                directory.display(),
                current_id.unwrap_or(0),
                lustre_id
            );
            return Ok(true);
        }

        self.queue_walk(root, directory, WalkKind::Clear, expires)
            .await
    }

    /// Assign a Lustre ID to a directory recursively
    ///
    /// Uses `lfs project -srp <id> <directory>` to recursively set the Lustre ID.
    /// This can take hours on a large tree, so it runs in the background (see
    /// `queue_walk`), and `false` is returned if it has not finished by the time
    /// the job must reply.
    ///
    /// The walk tags the top directory first, so the top directory carrying the
    /// ID does not mean the walk finished. The directory's marker decides - see
    /// `crate::quotamarker`: a `complete` marker for this ID means done, and a
    /// `started` marker, one for another ID, or one that cannot be trusted means
    /// the walk was interrupted, so it is walked again. No marker at all means
    /// the ID was assigned before markers existed, which is taken as done and
    /// recorded.
    async fn assign_lustre_id(
        &self,
        root: &Path,
        directory: &Path,
        lustre_id: u64,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<bool, Error> {
        let current_id = self.get_lustre_id_from_dir(directory, expires).await?;

        if current_id == Some(lustre_id) && !is_dry_run() {
            let location = MarkerLocation::new(root, directory)?;

            match crate::quotamarker::read_marker(&location).await {
                MarkerRead::Ours(marker) if marker.is_complete_for(lustre_id) => {
                    tracing::debug!(
                        "Directory {} already has Lustre ID {} ({})",
                        directory.display(),
                        lustre_id,
                        marker
                    );
                    return Ok(true);
                }
                MarkerRead::Absent => {
                    tracing::info!(
                        "Directory {} already has Lustre ID {} and no walk marker, so the ID \
                         was assigned before markers were kept - treating it as complete.",
                        directory.display(),
                        lustre_id
                    );

                    if let Err(e) = crate::quotamarker::write_marker(
                        &location,
                        &QuotaMarker::complete(lustre_id, None, Utc::now()),
                    )
                    .await
                    {
                        tracing::warn!("Could not record the earlier assignment: {}", e);
                    }

                    return Ok(true);
                }
                MarkerRead::Ours(marker) => {
                    tracing::info!(
                        "Directory {} has Lustre ID {}, but its marker records a {} - walking \
                         it (again) so that the whole tree carries the ID.",
                        directory.display(),
                        lustre_id,
                        marker
                    );
                }
                MarkerRead::Untrusted(reason) => {
                    tracing::warn!(
                        "Directory {} has Lustre ID {}, but its walk marker is ignored ({}) - \
                         walking it again so that the whole tree carries the ID.",
                        directory.display(),
                        lustre_id,
                        reason
                    );
                }
            }
        } else if current_id == Some(lustre_id) {
            return Ok(true);
        } else {
            tracing::info!(
                "Assigning Lustre ID {} to directory {} (currently {:?})",
                lustre_id,
                directory.display(),
                current_id
            );
        }

        self.queue_walk(root, directory, WalkKind::Assign(lustre_id), expires)
            .await
    }

    /// Clear quota limits for a project ID
    ///
    /// Uses `lfs setquota -p <id> -D <mount>`
    async fn clear_lustre_quota_limits(
        &self,
        project_id: u64,
        mount_point: &str,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<(), Error> {
        tracing::info!(
            "Clearing quota limits for project ID {} on mount {}",
            project_id,
            mount_point
        );

        if is_dry_run() || is_lustre_id_only() {
            tracing::info!(
                "DRY RUN: {} setquota -p {} -D {}",
                self.config.lfs_command(),
                project_id,
                mount_point
            );
            return Ok(());
        }

        self.run_lfs_command(
            &["setquota", "-p", &project_id.to_string(), "-D", mount_point],
            self.config.command_timeout(),
            expires,
        )
        .await?;

        Ok(())
    }

    /// Set quota limits for a project ID
    ///
    /// Uses `lfs setquota -p <id> -B <hard_limit> -I <inode_limit> <mount>`
    async fn set_lustre_quota_limits(
        &self,
        project_id: u64,
        limit: &QuotaLimit,
        inode_limit: u64,
        mount_point: &str,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<(), Error> {
        let block_limit = match limit {
            QuotaLimit::Unlimited => "0".to_string(), // 0 means unlimited in Lustre
            QuotaLimit::Limited(storage) => {
                // Convert to MB (Lustre uses MB units)
                let mb = storage.as_megabytes();
                format!("{}M", mb)
            }
        };

        tracing::info!(
            "Set quota limits for project ID {}: {} bytes, {} inodes",
            project_id,
            block_limit,
            inode_limit
        );

        if is_dry_run() || is_lustre_id_only() {
            tracing::info!(
                "DRY RUN: {} setquota -p {} -B {} -I {}k {}",
                self.config.lfs_command(),
                project_id,
                block_limit,
                inode_limit / 1000,
                mount_point
            );
            return Ok(());
        }

        self.run_lfs_command(
            &[
                "setquota",
                "-p",
                &project_id.to_string(),
                "-B",
                &block_limit,
                "-I",
                &format!("{}k", inode_limit / 1000), // Lustre uses k suffix for thousands
                mount_point,
            ],
            self.config.command_timeout(),
            expires,
        )
        .await?;

        Ok(())
    }

    /// Query quota usage and limits for a project ID
    ///
    /// Uses `lfs quota -p <id> <mount>` to query quota information
    async fn query_project_quota(
        &self,
        project_id: u64,
        mount_point: &str,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<Quota, Error> {
        if is_dry_run() || is_lustre_id_only() {
            tracing::info!(
                "DRY RUN: {} quota -p {} {}",
                self.config.lfs_command(),
                project_id,
                mount_point
            );
            return Ok(Quota::with_usage(
                QuotaLimit::Unlimited,
                StorageUsage::new(StorageSize::from_kilobytes(0.0)),
            ));
        }

        let output = self
            .run_lfs_command(
                &["quota", "-p", &project_id.to_string(), mount_point],
                self.config.command_timeout(),
                expires,
            )
            .await?;

        self.parse_quota_output(project_id, mount_point, &output)
    }

    /// Query quota usage and limits for a user ID
    ///
    /// Uses `lfs quota -p <id> <mount>` to query quota information
    async fn query_user_quota(
        &self,
        user_id: u64,
        mount_point: &str,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<Quota, Error> {
        if is_dry_run() || is_lustre_id_only() {
            tracing::info!(
                "DRY RUN: {} quota -p {} {}",
                self.config.lfs_command(),
                user_id,
                mount_point
            );
            return Ok(Quota::with_usage(
                QuotaLimit::Unlimited,
                StorageUsage::new(StorageSize::from_kilobytes(0.0)),
            ));
        }

        let output = self
            .run_lfs_command(
                &["quota", "-p", &user_id.to_string(), mount_point],
                self.config.command_timeout(),
                expires,
            )
            .await?;

        self.parse_quota_output(user_id, mount_point, &output)
    }

    /// Parse lfs quota command output
    ///
    /// The output format is typically:
    /// ```
    /// Disk quotas for prj 12345 (pid 12345):
    ///      Filesystem  kbytes   quota   limit   grace   files   quota   limit   grace
    ///       /scratch  1234567       0 5242880       -    5678       0 1000000       -
    /// ```
    fn parse_quota_output(
        &self,
        lustre_id: u64,
        mount_point: &str,
        output: &str,
    ) -> Result<Quota, Error> {
        if is_dry_run() || is_lustre_id_only() {
            tracing::debug!(
                "DRY RUN or ID ONLY: skipping parsing of quota output for Lustre ID {} on mount {}",
                lustre_id,
                mount_point
            );
            return Ok(Quota::with_usage(
                QuotaLimit::Unlimited,
                StorageUsage::new(StorageSize::from_kilobytes(0.0)),
            ));
        }

        // Find the line with actual quota data - this is directly after the header line
        let mut found_id = false;
        let mut found_header = false;

        for line in output.lines() {
            let trimmed = line.trim();
            let lower_trimmed = trimmed.to_lowercase();

            // see if this line contains the lustre id
            if !found_id && lower_trimmed.starts_with("disk quotas for") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 5 {
                    // parts[4] = id
                    match parts.get(4).copied().unwrap_or_default().parse::<u64>() {
                        Ok(id) => {
                            if id == lustre_id {
                                found_id = true;
                                tracing::info!(
                                    "Matching quota ID found ({}), processing line: {}",
                                    lustre_id,
                                    trimmed
                                );
                            } else {
                                tracing::warn!(
                                    "Lustre ID mismatch in quota output for mount {}: expected {}, found {}",
                                    mount_point,
                                    lustre_id,
                                    id
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to parse quota id from line '{}': {}",
                                trimmed,
                                e
                            );
                        }
                    }
                } else {
                    tracing::warn!("Quota ID line has insufficient columns: {}", trimmed);
                }

                continue;
            }

            // we can't process anything else until we have the id
            if !found_id {
                tracing::info!(
                    "Skipping quota output line until matching ID found: {}",
                    trimmed
                );
                continue;
            }

            // now look for the header line for the filesystem data
            if lower_trimmed.starts_with("filesystem") {
                // double check that header contains expected columns
                if lower_trimmed.contains("quota") && lower_trimmed.contains("limit") {
                    found_header = true;
                    tracing::info!(
                        "Found quota header line for Lustre ID {}: {}",
                        lustre_id,
                        trimmed
                    );
                } else {
                    tracing::warn!("Quota header line missing expected columns: {}", trimmed);
                }
                continue;
            }

            if !found_header {
                tracing::info!("Skipping quota output line until header found: {}", trimmed);
                continue;
            }

            // the next line we process should be the data line
            let parts: Vec<&str> = trimmed.split_whitespace().collect();

            if parts.len() < 4 {
                tracing::warn!("Quota data line has insufficient columns: {}", trimmed);
                return Err(Error::Parse(format!(
                    "Unable to parse quota data line: {}",
                    trimmed
                )));
            }

            // parts[0] = filesystem. Read via `get` throughout - this is
            // `lfs quota` output, not ours. See
            // docs/specifications/security-review-2.md (finding R1).
            let filesystem_str = parts.first().copied().unwrap_or_default();
            if filesystem_str != mount_point {
                tracing::warn!(
                    "Filesystem mount point mismatch in quota output: expected {}, found {}",
                    mount_point,
                    filesystem_str
                );

                continue;
            }

            // parts[1] = current kbytes usage
            // parts[3] = hard limit in kbytes (0 = unlimited)
            // Note: Lustre adds a '*' suffix to values that exceed quota, so we strip it
            let usage_field = parts.get(1).copied().unwrap_or_default();
            let usage_str = usage_field.trim_end_matches('*');
            let usage_kb = usage_str.parse::<u64>().map_err(|e| {
                Error::Parse(format!("Failed to parse usage '{}': {}", usage_field, e))
            })?;

            let limit_field = parts.get(3).copied().unwrap_or_default();
            let limit_str = limit_field.trim_end_matches('*');
            let limit_kb = limit_str.parse::<u64>().map_err(|e| {
                Error::Parse(format!("Failed to parse limit '{}': {}", limit_field, e))
            })?;

            tracing::info!(
                "Parsed quota for Lustre ID {} on mount {}: usage={} KB, limit={} KB from line: {}",
                lustre_id,
                mount_point,
                usage_kb,
                limit_kb,
                trimmed
            );

            let usage = StorageUsage::new(StorageSize::from_kilobytes(usage_kb as f64));
            let limit = if limit_kb == 0 {
                QuotaLimit::Unlimited
            } else {
                QuotaLimit::Limited(StorageSize::from_kilobytes(limit_kb as f64))
            };

            return Ok(Quota::with_usage(limit, usage));
        }

        // If we reach here, we did not find valid quota data
        Err(Error::Parse(
            "Unable to parse quota output: no valid data found".to_string(),
        ))
    }

    pub async fn clear_user_quota(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        volume_config: &UserVolumeConfig,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<(), Error> {
        let user = mapping.local_user();

        // Compute the quota ID for this user on this volume
        let quota_id = self.compute_user_quota_id(volume, mapping).await?;

        tracing::info!(
            "LustreEngine::clear_user_quota: user={}, volume={}, quota_id={}",
            user,
            volume,
            quota_id
        );

        let mount_point = volume_config.mount_point().ok_or_else(|| {
            Error::Misconfigured(format!(
                "Volume '{}' does not have a mount_point configured, which is required for Lustre quotas",
                volume
            ))
        })?;

        for path_config in volume_config.path_configs() {
            let path = path_config.path(mapping.clone().into())?;
            self.clear_lustre_id(Path::new(path_config.root()), &path, quota_id, expires)
                .await?;
        }

        if !is_lustre_id_only() {
            // clear the quota limits
            self.clear_lustre_quota_limits(quota_id, mount_point, expires)
                .await?;
        }

        Ok(())
    }

    pub async fn set_user_quota(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        volume_config: &UserVolumeConfig,
        limit: &QuotaLimit,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<Quota, Error> {
        let user = mapping.local_user();

        // Compute the quota ID for this user on this volume
        let quota_id = self.compute_user_quota_id(volume, mapping).await?;

        tracing::info!(
            "LustreEngine::set_user_quota: user={}, volume={}, quota_id={}, limit={}",
            user,
            volume,
            quota_id,
            limit
        );

        // Validate that the limit does not exceed the maximum quota for this volume
        if let Some(max_quota) = volume_config.max_quota() {
            if limit > max_quota {
                return Err(Error::Failed(format!(
                    "Requested quota limit ({}) exceeds maximum allowed quota ({}) for user {} on volume {}",
                    limit, max_quota, user, volume
                )));
            }
        }

        // Get mount point (required for Lustre operations)
        let mount_point = volume_config.mount_point().ok_or_else(|| {
            Error::Misconfigured(format!(
                "Volume '{}' does not have a mount_point configured, which is required for Lustre quotas",
                volume
            ))
        })?;

        // Get inode limit (use configured value or large default)
        let inode_limit = volume_config.default_inode_limit().unwrap_or(1_000_000);

        // The limit belongs to the quota ID, not to the files, so it is set
        // first: it then applies to each file as soon as the walk below tags it,
        // and a walk that takes hours does not hold the limit back.
        if !is_lustre_id_only() {
            self.set_lustre_quota_limits(quota_id, limit, inode_limit, mount_point, expires)
                .await?;
        }

        let mut walking = Vec::new();

        for path_config in volume_config.path_configs() {
            let path = path_config.path(mapping.clone().into())?;
            if !self
                .assign_lustre_id(Path::new(path_config.root()), &path, quota_id, expires)
                .await?
            {
                walking.push(path.display().to_string());
            }
        }

        if !walking.is_empty() {
            return Err(Error::NotReady(format!(
                "The quota limit for quota ID {} is set, but the ID is still being assigned \
                 to {} in the background. Resubmit to check again.",
                quota_id,
                walking.join(", ")
            )));
        }

        if is_lustre_id_only() {
            tracing::info!(
                "Skipping quota limit setting due to OPENPORTAL_LUSTRE_ID_ONLY environment variable"
            );
            return Ok(Quota::with_usage(
                QuotaLimit::Unlimited,
                StorageUsage::from(0),
            ));
        }

        self.query_user_quota(quota_id, mount_point, expires).await
    }

    pub async fn clear_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<(), Error> {
        let project = mapping.project().project();

        // Compute the quota ID for this project on this volume
        let quota_id = self.compute_project_quota_id(volume, mapping).await?;

        tracing::info!(
            "LustreEngine::clear_project_quota: project={}, volume={}, quota_id={}",
            project,
            volume,
            quota_id
        );

        // Get mount point (required for Lustre operations)
        let mount_point = volume_config.mount_point().ok_or_else(|| {
            Error::Misconfigured(format!(
                "Volume '{}' does not have a mount_point configured, which is required for Lustre quotas",
                volume
            ))
        })?;

        for path_config in volume_config.path_configs() {
            let path = path_config.path(mapping.clone().into())?;
            self.clear_lustre_id(Path::new(path_config.root()), &path, quota_id, expires)
                .await?;
        }

        if !is_lustre_id_only() {
            // clear the quota limits
            self.clear_lustre_quota_limits(quota_id, mount_point, expires)
                .await?;
        }

        Ok(())
    }

    pub async fn set_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
        limit: &QuotaLimit,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<Quota, Error> {
        let project = mapping.project().project();

        // Compute the quota ID for this project on this volume
        let quota_id = self.compute_project_quota_id(volume, mapping).await?;

        tracing::info!(
            "LustreEngine::set_project_quota: project={}, volume={}, quota_id={}, limit={}",
            project,
            volume,
            quota_id,
            limit
        );

        // Validate that the limit does not exceed the maximum quota for this volume
        if let Some(max_quota) = volume_config.max_quota() {
            if limit > max_quota {
                return Err(Error::Failed(format!(
                    "Requested quota limit ({}) exceeds maximum allowed quota ({}) for project {} on volume {}",
                    limit, max_quota, project, volume
                )));
            }
        }

        // Get mount point (required for Lustre operations)
        let mount_point = volume_config.mount_point().ok_or_else(|| {
            Error::Misconfigured(format!(
                "Volume '{}' does not have a mount_point configured, which is required for Lustre quotas",
                volume
            ))
        })?;

        // Get inode limit (use configured value or large default)
        let inode_limit = volume_config.default_inode_limit().unwrap_or(1_000_000);

        // The limit belongs to the quota ID, not to the files, so it is set
        // first: it then applies to each file as soon as the walk below tags it,
        // and a walk that takes hours does not hold the limit back.
        if !is_lustre_id_only() {
            self.set_lustre_quota_limits(quota_id, limit, inode_limit, mount_point, expires)
                .await?;
        }

        let mut walking = Vec::new();

        for path_config in volume_config.path_configs() {
            let path = path_config.project_path(mapping)?;
            if !self
                .assign_lustre_id(Path::new(path_config.root()), &path, quota_id, expires)
                .await?
            {
                walking.push(path.display().to_string());
            }
        }

        if !walking.is_empty() {
            return Err(Error::NotReady(format!(
                "The quota limit for quota ID {} is set, but the ID is still being assigned \
                 to {} in the background. Resubmit to check again.",
                quota_id,
                walking.join(", ")
            )));
        }

        if is_lustre_id_only() {
            tracing::info!(
                "Skipping quota limit setting due to OPENPORTAL_LUSTRE_ID_ONLY environment variable"
            );
            return Ok(Quota::with_usage(
                QuotaLimit::Unlimited,
                StorageUsage::from(0),
            ));
        }

        self.query_project_quota(quota_id, mount_point, expires)
            .await
    }

    /// The quota set on this user's quota ID, whether or not their directories
    /// carry that ID yet. Unlike `get_user_quota`, which reports a user whose
    /// directories have not been assigned the ID as unlimited, this says
    /// whether a limit is already in place for the ID.
    pub async fn get_user_quota_by_id(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        volume_config: &UserVolumeConfig,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<Quota, Error> {
        let quota_id = self.compute_user_quota_id(volume, mapping).await?;

        let mount_point = volume_config.mount_point().ok_or_else(|| {
            Error::Misconfigured(format!(
                "Volume '{}' does not have a mount_point configured, which is required for Lustre quotas",
                volume
            ))
        })?;

        self.query_user_quota(quota_id, mount_point, expires).await
    }

    /// The quota set on this project's quota ID, whether or not its
    /// directories carry that ID yet - see `get_user_quota_by_id`.
    pub async fn get_project_quota_by_id(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<Quota, Error> {
        let quota_id = self.compute_project_quota_id(volume, mapping).await?;

        let mount_point = volume_config.mount_point().ok_or_else(|| {
            Error::Misconfigured(format!(
                "Volume '{}' does not have a mount_point configured, which is required for Lustre quotas",
                volume
            ))
        })?;

        self.query_project_quota(quota_id, mount_point, expires)
            .await
    }

    async fn check_user_lustre_ids(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        volume_config: &UserVolumeConfig,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<u64, Error> {
        let user = mapping.local_user();

        // Compute the expected quota ID for this user on this volume
        let quota_id = self.compute_user_quota_id(volume, mapping).await?;

        let mut some_unassigned = false;
        let mut some_assigned = false;

        // loop over all path configs and check lustre ids
        for path_config in volume_config.path_configs() {
            let path = path_config.path(mapping.clone().into())?;

            // Check if the directory has the correct Lustre ID assigned
            let current_lustre_id = self.get_lustre_id_from_dir(&path, expires).await?;

            match classify_lustre_id(
                current_lustre_id,
                quota_id,
                self.config.unassigned_project_ids(),
            ) {
                LustreIdState::Assigned => some_assigned = true,
                LustreIdState::Unassigned(id) => {
                    tracing::warn!(
                        "Directory {} for user '{}' has an unassigned Lustre ID ({}) set.",
                        path.display(),
                        user,
                        id
                    );
                    some_unassigned = true;
                }
                LustreIdState::Mismatch(_) if !is_dry_run() => {
                    return Err(Error::Incompatible(format!(
                        "Directory {} for user '{}' has Lustre ID {:?}, expected {}. Please run set_user_quota first to assign the correct Lustre ID.",
                        path.display(),
                        user,
                        current_lustre_id,
                        quota_id
                    )));
                }
                LustreIdState::Mismatch(_) => {}
            }
        }

        if some_unassigned && !some_assigned {
            // return 0 as the user is effectively uninitialized
            Ok(0)
        } else {
            // At least one directory already carries the quota ID, so any quota set
            // on it is real and must be read rather than reported as unlimited -
            // otherwise a configured limit could be overwritten by a default.
            Ok(quota_id)
        }
    }

    /// Whether any of this user's directories on the volume still carries an
    /// unassigned Lustre project ID (see `unassigned_project_ids`), i.e. has never
    /// been given its own quota ID.
    pub async fn user_dirs_unassigned(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        volume_config: &UserVolumeConfig,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<bool, Error> {
        let quota_id = self.compute_user_quota_id(volume, mapping).await?;

        for path_config in volume_config.path_configs() {
            let path = path_config.path(mapping.clone().into())?;
            let current_lustre_id = self.get_lustre_id_from_dir(&path, expires).await?;

            if let LustreIdState::Unassigned(_) = classify_lustre_id(
                current_lustre_id,
                quota_id,
                self.config.unassigned_project_ids(),
            ) {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Give this user's directories on the volume their quota ID, **without**
    /// touching the quota limit.
    ///
    /// A directory that still carries an unassigned project ID (see
    /// `unassigned_project_ids`) is walked, and so is one that carries the quota
    /// ID but whose last walk was interrupted (see `assign_lustre_id`). One with
    /// some other ID, or none at all, is left alone - it was either cleared
    /// deliberately or belongs to someone else. This is what brings a straggler
    /// under a limit that is already set, where `set_user_quota` would have to
    /// rewrite that limit to do the same.
    ///
    /// Returns `false` if a walk is still running in the background when the
    /// wait for it ends.
    pub async fn assign_user_ids(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        volume_config: &UserVolumeConfig,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<bool, Error> {
        let quota_id = self.compute_user_quota_id(volume, mapping).await?;

        let mut all_set = true;

        for path_config in volume_config.path_configs() {
            let path = path_config.path(mapping.clone().into())?;
            let current_lustre_id = self.get_lustre_id_from_dir(&path, expires).await?;

            match classify_lustre_id(
                current_lustre_id,
                quota_id,
                self.config.unassigned_project_ids(),
            ) {
                // an assigned directory is walked again only if its marker
                // says the last walk did not finish - see `assign_lustre_id`
                LustreIdState::Unassigned(_) | LustreIdState::Assigned => {
                    if !self
                        .assign_lustre_id(Path::new(path_config.root()), &path, quota_id, expires)
                        .await?
                    {
                        all_set = false;
                    }
                }
                LustreIdState::Mismatch(_) => {}
            }
        }

        Ok(all_set)
    }

    pub async fn get_user_quota(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        volume_config: &UserVolumeConfig,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<Quota, Error> {
        let user = mapping.local_user();

        // Check to see if the directories have the correct Lustre ID assigned
        let quota_id = self
            .check_user_lustre_ids(mapping, volume, volume_config, expires)
            .await?;

        tracing::info!(
            "LustreEngine::get_user_quota: user={}, volume={}, quota_id={}",
            user,
            volume,
            quota_id
        );

        if quota_id == 0 {
            // return unlimited quota as the user is uninitialized
            return Ok(Quota::with_usage(
                QuotaLimit::Unlimited,
                StorageUsage::from(0),
            ));
        }

        // Get mount point (required for Lustre operations)
        let mount_point = volume_config.mount_point().ok_or_else(|| {
            Error::Misconfigured(format!(
                "Volume '{}' does not have a mount_point configured, which is required for Lustre quotas",
                volume
            ))
        })?;

        // Query the quota and return
        self.query_user_quota(quota_id, mount_point, expires).await
    }

    async fn check_project_lustre_ids(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<u64, Error> {
        let project = mapping.project().project();

        // Compute the expected quota ID for this project on this volume
        let quota_id = self.compute_project_quota_id(volume, mapping).await?;

        let mut some_unassigned = false;
        let mut some_assigned = false;

        // loop over all path configs and check lustre ids
        for path_config in volume_config.path_configs() {
            let path = path_config.project_path(mapping)?;

            // Check if the directory has the correct Lustre ID assigned
            let current_lustre_id = self.get_lustre_id_from_dir(&path, expires).await?;

            match classify_lustre_id(
                current_lustre_id,
                quota_id,
                self.config.unassigned_project_ids(),
            ) {
                LustreIdState::Assigned => some_assigned = true,
                LustreIdState::Unassigned(id) => {
                    tracing::warn!(
                        "Directory {} for project '{}' has an unassigned Lustre ID ({}) set.",
                        path.display(),
                        project,
                        id
                    );
                    some_unassigned = true;
                }
                LustreIdState::Mismatch(_) if !is_dry_run() => {
                    return Err(Error::Incompatible(format!(
                        "Directory {} for project '{}' has Lustre ID {:?}, expected {}. Please run set_project_quota first to assign the correct Lustre ID.",
                        path.display(),
                        project,
                        current_lustre_id,
                        quota_id
                    )));
                }
                LustreIdState::Mismatch(_) => {}
            }
        }

        if some_unassigned && !some_assigned {
            // return 0 as the project is effectively uninitialized
            Ok(0)
        } else {
            // At least one directory already carries the quota ID, so any quota set
            // on it is real and must be read rather than reported as unlimited -
            // otherwise a configured limit could be overwritten by a default.
            Ok(quota_id)
        }
    }

    /// Whether any of this project's directories on the volume still carries an
    /// unassigned Lustre project ID (see `unassigned_project_ids`), i.e. has never
    /// been given its own quota ID.
    pub async fn project_dirs_unassigned(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<bool, Error> {
        let quota_id = self.compute_project_quota_id(volume, mapping).await?;

        for path_config in volume_config.path_configs() {
            let path = path_config.project_path(mapping)?;
            let current_lustre_id = self.get_lustre_id_from_dir(&path, expires).await?;

            if let LustreIdState::Unassigned(_) = classify_lustre_id(
                current_lustre_id,
                quota_id,
                self.config.unassigned_project_ids(),
            ) {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Give this project's directories on the volume their quota ID, **without**
    /// touching the quota limit.
    ///
    /// A directory that still carries an unassigned project ID (see
    /// `unassigned_project_ids`) is walked, and so is one that carries the quota
    /// ID but whose last walk was interrupted (see `assign_lustre_id`). One with
    /// some other ID, or none at all, is left alone - it was either cleared
    /// deliberately or belongs to someone else. This is what brings a straggler
    /// under a limit that is already set, where `set_project_quota` would have to
    /// rewrite that limit to do the same.
    ///
    /// Returns `false` if a walk is still running in the background when the
    /// wait for it ends.
    pub async fn assign_project_ids(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<bool, Error> {
        let quota_id = self.compute_project_quota_id(volume, mapping).await?;

        let mut all_set = true;

        for path_config in volume_config.path_configs() {
            let path = path_config.project_path(mapping)?;
            let current_lustre_id = self.get_lustre_id_from_dir(&path, expires).await?;

            match classify_lustre_id(
                current_lustre_id,
                quota_id,
                self.config.unassigned_project_ids(),
            ) {
                // an assigned directory is walked again only if its marker
                // says the last walk did not finish - see `assign_lustre_id`
                LustreIdState::Unassigned(_) | LustreIdState::Assigned => {
                    if !self
                        .assign_lustre_id(Path::new(path_config.root()), &path, quota_id, expires)
                        .await?
                    {
                        all_set = false;
                    }
                }
                LustreIdState::Mismatch(_) => {}
            }
        }

        Ok(all_set)
    }

    pub async fn get_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
        expires: &bradshaw::DateTime<Utc>,
    ) -> Result<Quota, Error> {
        // Compute the quota ID for this project on this volume
        let quota_id = self
            .check_project_lustre_ids(mapping, volume, volume_config, expires)
            .await?;

        let project = mapping.project().project();

        tracing::info!(
            "LustreEngine::get_project_quota: project={}, volume={}, quota_id={}",
            project,
            volume,
            quota_id
        );

        if quota_id == 0 {
            // return unlimited quota as the project is uninitialized
            return Ok(Quota::with_usage(
                QuotaLimit::Unlimited,
                StorageUsage::from(0),
            ));
        }

        // Get mount point (required for Lustre operations)
        let mount_point = volume_config.mount_point().ok_or_else(|| {
            Error::Misconfigured(format!(
                "Volume '{}' does not have a mount_point configured, which is required for Lustre quotas",
                volume
            ))
        })?;

        // Query the quota and return
        self.query_project_quota(quota_id, mount_point, expires)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quota_id_strategy_gid() {
        let strategy = LustreIdStrategy {
            format: "{GID}".to_string(),
        };

        let result = strategy.compute_id(Some(1483800125), Some(5000));
        assert!(result.is_ok());
        #[allow(clippy::unwrap_used)]
        {
            assert_eq!(result.unwrap(), 5000);
        }
    }

    #[test]
    fn test_quota_id_strategy_uid() {
        let strategy = LustreIdStrategy {
            format: "{UID}".to_string(),
        };

        let result = strategy.compute_id(Some(1483800125), Some(5000));
        assert!(result.is_ok());
        #[allow(clippy::unwrap_used)]
        {
            assert_eq!(result.unwrap(), 1483800125);
        }
    }

    #[test]
    fn test_quota_id_strategy_uid_with_offset_and_suffix() {
        let strategy = LustreIdStrategy {
            format: "{UID-1483800000}01".to_string(),
        };

        let result = strategy.compute_id(Some(1483800125), Some(5000));
        assert!(result.is_ok());
        // 1483800125 - 1483800000 = 125, then append "01" = "12501"
        #[allow(clippy::unwrap_used)]
        {
            assert_eq!(result.unwrap(), 12501);
        }
    }

    #[test]
    fn test_quota_id_strategy_uid_different_suffix() {
        let strategy = LustreIdStrategy {
            format: "{UID-1483800000}02".to_string(),
        };

        let result = strategy.compute_id(Some(1483800125), Some(5000));
        assert!(result.is_ok());
        // 1483800125 - 1483800000 = 125, then append "02" = "12502"

        #[allow(clippy::unwrap_used)]
        {
            assert_eq!(result.unwrap(), 12502);
        }
    }

    #[test]
    fn test_quota_id_strategy_gid_with_offset() {
        let strategy = LustreIdStrategy {
            format: "{GID+1000}".to_string(),
        };

        let result = strategy.compute_id(Some(1483800125), Some(5000));
        assert!(result.is_ok());
        // 5000 + 1000 = 6000
        #[allow(clippy::unwrap_used)]
        {
            assert_eq!(result.unwrap(), 6000);
        }
    }

    #[test]
    fn test_quota_id_strategy_negative_result() {
        let strategy = LustreIdStrategy {
            format: "{UID-2000000000}".to_string(),
        };

        let result = strategy.compute_id(Some(1483800125), Some(5000));
        assert!(result.is_err());
        // Negative intermediate result will produce invalid u64
    }

    #[test]
    fn test_quota_id_strategy_invalid_offset() {
        let strategy = LustreIdStrategy {
            format: "{UID-notanumber}".to_string(),
        };

        let result = strategy.compute_id(Some(1483800125), Some(5000));
        assert!(result.is_err());
        #[allow(clippy::unwrap_used)]
        {
            assert!(result.unwrap_err().to_string().contains("Failed to parse"));
        }
    }

    #[test]
    fn test_evaluate_simple_number() {
        let strategy = LustreIdStrategy {
            format: "{5000}".to_string(),
        };

        let result = strategy.compute_id(Some(125), Some(5000));
        assert!(result.is_ok());
        #[allow(clippy::unwrap_used)]
        {
            assert_eq!(result.unwrap(), 5000);
        }
    }

    #[test]
    fn test_evaluate_addition() {
        let strategy = LustreIdStrategy {
            format: "{GID+1000}".to_string(),
        };

        let result = strategy.compute_id(Some(125), Some(5000));
        assert!(result.is_ok());
        #[allow(clippy::unwrap_used)]
        {
            assert_eq!(result.unwrap(), 6000);
        }
    }

    #[test]
    fn test_evaluate_subtraction() {
        let strategy = LustreIdStrategy {
            format: "{UID-1483800000}".to_string(),
        };

        let result = strategy.compute_id(Some(1483800125), Some(5000));
        assert!(result.is_ok());
        #[allow(clippy::unwrap_used)]
        {
            assert_eq!(result.unwrap(), 125);
        }
    }

    #[test]
    fn test_evaluate_with_suffix() {
        let strategy = LustreIdStrategy {
            format: "{UID-1483800000}01".to_string(),
        };

        let result = strategy.compute_id(Some(1483800125), Some(5000));
        assert!(result.is_ok());
        // 125 becomes "125", then "12501"
        #[allow(clippy::unwrap_used)]
        {
            assert_eq!(result.unwrap(), 12501);
        }
    }

    #[test]
    fn test_evaluate_negative_intermediate_result() {
        let strategy = LustreIdStrategy {
            format: "{UID-2000000000}99".to_string(),
        };

        let result = strategy.compute_id(Some(1483800125), Some(5000));
        // This should work - intermediate result is negative but becomes "-51619987599"
        // which is a valid string but will fail to parse as u64
        assert!(result.is_err());
    }

    #[test]
    fn test_evaluate_multiple_operations() {
        // After UID substitution: {1483800125-1483800000}
        // After evaluation: 125
        let strategy = LustreIdStrategy {
            format: "{UID-1483800000}".to_string(),
        };

        let result = strategy.compute_id(Some(1483800125), Some(5000));
        assert!(result.is_ok());
        #[allow(clippy::unwrap_used)]
        {
            assert_eq!(result.unwrap(), 125);
        }
    }

    #[test]
    fn test_unmatched_brace() {
        let strategy = LustreIdStrategy {
            format: "{UID-1000".to_string(),
        };

        let result = strategy.compute_id(Some(1483800125), Some(5000));
        assert!(result.is_err());
        #[allow(clippy::unwrap_used)]
        {
            assert!(result.unwrap_err().to_string().contains("Unmatched"));
        }
    }

    #[test]
    fn test_no_braces_direct_number() {
        let strategy = LustreIdStrategy {
            format: "12345".to_string(),
        };

        let result = strategy.compute_id(Some(1483800125), Some(5000));
        assert!(result.is_ok());
        #[allow(clippy::unwrap_used)]
        {
            assert_eq!(result.unwrap(), 12345);
        }
    }

    #[test]
    fn test_uid_required_but_not_provided() {
        let strategy = LustreIdStrategy {
            format: "{UID}".to_string(),
        };

        let result = strategy.compute_id(None, Some(5000));
        assert!(result.is_err());
        #[allow(clippy::unwrap_used)]
        {
            assert!(result.unwrap_err().to_string().contains("UID is required"));
        }
    }

    #[test]
    fn test_gid_required_but_not_provided() {
        let strategy = LustreIdStrategy {
            format: "{GID}".to_string(),
        };

        let result = strategy.compute_id(Some(125), None);
        assert!(result.is_err());
        #[allow(clippy::unwrap_used)]
        {
            assert!(result.unwrap_err().to_string().contains("GID is required"));
        }
    }

    #[test]
    fn test_deserialize_string_format() {
        // Test that we can deserialize a simple string
        let toml_str = r#"
            home = "{UID-1483800000}01"
        "#;

        let result: Result<HashMap<Volume, LustreIdStrategy>, _> = toml::from_str(toml_str);
        assert!(result.is_ok());

        #[allow(clippy::unwrap_used)]
        let strategies = result.unwrap();
        let home_strategy = strategies.get(&Volume::new("home"));
        assert!(home_strategy.is_some());

        #[allow(clippy::unwrap_used)]
        let home_strategy = home_strategy.unwrap();
        assert_eq!(home_strategy.format, "{UID-1483800000}01");
    }

    #[test]
    fn test_deserialize_map_format() {
        // Test that we can still deserialize the explicit format
        let toml_str = r#"
            home = { format = "{UID-1483800000}01" }
        "#;

        let result: Result<HashMap<Volume, LustreIdStrategy>, _> = toml::from_str(toml_str);
        assert!(result.is_ok());

        #[allow(clippy::unwrap_used)]
        let strategies = result.unwrap();
        let home_strategy = strategies.get(&Volume::new("home"));
        assert!(home_strategy.is_some());

        #[allow(clippy::unwrap_used)]
        let home_strategy = home_strategy.unwrap();
        assert_eq!(home_strategy.format, "{UID-1483800000}01");
    }

    #[test]
    fn test_deserialize_config_with_flattened_strategies() {
        // Test the full config with flattened strategies
        // Note: We deserialize without the 'type' field since LustreEngineConfig
        // is typically used inside QuotaEngineConfig which handles that field
        let toml_str = r#"
            lfs_command = "lfs"
            command_timeout_secs = 30
            home = "{UID-1483800000}01"
            scratch = "{UID-1483800000}02"
            projects = "{GID}"
        "#;

        let result: Result<LustreEngineConfig, _> = toml::from_str(toml_str);
        assert!(result.is_ok());

        #[allow(clippy::unwrap_used)]
        let config = result.unwrap();
        assert_eq!(config.lfs_command, "lfs");
        assert_eq!(config.command_timeout_secs, 30);
        assert_eq!(config.id_strategies.len(), 3);

        let home_strategy = config.id_strategies.get(&Volume::new("home"));
        assert!(home_strategy.is_some());
        #[allow(clippy::unwrap_used)]
        {
            assert_eq!(home_strategy.unwrap().format, "{UID-1483800000}01");
        }
    }

    #[test]
    fn test_classify_lustre_id() {
        let unassigned = vec![0, 1];

        assert_eq!(
            classify_lustre_id(Some(928602), 928602, &unassigned),
            LustreIdState::Assigned
        );
        assert_eq!(
            classify_lustre_id(Some(0), 928602, &unassigned),
            LustreIdState::Unassigned(0)
        );
        // inherited from a parent tagged with the catch-all project 1
        assert_eq!(
            classify_lustre_id(Some(1), 928602, &unassigned),
            LustreIdState::Unassigned(1)
        );
        assert_eq!(
            classify_lustre_id(Some(672202), 928602, &unassigned),
            LustreIdState::Mismatch(Some(672202))
        );
        // no project ID / inherit flag (e.g. explicitly cleared)
        assert_eq!(
            classify_lustre_id(None, 928602, &unassigned),
            LustreIdState::Mismatch(None)
        );
        // the expected ID wins even if it is also in the unassigned list
        assert_eq!(
            classify_lustre_id(Some(1), 1, &unassigned),
            LustreIdState::Assigned
        );
        // a site-specific list
        assert_eq!(
            classify_lustre_id(Some(1), 928602, &[0, 99]),
            LustreIdState::Mismatch(Some(1))
        );
        assert_eq!(
            classify_lustre_id(Some(99), 928602, &[0, 99]),
            LustreIdState::Unassigned(99)
        );
    }

    #[test]
    fn test_unassigned_project_ids_unset_by_default() {
        let toml_str = r#"
            lfs_command = "lfs"
            scratch = "{UID-1483800000}02"
            projects = "{GID}"
        "#;

        let result: Result<LustreEngineConfig, _> = toml::from_str(toml_str);
        assert!(result.is_ok());

        #[allow(clippy::unwrap_used)]
        let config = result.unwrap();
        // there is no default: unset means no ID counts as unassigned
        assert!(!config.has_unassigned_project_ids());
        assert!(config.unassigned_project_ids().is_empty());
        assert_eq!(config.id_strategies.len(), 2);
        assert!(!LustreEngineConfig::default().has_unassigned_project_ids());

        // so nothing is ever unassigned, not even the IDs that used to be
        assert_eq!(
            classify_lustre_id(Some(1), 928602, config.unassigned_project_ids()),
            LustreIdState::Mismatch(Some(1))
        );

        // set but empty is different from unset
        let config: LustreEngineConfig =
            toml::from_str("unassigned_project_ids = []\nprojects = \"{GID}\"").unwrap();
        assert!(config.has_unassigned_project_ids());
        assert!(config.unassigned_project_ids().is_empty());
    }

    #[test]
    fn test_unassigned_project_ids_configured() {
        let toml_str = r#"
            lfs_command = "lfs"
            unassigned_project_ids = [0, 7]
            scratch = "{UID-1483800000}02"
            projects = "{GID}"
        "#;

        let result: Result<LustreEngineConfig, _> = toml::from_str(toml_str);
        assert!(result.is_ok());

        #[allow(clippy::unwrap_used)]
        let config = result.unwrap();
        assert!(config.has_unassigned_project_ids());
        assert_eq!(config.unassigned_project_ids(), &[0, 7]);
        // the option must not be mistaken for a volume's ID strategy
        assert_eq!(config.id_strategies.len(), 2);
        assert!(config
            .get_id_strategy(&Volume::new("unassigned_project_ids"))
            .is_none());
    }

    #[test]
    fn test_unassigned_project_ids_via_quota_engine_config() {
        // as loaded from the agent's config.toml, inside the tagged engine enum
        let toml_str = r#"
            type = "lustre"
            lfs_command = "lfs"
            unassigned_project_ids = [0, 1, 2]
            scratch = "{UID-1483800000}02"
        "#;

        let result: Result<crate::quotaengine::QuotaEngineConfig, _> = toml::from_str(toml_str);
        assert!(result.is_ok(), "{:?}", result.err());

        #[allow(clippy::unwrap_used)]
        match result.unwrap() {
            crate::quotaengine::QuotaEngineConfig::Lustre(config) => {
                assert_eq!(config.unassigned_project_ids(), &[0, 1, 2]);
                assert_eq!(config.id_strategies.len(), 1);
            }
            other => panic!("expected a Lustre engine, got {:?}", other),
        }
    }

    /// A Lustre stand-in for the engine tests: `lfs` is replaced by
    /// `tests/fake_lfs.sh`, which keeps each directory's project line, and each
    /// quota ID's limit, in files under a state directory, and logs every walk
    /// and every limit it is asked for. The project directories also exist for
    /// real, so that the walk markers can be written into them.
    struct FakeLustre {
        dir: std::path::PathBuf,
        _serial: tokio::sync::OwnedMutexGuard<()>,
    }

    /// Walks share one worker, and the tests that time a walk assume theirs is
    /// the only one, so the tests that walk take turns.
    static FAKE_LUSTRE_TESTS: Lazy<Arc<Mutex<()>>> = Lazy::new(|| Arc::new(Mutex::new(())));

    const FAKE_LFS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_lfs.sh");

    impl FakeLustre {
        async fn new(name: &str) -> Self {
            let serial = FAKE_LUSTRE_TESTS.clone().lock_owned().await;
            let dir =
                std::env::temp_dir().join(format!("op-fake-lfs-{}-{}", name, std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self {
                dir,
                _serial: serial,
            }
        }

        fn root(&self, name: &str) -> String {
            format!("{}/{}", self.dir.display(), name)
        }

        /// The project directory under a root, created for real.
        fn project_dir(&self, root: &str) -> std::path::PathBuf {
            let path = std::path::PathBuf::from(format!("{}/proj", self.root(root)));
            std::fs::create_dir_all(&path).unwrap();
            path
        }

        fn seed(&self, path: &str, id: &str, flag: &str) {
            let status = std::process::Command::new("sh")
                .arg(FAKE_LFS)
                .arg(&self.dir)
                .args(["seed", path, id, flag])
                .status()
                .unwrap();
            assert!(status.success());
        }

        /// Seed the project directory under `root`, and return it.
        fn seed_project(&self, root: &str, id: &str, flag: &str) -> std::path::PathBuf {
            let path = self.project_dir(root);
            self.seed(path.to_str().unwrap(), id, flag);
            path
        }

        fn line(&self, path: &Path) -> String {
            std::fs::read_to_string(self.dir.join(path.to_str().unwrap().replace('/', "_")))
                .unwrap()
        }

        fn log(&self, name: &str) -> String {
            std::fs::read_to_string(self.dir.join(name)).unwrap_or_default()
        }

        fn touch(&self, name: &str, contents: &str) {
            use std::io::Write;
            std::fs::File::create(self.dir.join(name))
                .unwrap()
                .write_all(contents.as_bytes())
                .unwrap();
        }

        fn engine(&self, extra: &str) -> LustreEngine {
            let config: LustreEngineConfig = toml::from_str(&format!(
                "lfs_command = \"sh {} {}\"\n{}\nprojects = \"{{GID+500}}\"\n",
                FAKE_LFS,
                self.dir.display(),
                extra
            ))
            .unwrap();
            LustreEngine::new(config).unwrap()
        }

        fn volume(&self, roots: &[&str]) -> ProjectVolumeConfig {
            let roots: Vec<String> = roots
                .iter()
                .map(|r| format!("\"{}\"", self.root(r)))
                .collect();
            toml::from_str(&format!(
                "roots = [{}]\nquota_engine = \"lustre\"\nmount_point = \"{}\"\n",
                roots.join(", "),
                self.dir.display()
            ))
            .unwrap()
        }
    }

    impl Drop for FakeLustre {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn expires_in(seconds: i64) -> bradshaw::DateTime<Utc> {
        Utc::now() + bradshaw::Duration::seconds(seconds)
    }

    /// The project's group is a test-only name (see `nameservice::testing`), so
    /// these tests do not depend on the groups - or the `getent` - of the host
    /// running them. It has GID 0, so with `{GID+500}` the quota ID is 500.
    fn proj_mapping() -> ProjectMapping {
        crate::nameservice::testing::add_group("openportal-test-group", 0);
        ProjectMapping::parse("proj.portal:openportal-test-group").unwrap()
    }

    /// The marker location for a project directory directly under its root.
    fn loc(dir: &Path) -> MarkerLocation {
        MarkerLocation::new(dir.parent().unwrap(), dir).unwrap()
    }

    async fn marker(path: &Path) -> QuotaMarker {
        match crate::quotamarker::read_marker(&loc(path)).await {
            MarkerRead::Ours(marker) => marker,
            other => panic!("expected a marker in {}, got {:?}", path.display(), other),
        }
    }

    /// `assign_project_ids` must walk only the directories that still carry an
    /// unassigned ID - not one that already has the quota ID, not one that has
    /// been cleared, and not one carrying somebody else's ID - and must record
    /// each walk in the directory's marker.
    #[tokio::test]
    async fn test_assign_project_ids_only_tags_unassigned() {
        let lustre = FakeLustre::new("assign").await;

        let has_id = lustre.seed_project("has_id", "500", "P");
        let inherited = lustre.seed_project("inherited", "1", "P");
        let cleared = lustre.seed_project("cleared", "0", "-");
        let foreign = lustre.seed_project("foreign", "777", "P");

        let engine = lustre.engine("unassigned_project_ids = [1]");
        engine.initialize().await.unwrap();

        let volume = Volume::new("projects");
        let volume_config = lustre.volume(&["has_id", "inherited", "cleared", "foreign"]);
        let mapping = proj_mapping();
        let expires = expires_in(300);

        assert!(engine
            .project_dirs_unassigned(&mapping, &volume, &volume_config, &expires)
            .await
            .unwrap());

        assert!(engine
            .assign_project_ids(&mapping, &volume, &volume_config, &expires)
            .await
            .unwrap());

        // exactly one walk, for the directory with the inherited ID
        assert_eq!(
            lustre.log("assigned.log"),
            format!("500 {}\n", inherited.display())
        );

        assert!(lustre.line(&has_id).starts_with("500 P "));
        assert!(lustre.line(&inherited).starts_with("500 P "));
        assert!(lustre.line(&cleared).starts_with("0 - "));
        assert!(lustre.line(&foreign).starts_with("777 P "));

        // the walk is recorded as started and completed
        let walked = marker(&inherited).await;
        assert!(walked.is_complete_for(500));
        assert!(walked.started.is_some());
        assert!(walked.completed >= walked.started);

        // the directory that already had the ID, with no marker, was assigned
        // before markers were kept: it is taken as complete, and recorded so
        let earlier = marker(&has_id).await;
        assert!(earlier.is_complete_for(500));
        assert!(earlier.started.is_none());

        // directories that are not ours to walk get no marker
        assert_eq!(
            crate::quotamarker::read_marker(&loc(&cleared)).await,
            MarkerRead::Absent
        );
        assert_eq!(
            crate::quotamarker::read_marker(&loc(&foreign)).await,
            MarkerRead::Absent
        );

        assert!(!engine
            .project_dirs_unassigned(&mapping, &volume, &volume_config, &expires)
            .await
            .unwrap());

        // nothing left to do, so a second call walks nothing
        assert!(engine
            .assign_project_ids(&mapping, &volume, &volume_config, &expires)
            .await
            .unwrap());
        assert_eq!(lustre.log("assigned.log").lines().count(), 1);
    }

    /// A directory that carries its quota ID, but whose walk did not finish -
    /// the marker still says "started", is for another ID, or is not one the
    /// agent wrote - is walked again.
    #[tokio::test]
    async fn test_interrupted_walk_is_repeated() {
        let lustre = FakeLustre::new("interrupted").await;

        let started = lustre.seed_project("started", "500", "P");
        let other_id = lustre.seed_project("other_id", "500", "P");
        let planted = lustre.seed_project("planted", "500", "P");
        let complete = lustre.seed_project("complete", "500", "P");

        crate::quotamarker::write_marker(&loc(&started), &QuotaMarker::started(500, Utc::now()))
            .await
            .unwrap();
        crate::quotamarker::write_marker(
            &loc(&other_id),
            &QuotaMarker::complete(499, None, Utc::now()),
        )
        .await
        .unwrap();
        std::fs::create_dir(loc(&planted).path().parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("/dev/null", loc(&planted).path()).unwrap();
        crate::quotamarker::write_marker(
            &loc(&complete),
            &QuotaMarker::complete(500, None, Utc::now()),
        )
        .await
        .unwrap();

        let engine = lustre.engine("unassigned_project_ids = [1]");
        engine.initialize().await.unwrap();

        let volume = Volume::new("projects");
        let volume_config = lustre.volume(&["started", "other_id", "planted", "complete"]);
        let mapping = proj_mapping();

        // no directory is unassigned, so this is only finishing walks
        assert!(!engine
            .project_dirs_unassigned(&mapping, &volume, &volume_config, &expires_in(300))
            .await
            .unwrap());

        assert!(engine
            .assign_project_ids(&mapping, &volume, &volume_config, &expires_in(300))
            .await
            .unwrap());

        let log = lustre.log("assigned.log");
        assert_eq!(log.lines().count(), 3, "{}", log);
        assert!(log.contains(&format!("500 {}\n", started.display())));
        assert!(log.contains(&format!("500 {}\n", other_id.display())));
        assert!(log.contains(&format!("500 {}\n", planted.display())));
        assert!(!log.contains(&complete.display().to_string()));

        for path in [&started, &other_id, &planted, &complete] {
            assert!(
                marker(path).await.is_complete_for(500),
                "{}",
                path.display()
            );
        }
    }

    /// `set_project_quota` sets the limit before the walk, does not wait past
    /// the job's expiry for a long walk, and reports NotReady while it runs. A
    /// resubmission joins the running walk instead of starting another.
    #[tokio::test]
    async fn test_set_project_quota_sets_limit_first_and_is_not_ready_while_walking() {
        let lustre = FakeLustre::new("limit-first").await;
        lustre.touch("delay", "3");

        let dir = lustre.seed_project("root", "1", "P");

        let engine = lustre.engine("unassigned_project_ids = [1]");
        engine.initialize().await.unwrap();

        let volume = Volume::new("projects");
        let volume_config = lustre.volume(&["root"]);
        let mapping = proj_mapping();
        let limit = QuotaLimit::Limited(StorageSize::from_gigabytes(10.0));

        // the job expires just after the wait margin, so it can only wait ~1s
        let result = engine
            .set_project_quota(
                &mapping,
                &volume,
                &volume_config,
                &limit,
                &(Utc::now()
                    + bradshaw::Duration::from_std(WALK_WAIT_MARGIN).unwrap()
                    + bradshaw::Duration::seconds(1)),
            )
            .await;

        assert!(matches!(result, Err(Error::NotReady(_))), "{:?}", result);

        // the limit is already in place, and the walk is under way
        assert_eq!(lustre.log("setquota.log"), "500 10240M\n");
        assert_eq!(lustre.log("assigned.log"), "");
        assert_eq!(
            marker(&dir).await.state,
            crate::quotamarker::WalkState::Started
        );

        // a resubmission joins the walk, and returns the quota once it is done
        let quota = engine
            .set_project_quota(&mapping, &volume, &volume_config, &limit, &expires_in(60))
            .await
            .unwrap();

        assert_eq!(quota.limit(), &limit);
        assert_eq!(lustre.log("assigned.log").lines().count(), 1);
        assert!(marker(&dir).await.is_complete_for(500));
    }

    /// A walk that fails is reported as a failure, not as success or as still
    /// running, and leaves the marker saying it did not finish.
    #[tokio::test]
    async fn test_failed_walk_is_an_error() {
        let lustre = FakeLustre::new("failed").await;
        lustre.touch("fail", "");

        let dir = lustre.seed_project("root", "1", "P");

        let engine = lustre.engine("unassigned_project_ids = [1]");
        engine.initialize().await.unwrap();

        let result = engine
            .assign_project_ids(
                &proj_mapping(),
                &Volume::new("projects"),
                &lustre.volume(&["root"]),
                &expires_in(60),
            )
            .await;

        assert!(matches!(result, Err(Error::Failed(_))), "{:?}", result);
        assert_eq!(
            marker(&dir).await.state,
            crate::quotamarker::WalkState::Started
        );

        // once the cause is fixed, the next attempt walks again
        std::fs::remove_file(lustre.dir.join("fail")).unwrap();

        assert!(engine
            .assign_project_ids(
                &proj_mapping(),
                &Volume::new("projects"),
                &lustre.volume(&["root"]),
                &expires_in(60),
            )
            .await
            .unwrap());
        assert!(marker(&dir).await.is_complete_for(500));
    }

    /// Clearing a quota walks the tree to clear the ID, and removes the marker.
    #[tokio::test]
    async fn test_clear_project_quota_removes_marker() {
        let lustre = FakeLustre::new("clear").await;

        let dir = lustre.seed_project("root", "500", "P");
        crate::quotamarker::write_marker(&loc(&dir), &QuotaMarker::complete(500, None, Utc::now()))
            .await
            .unwrap();

        let engine = lustre.engine("unassigned_project_ids = [1]");
        engine.initialize().await.unwrap();

        engine
            .clear_project_quota(
                &proj_mapping(),
                &Volume::new("projects"),
                &lustre.volume(&["root"]),
                &expires_in(60),
            )
            .await
            .unwrap();

        assert_eq!(lustre.log("cleared.log"), format!("{}\n", dir.display()));
        assert!(lustre.line(&dir).starts_with("0 - "));
        // and the limit is reset (`setquota -p 500 -D <mount>`)
        assert_eq!(
            lustre.log("setquota.log"),
            format!("500 {}\n", lustre.dir.display())
        );
        assert_eq!(
            crate::quotamarker::read_marker(&loc(&dir)).await,
            MarkerRead::Absent
        );
    }

    /// At start-up, a volume with a default quota must have roots tagged with
    /// one of the unassigned IDs, with the inherit flag.
    #[tokio::test]
    async fn test_check_default_quota_root() {
        let lustre = FakeLustre::new("roots").await;
        let volume = Volume::new("projects");

        let tagged = lustre.root("tagged");
        let untagged = lustre.root("untagged");
        let other = lustre.root("other");
        lustre.seed(&tagged, "1", "P");
        lustre.seed(&untagged, "0", "-");
        lustre.seed(&other, "5", "P");

        let engine = lustre.engine("unassigned_project_ids = [1]");
        engine.initialize().await.unwrap();

        engine
            .check_default_quota_root(&volume, Path::new(&tagged))
            .await
            .unwrap();

        let error = engine
            .check_default_quota_root(&volume, Path::new(&untagged))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&format!("lfs project -p 1 -s {}", untagged)),
            "{}",
            error
        );

        let error = engine
            .check_default_quota_root(&volume, Path::new(&other))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("project ID 5"), "{}", error);

        // a root that cannot be read has not passed the check
        assert!(engine
            .check_default_quota_root(&volume, Path::new(&lustre.root("missing")))
            .await
            .is_err());

        // unset, nothing counts as unassigned, so no root can pass
        let engine = lustre.engine("");
        let error = engine
            .check_default_quota_root(&volume, Path::new(&tagged))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("unassigned_project_ids"), "{}", error);
    }

    /// The agent's rules for a volume's default quota, end to end through its
    /// configuration: the start-up check of the roots, then each row of the
    /// table in `set_default_project_quotas`.
    #[tokio::test]
    async fn test_default_project_quota_rules() {
        let lustre = FakeLustre::new("default-rules").await;
        let mapping = proj_mapping();
        let none = std::collections::HashSet::new();

        let config = |extra: &str| -> crate::volumeconfig::FilesystemConfig {
            toml::from_str(&format!(
                r#"
                [quota_engines.lustre]
                type = "lustre"
                lfs_command = "sh {} {}"
                {}
                projects = "{{GID+500}}"

                [project_volumes.projects]
                roots = ["{}"]
                quota_engine = "lustre"
                default_quota = "1.00 GB"
                mount_point = "{}"
                "#,
                FAKE_LFS,
                lustre.dir.display(),
                extra,
                lustre.root("projects"),
                lustre.dir.display()
            ))
            .unwrap()
        };

        // refused at start-up: unset, then an untagged root
        let error = crate::cache::set_filesystem_config(config(""))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("unassigned_project_ids"), "{}", error);

        lustre.seed(&lustre.root("projects"), "0", "-");
        let error = crate::cache::set_filesystem_config(config("unassigned_project_ids = [1]"))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("lfs project -p 1 -s"), "{}", error);

        // a tagged root passes
        lustre.seed(&lustre.root("projects"), "1", "P");
        crate::cache::set_filesystem_config(config("unassigned_project_ids = [1]"))
            .await
            .unwrap();

        // a new directory inherits the root's "1 P" (which the fake can't do)
        let dir = lustre.seed_project("projects", "1", "P");

        // unassigned, no limit: the default is set, then the ID is assigned
        crate::set_default_project_quotas(&mapping, &none, &expires_in(60))
            .await
            .unwrap();
        assert_eq!(lustre.log("setquota.log").lines().count(), 1);
        assert_eq!(lustre.log("assigned.log").lines().count(), 1);
        assert!(marker(&dir).await.is_complete_for(500));

        // repeated: nothing to do
        crate::set_default_project_quotas(&mapping, &none, &expires_in(60))
            .await
            .unwrap();
        assert_eq!(lustre.log("setquota.log").lines().count(), 1);
        assert_eq!(lustre.log("assigned.log").lines().count(), 1);

        // interrupted walk: walked again, the limit is not touched
        crate::quotamarker::write_marker(&loc(&dir), &QuotaMarker::started(500, Utc::now()))
            .await
            .unwrap();
        crate::set_default_project_quotas(&mapping, &none, &expires_in(60))
            .await
            .unwrap();
        assert_eq!(lustre.log("setquota.log").lines().count(), 1);
        assert_eq!(lustre.log("assigned.log").lines().count(), 2);
        assert!(marker(&dir).await.is_complete_for(500));

        // unassigned, but the ID already has a (raised) limit: only walked
        lustre.touch("limit_500", "5000M\n");
        lustre.seed(dir.to_str().unwrap(), "1", "P");
        crate::set_default_project_quotas(&mapping, &none, &expires_in(60))
            .await
            .unwrap();
        assert_eq!(lustre.log("setquota.log").lines().count(), 1);
        assert_eq!(lustre.log("assigned.log").lines().count(), 3);
        assert_eq!(
            std::fs::read_to_string(lustre.dir.join("limit_500")).unwrap(),
            "5000M\n"
        );

        // a walk that outlasts the job: not ready, and a resubmission joins it
        lustre.touch("delay", "3");
        lustre.seed(dir.to_str().unwrap(), "1", "P");
        let result = crate::set_default_project_quotas(
            &mapping,
            &none,
            &(Utc::now()
                + bradshaw::Duration::from_std(WALK_WAIT_MARGIN).unwrap()
                + bradshaw::Duration::seconds(1)),
        )
        .await;
        assert!(matches!(result, Err(Error::NotReady(_))), "{:?}", result);

        crate::set_default_project_quotas(&mapping, &none, &expires_in(60))
            .await
            .unwrap();
        assert_eq!(lustre.log("assigned.log").lines().count(), 4);
        assert!(marker(&dir).await.is_complete_for(500));
    }

    /// Two Lustre engines with different `lfs` commands each run their own.
    /// (The runners used to be global, and the last engine initialised won.)
    #[tokio::test]
    async fn test_engines_run_their_own_lfs_command() {
        let first = FakeLustre::new("engine-one").await;
        let one = first.seed_project("root", "111", "P");
        let first_engine = first.engine("unassigned_project_ids = [1]");

        // the second fake shares the lock, so build it by hand
        let second_dir =
            std::env::temp_dir().join(format!("op-fake-lfs-engine-two-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&second_dir);
        std::fs::create_dir_all(second_dir.join("root/proj")).unwrap();
        let two = second_dir.join("root/proj");
        let status = std::process::Command::new("sh")
            .arg(FAKE_LFS)
            .arg(&second_dir)
            .args(["seed", two.to_str().unwrap(), "222", "P"])
            .status()
            .unwrap();
        assert!(status.success());
        let second_engine = LustreEngine::new(
            toml::from_str(&format!(
                "lfs_command = \"sh {} {}\"\nmax_runners = 2\nprojects = \"{{GID}}\"\n",
                FAKE_LFS,
                second_dir.display()
            ))
            .unwrap(),
        )
        .unwrap();

        first_engine.initialize().await.unwrap();
        second_engine.initialize().await.unwrap();

        let expires = expires_in(60);

        for _ in 0..3 {
            let (a, b) = tokio::join!(
                first_engine.get_lustre_id_from_dir(&one, &expires),
                second_engine.get_lustre_id_from_dir(&two, &expires)
            );
            assert_eq!(a.unwrap(), Some(111));
            assert_eq!(b.unwrap(), Some(222));
        }

        let _ = std::fs::remove_dir_all(&second_dir);
    }

    /// A Lustre volume with a default quota is refused when the engine does not
    /// say which IDs are unassigned; without a default it is fine.
    #[test]
    fn test_default_quota_requires_unassigned_project_ids() {
        let engine: crate::quotaengine::QuotaEngineConfig =
            toml::from_str("type = \"lustre\"\nprojects = \"{GID}\"\n").unwrap();
        let volume = Volume::new("projects");
        let default = QuotaLimit::Limited(StorageSize::from_gigabytes(1.0));

        assert!(engine.verify_volume_config(&volume, None).is_ok());

        let error = engine
            .verify_volume_config(&volume, Some(&default))
            .unwrap_err()
            .to_string();
        assert!(error.contains("unassigned_project_ids"), "{}", error);

        let engine: crate::quotaengine::QuotaEngineConfig = toml::from_str(
            "type = \"lustre\"\nunassigned_project_ids = [1]\nprojects = \"{GID}\"\n",
        )
        .unwrap();
        assert!(engine.verify_volume_config(&volume, Some(&default)).is_ok());
    }

    #[test]
    fn test_validation_empty_format() {
        let toml_str = r#"
            home = ""
        "#;

        let result: Result<HashMap<Volume, LustreIdStrategy>, _> = toml::from_str(toml_str);
        assert!(result.is_err());
        #[allow(clippy::unwrap_used)]
        {
            assert!(result.unwrap_err().to_string().contains("empty"));
        }
    }

    #[test]
    fn test_validation_mismatched_braces() {
        let toml_str = r#"
            home = "{UID-1000"
        "#;

        let result: Result<HashMap<Volume, LustreIdStrategy>, _> = toml::from_str(toml_str);
        assert!(result.is_err());
        #[allow(clippy::unwrap_used)]
        {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Mismatched braces"));
        }
    }

    #[test]
    fn test_validation_dollar_prefix() {
        let toml_str = r#"
            home = "{$UID-1000}01"
        "#;

        let result: Result<HashMap<Volume, LustreIdStrategy>, _> = toml::from_str(toml_str);
        assert!(result.is_err());
        #[allow(clippy::unwrap_used)]
        {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("without the $ prefix"));
        }
    }

    #[test]
    fn test_validation_lowercase_variable() {
        let toml_str = r#"
            home = "{uid-1000}01"
        "#;

        let result: Result<HashMap<Volume, LustreIdStrategy>, _> = toml::from_str(toml_str);
        assert!(result.is_err());
        #[allow(clippy::unwrap_used)]
        {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("must be uppercase"));
        }
    }

    #[test]
    fn test_validation_spaces_in_braces() {
        let toml_str = r#"
            home = "{ UID-1000 }01"
        "#;

        let result: Result<HashMap<Volume, LustreIdStrategy>, _> = toml::from_str(toml_str);
        assert!(result.is_err());
        #[allow(clippy::unwrap_used)]
        {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("leading/trailing spaces"));
        }
    }

    #[test]
    fn test_validation_valid_formats() {
        // These should all be valid
        let valid_formats = vec![
            "{UID}",
            "{GID}",
            "{UID-1483800000}01",
            "{GID+1000}",
            "12345", // Direct number is OK
        ];

        for format_str in valid_formats {
            let strategy = LustreIdStrategy {
                format: format_str.to_string(),
            };
            assert!(
                strategy.validate().is_ok(),
                "Format '{}' should be valid",
                format_str
            );
        }
    }
}
