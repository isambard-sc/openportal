// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Concrete implementation of the Lustre quota engine.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use templemeads::grammar::{ProjectMapping, UserMapping};
use templemeads::storage::{Quota, QuotaLimit, StorageUsage, Volume};
use templemeads::Error;

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
                Ok(LustreIdStrategy {
                    format: value.to_string(),
                })
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
                Ok(LustreIdStrategy { format })
            }
        }

        deserializer.deserialize_any(LustreIdStrategyVisitor)
    }
}

impl LustreIdStrategy {
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
    /// Example: "125-1000" evaluates to -875, "5000+1000" evaluates to 6000
    fn evaluate_arithmetic(expr: &str) -> Result<i64, Error> {
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

            Ok(result)
        } else {
            // No operator found, just parse as a number
            expr.parse::<i64>().map_err(|e| {
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
    pub lfs_command: String,

    /// Timeout in seconds for lfs commands (default: 30)
    #[serde(default = "default_command_timeout")]
    pub command_timeout_secs: u64,

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
    pub id_strategies: HashMap<Volume, LustreIdStrategy>,
}

fn default_lfs_command() -> String {
    "lfs".to_string()
}

fn default_command_timeout() -> u64 {
    30
}

impl Default for LustreEngineConfig {
    fn default() -> Self {
        Self {
            lfs_command: default_lfs_command(),
            command_timeout_secs: default_command_timeout(),
            id_strategies: HashMap::new(),
        }
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
        // Validate that the lfs command exists
        // TODO: Add validation that lfs is available
        Ok(Self { config })
    }

    fn get_id_strategy(&self, volume: &Volume) -> Result<&LustreIdStrategy, Error> {
        self.config.id_strategies.get(volume).ok_or_else(|| {
            Error::Misconfigured(format!(
                "No Lustre quota ID strategy defined for volume '{}'",
                volume
            ))
        })
    }

    /// Build the command for running lfs operations
    ///
    /// The lfs_command may contain multiple parts (e.g., "sudo lfs" or "docker exec container lfs")
    fn build_command(&self, command: &str) -> Vec<String> {
        let mut cmd = self
            .config
            .lfs_command
            .split_whitespace()
            .map(|s| s.to_string())
            .collect::<Vec<String>>();

        cmd.extend(
            command
                .split_whitespace()
                .map(|s| s.to_string())
                .collect::<Vec<String>>(),
        );

        cmd
    }

    /// Get the UID for a username by running `id -u <username>`
    async fn get_uid(&self, username: &str) -> Result<u32, Error> {
        let output = tokio::process::Command::new("id")
            .arg("-u")
            .arg(username)
            .output()
            .await
            .map_err(|e| Error::Failed(format!("Failed to execute 'id -u {}': {}", username, e)))?;

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

        Ok(uid)
    }

    /// Get the GID for a group name by running `getent group <groupname>`
    async fn get_gid(&self, groupname: &str) -> Result<u32, Error> {
        let output = tokio::process::Command::new("getent")
            .arg("group")
            .arg(groupname)
            .output()
            .await
            .map_err(|e| {
                Error::Failed(format!(
                    "Failed to execute 'getent group {}': {}",
                    groupname, e
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Failed(format!(
                "Failed to get GID for group '{}': {}",
                groupname, stderr
            )));
        }

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

        Ok(gid)
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

    pub async fn set_user_quota(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        volume_config: &UserVolumeConfig,
        limit: &QuotaLimit,
    ) -> Result<Quota, Error> {
        // TODO: Implement actual Lustre user quota setting
        // This will use: lfs setquota -u <user> -b <soft> -B <hard> <path>
        // Then immediately query to get current usage

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

        // make sure that the limit does not exceed the maximum quota for this volume
        if let Some(max_quota) = volume_config.max_quota() {
            if limit > max_quota {
                return Err(Error::Failed(format!(
                    "Requested quota limit ({}) exceeds maximum allowed quota ({}) for user {} on volume {}",
                    limit, max_quota, user, volume
                )));
            }
        }

        // Placeholder implementation
        let quota = match limit {
            QuotaLimit::Limited(size) => Quota::with_usage(
                QuotaLimit::Limited(*size),
                StorageUsage::from(0), // Would query actual usage
            ),
            QuotaLimit::Unlimited => {
                Quota::with_usage(QuotaLimit::Unlimited, StorageUsage::from(0))
            }
        };

        Ok(quota)
    }

    pub async fn set_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
        limit: &QuotaLimit,
    ) -> Result<Quota, Error> {
        // TODO: Implement actual Lustre project quota setting
        // This will use: lfs setquota -p <project_id> -b <soft> -B <hard> <path>
        // Lustre uses numeric project IDs, so we'll need to map project names to IDs
        // Then immediately query to get current usage

        let project = mapping.project().project();
        // Use the first path config's path for quota operations
        let path = volume_config.path_configs()[0].path(mapping.clone().into())?;

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

        // make sure that the limit does not exceed the maximum quota for this volume
        if let Some(max_quota) = volume_config.max_quota() {
            if limit > max_quota {
                return Err(Error::Failed(format!(
                    "Requested quota limit ({}) exceeds maximum allowed quota ({}) for project {} on volume {}",
                    limit, max_quota, project, volume
                )));
            }
        }

        // Placeholder implementation
        let quota = match limit {
            QuotaLimit::Limited(size) => Quota::with_usage(
                QuotaLimit::Limited(*size),
                StorageUsage::from(0), // Would query actual usage
            ),
            QuotaLimit::Unlimited => {
                Quota::with_usage(QuotaLimit::Unlimited, StorageUsage::from(0))
            }
        };

        Ok(quota)
    }

    pub async fn get_user_quota(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        volume_config: &UserVolumeConfig,
    ) -> Result<Quota, Error> {
        // TODO: Implement actual Lustre user quota retrieval
        // This will use: lfs quota -u <user> <path>
        // Parse the output to extract limit and usage

        let user = mapping.local_user();
        // Use the first path config's path for quota operations
        let path = volume_config.path_configs()[0].path(mapping.clone().into())?;

        // Compute the quota ID for this user on this volume
        let quota_id = self.compute_user_quota_id(volume, mapping).await?;

        tracing::info!(
            "LustreEngine::get_user_quota: user={}, volume={}, quota_id={}, path={}",
            user,
            volume,
            quota_id,
            path.display()
        );

        // Placeholder implementation
        Ok(Quota::with_usage(
            QuotaLimit::Unlimited,
            StorageUsage::from(0),
        ))
    }

    pub async fn get_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
    ) -> Result<Quota, Error> {
        // TODO: Implement actual Lustre project quota retrieval
        // This will use: lfs quota -p <project_id> <path>
        // Parse the output to extract limit and usage

        let project = mapping.project().project();
        // Use the first path config's path for quota operations
        let path = volume_config.path_configs()[0].path(mapping.clone().into())?;

        // Compute the quota ID for this project on this volume
        let quota_id = self.compute_project_quota_id(volume, mapping).await?;

        tracing::info!(
            "LustreEngine::get_project_quota: project={}, volume={}, quota_id={}, path={}",
            project,
            volume,
            quota_id,
            path.display()
        );

        // Placeholder implementation
        Ok(Quota::with_usage(
            QuotaLimit::Unlimited,
            StorageUsage::from(0),
        ))
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
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Failed to parse"));
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
        let home_strategy = strategies.get(&Volume::from("home"));
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
        let home_strategy = strategies.get(&Volume::from("home"));
        assert!(home_strategy.is_some());

        #[allow(clippy::unwrap_used)]
        let home_strategy = home_strategy.unwrap();
        assert_eq!(home_strategy.format, "{UID-1483800000}01");
    }

    #[test]
    fn test_deserialize_config_with_flattened_strategies() {
        // Test the full config with flattened strategies
        let toml_str = r#"
            type = "lustre"
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

        let home_strategy = config.id_strategies.get(&Volume::from("home"));
        assert!(home_strategy.is_some());
        #[allow(clippy::unwrap_used)]
        assert_eq!(home_strategy.unwrap().format, "{UID-1483800000}01");
    }
}
