// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Concrete implementation of the Lustre quota engine.

use anyhow::Result;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use templemeads::grammar::{ProjectMapping, UserMapping};
use templemeads::storage::{Quota, QuotaLimit, StorageSize, StorageUsage, Volume};
use templemeads::Error;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::volumeconfig::{ProjectVolumeConfig, UserVolumeConfig};

/// Global per-volume locks to prevent concurrent lfs commands on the same volume.
///
/// Since LustreEngine instances are created per-call, we need a global lock registry
/// to coordinate operations across multiple engine instances. This prevents race
/// conditions when setting project IDs and quotas on the same volume.
static VOLUME_LOCKS: Lazy<Mutex<HashMap<Volume, std::sync::Arc<Mutex<()>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

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
            if let Some(end) = format[start..].find('}') {
                let expr = &format[start + 1..start + end];
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
            let end = match result[start..].find('}') {
                Some(pos) => start + pos,
                None => {
                    return Err(Error::Misconfigured(format!(
                        "Unmatched opening brace in expression: {}",
                        result
                    )))
                }
            };

            // Extract the expression content (without braces)
            let expr = &result[start + 1..end];

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

        // Scan from left to right, skipping the first character (which might be a negative sign)
        for (i, ch) in expr.chars().enumerate().skip(1) {
            if ch == '+' || ch == '-' {
                operator_pos = Some(i);
                operator = ch;
                // Don't break - keep looking for the rightmost operator
            }
        }

        if let Some(pos) = operator_pos {
            // Split at the operator
            let left = expr[..pos].trim();
            let right = expr[pos + 1..].trim();

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

    /// Timeout in seconds for lfs commands (default: 30)
    #[serde(default = "default_command_timeout")]
    command_timeout_secs: u64,

    /// Timeout in seconds for recursive lfs project operations (default: 600)
    /// The `lfs project -srp` command can take a long time on large directory trees
    #[serde(default = "default_recursive_timeout")]
    recursive_timeout_secs: u64,

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

fn default_lfs_command() -> String {
    "lfs".to_string()
}

fn default_command_timeout() -> u64 {
    30
}

fn default_recursive_timeout() -> u64 {
    600
}

impl Default for LustreEngineConfig {
    fn default() -> Self {
        Self {
            lfs_command: default_lfs_command(),
            command_timeout_secs: default_command_timeout(),
            recursive_timeout_secs: default_recursive_timeout(),
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

    pub fn get_id_strategy(&self, volume: &Volume) -> Option<&LustreIdStrategy> {
        self.id_strategies.get(volume)
    }
}

/// Lustre filesystem quota engine implementation.
///
/// This engine uses the `lfs` command-line tool to manage quotas on Lustre filesystems.
/// Lustre supports both user and project quotas with separate tracking for block (storage)
/// and inode (file count) quotas.
///
/// The engine uses global per-volume locking (via VOLUME_LOCKS) to prevent race conditions
/// when multiple quota operations are happening simultaneously on the same volume.
pub struct LustreEngine {
    config: LustreEngineConfig,
}

impl LustreEngine {
    /// Create a new Lustre quota engine with the given configuration
    pub fn new(config: LustreEngineConfig) -> Result<Self> {
        // Validate that the lfs command exists
        // TODO: Add validation that lfs is available
        Ok(Self { config })
    }

    /// Get or create a lock for a specific volume
    ///
    /// This ensures that only one lfs command runs at a time on a given volume,
    /// preventing race conditions when setting project IDs and quotas.
    async fn get_volume_lock(volume: &Volume) -> std::sync::Arc<Mutex<()>> {
        let mut locks = VOLUME_LOCKS.lock().await;
        locks
            .entry(volume.clone())
            .or_insert_with(|| std::sync::Arc::new(Mutex::new(())))
            .clone()
    }

    fn get_id_strategy(&self, volume: &Volume) -> Result<&LustreIdStrategy, Error> {
        self.config.id_strategies.get(volume).ok_or_else(|| {
            Error::Misconfigured(format!(
                "No Lustre quota ID strategy defined for volume '{}'",
                volume
            ))
        })
    }

    /// Get the UID for a username by running `id -u <username>`
    ///
    /// Tries /usr/bin/id first (standard location), then falls back to PATH.
    async fn get_uid(&self, username: &str) -> Result<u32, Error> {
        // Try /usr/bin/id first (standard location)
        let result = tokio::process::Command::new("/usr/bin/id")
            .arg("-u")
            .arg(username)
            .output()
            .await;

        let output = match result {
            Ok(output) => output,
            Err(_) => {
                // Fall back to searching PATH
                tokio::process::Command::new("id")
                    .arg("-u")
                    .arg(username)
                    .output()
                    .await
                    .map_err(|e| {
                        Error::Failed(format!("Failed to execute 'id -u {}': {}", username, e))
                    })?
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Failed(format!(
                "Failed to get UID for user '{}': {}",
                username, stderr
            )));
        }

        let uid_str = String::from_utf8_lossy(&output.stdout);
        let uid = uid_str.trim().parse::<u32>().map_err(|e| {
            Error::Failed(format!(
                "Failed to parse UID '{}' for user '{}': {}",
                uid_str.trim(),
                username,
                e
            ))
        })?;

        tracing::info!("Resolved username '{}' to UID {}", username, uid);

        Ok(uid)
    }

    /// Get the GID for a group name by running `getent group <groupname>`
    ///
    /// This uses the system's getent command which queries NSS databases
    /// (including /etc/group, LDAP, IPA, etc.). Tries /usr/bin/getent first
    /// (standard Linux location), then falls back to searching PATH, and finally
    /// falls back to reading /etc/group directly (for development on macOS).
    async fn get_gid(&self, groupname: &str) -> Result<u32, Error> {
        // Try /usr/bin/getent first (standard Linux location)
        let result = tokio::process::Command::new("/usr/bin/getent")
            .arg("group")
            .arg(groupname)
            .output()
            .await;

        let output = match result {
            Ok(output) if output.status.success() => output,
            _ => {
                // Fall back to searching PATH
                let result2 = tokio::process::Command::new("getent")
                    .arg("group")
                    .arg(groupname)
                    .output()
                    .await;

                match result2 {
                    Ok(output) if output.status.success() => output,
                    _ => {
                        // Final fallback: read /etc/group directly (for macOS development)
                        return self.get_gid_from_file(groupname).await;
                    }
                }
            }
        };

        // getent group returns: groupname:x:gid:members
        let group_info = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = group_info.trim().split(':').collect();

        if parts.len() < 3 {
            return Err(Error::Failed(format!(
                "Invalid group info format for '{}': {}",
                groupname,
                group_info.trim()
            )));
        }

        let gid = parts[2].parse::<u32>().map_err(|e| {
            Error::Failed(format!(
                "Failed to parse GID '{}' for group '{}': {}",
                parts[2], groupname, e
            ))
        })?;

        tracing::info!("Resolved group name '{}' to GID {}", groupname, gid);

        Ok(gid)
    }

    /// Fallback method to get GID by reading /etc/group directly
    ///
    /// This mimics the shell getent function behavior for development on macOS.
    async fn get_gid_from_file(&self, groupname: &str) -> Result<u32, Error> {
        let contents = tokio::fs::read_to_string("/etc/group")
            .await
            .map_err(|e| Error::Failed(format!("Failed to read /etc/group: {}", e)))?;

        // Search for line matching "^groupname:"
        for line in contents.lines() {
            // Skip comments
            let line = match line.split('#').next() {
                Some(l) => l.trim(),
                None => continue,
            };

            if line.is_empty() {
                continue;
            }

            // Check if line starts with "groupname:"
            if line.starts_with(&format!("{}:", groupname)) {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 3 {
                    let gid = parts[2].parse::<u32>().map_err(|e| {
                        Error::Failed(format!(
                            "Failed to parse GID '{}' for group '{}': {}",
                            parts[2], groupname, e
                        ))
                    })?;
                    return Ok(gid);
                }
            }
        }

        Err(Error::Failed(format!(
            "Group '{}' not found in /etc/group",
            groupname
        )))
    }

    /// Compute the quota ID for a user on a specific volume
    async fn compute_user_quota_id(
        &self,
        volume: &Volume,
        mapping: &UserMapping,
    ) -> Result<u64, Error> {
        let strategy = self.get_id_strategy(volume)?;

        let uid = self.get_uid(mapping.local_user()).await?;
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

        let project_name = mapping.project().project();
        let gid = self.get_gid(&project_name).await?;

        strategy.compute_id(None, Some(gid))
    }

    /// Execute an lfs command with timeout
    ///
    /// Returns the stdout as a String if successful
    async fn run_lfs_command(
        &self,
        args: &[&str],
        timeout_duration: Duration,
    ) -> Result<String, Error> {
        let cmd_parts: Vec<&str> = self.config.lfs_command().split_whitespace().collect();
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
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(Error::Failed(format!(
                "lfs command failed: {}",
                stderr.trim()
            )));
        }

        Ok(String::from_utf8_lossy(&result.stdout).to_string())
    }

    /// Get the current Lustre ID assigned to a directory
    ///
    /// Uses `lfs project -d <directory>` to query the Lustre ID
    /// Returns None if no Lustre ID is set
    async fn get_lustre_id_from_dir(&self, directory: &Path) -> Result<Option<u64>, Error> {
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
            )
            .await?;

        // Parse output like "1234 P /path/to/directory"
        // or just "0 - /path/to/directory" if no project ID is set
        for line in output.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let id_str = parts[0];
                let flag = parts[1];

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

    /// Assign a Lustre ID to a directory recursively
    ///
    /// Uses `lfs project -srp <id> <directory>` to recursively set the Lustre ID
    /// This operation can be very slow on large directory trees
    async fn assign_lustre_id(&self, directory: &Path, lustre_id: u64) -> Result<(), Error> {
        tracing::info!(
            "Assigning Lustre ID {} to directory {} (this may take a while)",
            lustre_id,
            directory.display()
        );

        self.run_lfs_command(
            &[
                "project",
                "-srp",
                &lustre_id.to_string(),
                directory.to_str().ok_or_else(|| {
                    Error::Incompatible("Directory path contains invalid UTF-8".to_string())
                })?,
            ],
            self.config.recursive_timeout(),
        )
        .await?;

        tracing::info!(
            "Successfully assigned Lustre ID {} to directory {}",
            lustre_id,
            directory.display()
        );

        Ok(())
    }

    /// Set quota limits for a project ID
    ///
    /// Uses `lfs setquota -p <id> -B <hard_limit> -I <inode_limit> <mount>`
    async fn set_project_quota_limits(
        &self,
        project_id: u64,
        limit: &QuotaLimit,
        inode_limit: u64,
        mount_point: &str,
    ) -> Result<(), Error> {
        let block_limit = match limit {
            QuotaLimit::Unlimited => "0".to_string(), // 0 means unlimited in Lustre
            QuotaLimit::Limited(storage) => {
                // Convert to MB (Lustre uses MB units)
                let mb = storage.as_megabytes();
                format!("{}M", mb)
            }
        };

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
        )
        .await?;

        tracing::info!(
            "Set quota limits for project ID {}: {} bytes, {} inodes",
            project_id,
            block_limit,
            inode_limit
        );

        Ok(())
    }

    /// Set quota limits for a user ID
    ///
    /// Uses `lfs setquota -u <id> -B <hard_limit> -I <inode_limit> <mount>`
    async fn set_user_quota_limits(
        &self,
        user_id: u64,
        limit: &QuotaLimit,
        inode_limit: u64,
        mount_point: &str,
    ) -> Result<(), Error> {
        let block_limit = match limit {
            QuotaLimit::Unlimited => "0".to_string(),
            QuotaLimit::Limited(storage) => {
                let mb = storage.as_megabytes();
                format!("{}M", mb)
            }
        };

        self.run_lfs_command(
            &[
                "setquota",
                "-u",
                &user_id.to_string(),
                "-B",
                &block_limit,
                "-I",
                &format!("{}k", inode_limit / 1000),
                mount_point,
            ],
            self.config.command_timeout(),
        )
        .await?;

        tracing::info!(
            "Set quota limits for user ID {}: {} bytes, {} inodes",
            user_id,
            block_limit,
            inode_limit
        );

        Ok(())
    }

    /// Query quota usage and limits for a project ID
    ///
    /// Uses `lfs quota -p <id> <mount>` to query quota information
    async fn query_project_quota(
        &self,
        project_id: u64,
        mount_point: &str,
    ) -> Result<Quota, Error> {
        let output = self
            .run_lfs_command(
                &["quota", "-p", &project_id.to_string(), mount_point],
                self.config.command_timeout(),
            )
            .await?;

        self.parse_quota_output(&output)
    }

    /// Query quota usage and limits for a user ID
    ///
    /// Uses `lfs quota -u <id> <mount>` to query quota information
    async fn query_user_quota(&self, user_id: u64, mount_point: &str) -> Result<Quota, Error> {
        let output = self
            .run_lfs_command(
                &["quota", "-u", &user_id.to_string(), mount_point],
                self.config.command_timeout(),
            )
            .await?;

        self.parse_quota_output(&output)
    }

    /// Parse lfs quota command output
    ///
    /// The output format is typically:
    /// ```
    /// Disk quotas for prj 12345 (pid 12345):
    ///      Filesystem  kbytes   quota   limit   grace   files   quota   limit   grace
    ///       /scratch  1234567       0 5242880       -    5678       0 1000000       -
    /// ```
    fn parse_quota_output(&self, output: &str) -> Result<Quota, Error> {
        // Find the line with actual quota data (starts with filesystem path)
        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('/') {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 4 {
                    // parts[1] = current kbytes usage
                    // parts[3] = hard limit in kbytes (0 = unlimited)
                    let usage_kb = parts[1].parse::<u64>().map_err(|e| {
                        Error::Parse(format!("Failed to parse usage '{}': {}", parts[1], e))
                    })?;
                    let limit_kb = parts[3].parse::<u64>().map_err(|e| {
                        Error::Parse(format!("Failed to parse limit '{}': {}", parts[3], e))
                    })?;

                    let usage = StorageUsage::new(StorageSize::from_kilobytes(usage_kb as f64));
                    let limit = if limit_kb == 0 {
                        QuotaLimit::Unlimited
                    } else {
                        QuotaLimit::Limited(StorageSize::from_kilobytes(limit_kb as f64))
                    };

                    return Ok(Quota::with_usage(limit, usage));
                }
            }
        }

        // If we couldn't parse the output, return unlimited with zero usage
        tracing::warn!(
            "Could not parse quota output, returning unlimited: {}",
            output
        );
        Ok(Quota::with_usage(
            QuotaLimit::Unlimited,
            StorageUsage::from(0),
        ))
    }

    pub async fn set_user_quota(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        volume_config: &UserVolumeConfig,
        limit: &QuotaLimit,
    ) -> Result<Quota, Error> {
        let user = mapping.local_user();
        // Use the first path config's path for quota operations
        let path = volume_config.path_configs()[0].path(mapping.clone().into())?;

        // Compute the quota ID for this user on this volume
        let quota_id = self.compute_user_quota_id(volume, mapping).await?;

        tracing::info!(
            "LustreEngine::set_user_quota: user={}, volume={}, quota_id={}, path={}, limit={}",
            user,
            volume,
            quota_id,
            path.display(),
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

        // Acquire volume lock to prevent concurrent operations
        let volume_lock = Self::get_volume_lock(volume).await;
        let _lock = volume_lock.lock().await;

        // Step 1: Check if the directory has the correct Lustre ID assigned (idempotency)
        let current_lustre_id = self.get_lustre_id_from_dir(&path).await?;
        if current_lustre_id != Some(quota_id) {
            tracing::info!(
                "Directory {} has Lustre ID {:?}, need to assign {}",
                path.display(),
                current_lustre_id,
                quota_id
            );
            self.assign_lustre_id(&path, quota_id).await?;
        } else {
            tracing::debug!(
                "Directory {} already has correct Lustre ID {}",
                path.display(),
                quota_id
            );
        }

        // Set the quota limits
        self.set_user_quota_limits(quota_id, limit, inode_limit, mount_point)
            .await?;

        // Query the current usage and return the quota
        let quota = self.query_user_quota(quota_id, mount_point).await?;

        Ok(quota)
    }

    pub async fn set_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
        limit: &QuotaLimit,
    ) -> Result<Quota, Error> {
        let project = mapping.project().project();
        // Use the first path config's path for quota operations
        let path = volume_config.path_configs()[0].project_path(mapping)?;

        // Compute the quota ID for this project on this volume
        let quota_id = self.compute_project_quota_id(volume, mapping).await?;

        tracing::info!(
            "LustreEngine::set_project_quota: project={}, volume={}, quota_id={}, path={}, limit={}",
            project,
            volume,
            quota_id,
            path.display(),
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

        // Acquire volume lock to prevent concurrent operations
        let volume_lock = Self::get_volume_lock(volume).await;
        let _lock = volume_lock.lock().await;

        // Step 1: Check if the directory has the correct Lustre ID assigned (idempotency)
        let current_lustre_id = self.get_lustre_id_from_dir(&path).await?;
        if current_lustre_id != Some(quota_id) {
            tracing::info!(
                "Directory {} has Lustre ID {:?}, need to assign {}",
                path.display(),
                current_lustre_id,
                quota_id
            );
            self.assign_lustre_id(&path, quota_id).await?;
        } else {
            tracing::debug!(
                "Directory {} already has correct Lustre ID {}",
                path.display(),
                quota_id
            );
        }

        // Step 2: Set the quota limits (always do this to ensure limits are correct)
        self.set_project_quota_limits(quota_id, limit, inode_limit, mount_point)
            .await?;

        // Step 3: Query the current usage and return the quota
        let quota = self.query_project_quota(quota_id, mount_point).await?;

        Ok(quota)
    }

    pub async fn get_user_quota(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        volume_config: &UserVolumeConfig,
    ) -> Result<Quota, Error> {
        let user = mapping.local_user();

        // Compute the quota ID for this user on this volume
        let quota_id = self.compute_user_quota_id(volume, mapping).await?;

        tracing::info!(
            "LustreEngine::get_user_quota: user={}, volume={}, quota_id={}",
            user,
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

        // Query the quota and return
        self.query_user_quota(quota_id, mount_point).await
    }

    pub async fn get_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
    ) -> Result<Quota, Error> {
        let project = mapping.project().project();

        // Compute the quota ID for this project on this volume
        let quota_id = self.compute_project_quota_id(volume, mapping).await?;

        tracing::info!(
            "LustreEngine::get_project_quota: project={}, volume={}, quota_id={}",
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

        // Query the quota and return
        self.query_project_quota(quota_id, mount_point).await
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
