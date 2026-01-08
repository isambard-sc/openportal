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
/// The strategy is specified as a format string that supports variable substitution
/// and arithmetic operations.
///
/// Supported variables:
/// - `{$UID}` - User ID
/// - `{$GID}` - Group ID
///
/// Supported operations:
/// - `{$UID-offset}` - Subtract offset from UID (e.g., `{$UID-1483800000}`)
/// - `{$GID+offset}` - Add offset to GID (e.g., `{$GID+1000}`)
///
/// The format string can include a literal suffix after the variable:
/// - `{$UID-1483800000}01` - Subtract offset and append "01"
/// - `{$GID}02` - Use GID directly and append "02"
///
/// Examples:
/// - `"{$GID}"` - Use the group ID directly for project quotas
/// - `"{$UID-1483800000}01"` - For home volume: subtract offset and append "01"
/// - `"{$UID-1483800000}02"` - For scratch volume: subtract offset and append "02"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LustreIdStrategy {
    format: String,
}

impl LustreIdStrategy {
    /// Create a new lustre quota ID strategy from a format string
    pub fn new(format: String) -> Self {
        Self { format }
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
    /// - Arithmetic operations result in negative numbers
    /// - The resulting ID cannot be parsed as a number
    pub fn compute_id(&self, uid: u32, gid: u32) -> Result<u64, Error> {
        let format = &self.format;

        // Check if format contains a variable substitution
        if !format.contains("{$") {
            return Err(Error::Misconfigured(format!(
                "Invalid quota ID strategy format '{}': must contain a variable like {{$UID}} or {{$GID}}",
                format
            )));
        }

        // Find the variable and extract the formula
        let start = format
            .find("{$")
            .ok_or_else(|| Error::Misconfigured("Missing {$ in format".to_string()))?;
        let end = format[start..]
            .find('}')
            .ok_or_else(|| Error::Misconfigured("Missing } in format".to_string()))?
            + start;

        let variable_expr = &format[start + 2..end]; // Skip the "{$" prefix
        let suffix = &format[end + 1..]; // Everything after the "}"

        // Parse the variable expression (e.g., "UID-1483800000" or "GID")
        let (var_name, operation, offset) = Self::parse_variable_expression(variable_expr)?;

        // Get the base value
        let base_value = match var_name {
            "UID" => uid as i64,
            "GID" => gid as i64,
            _ => {
                return Err(Error::Misconfigured(format!(
                    "Unknown variable '{}' in quota ID strategy. Supported: UID, GID",
                    var_name
                )))
            }
        };

        // Apply the operation
        let computed_value = match operation {
            Some('+') => base_value.checked_add(offset).ok_or_else(|| {
                Error::Failed("Arithmetic overflow in quota ID computation".to_string())
            })?,
            Some('-') => base_value.checked_sub(offset).ok_or_else(|| {
                Error::Failed("Arithmetic underflow in quota ID computation".to_string())
            })?,
            None => base_value,
            _ => {
                return Err(Error::Misconfigured(format!(
                    "Unsupported operation '{}' in quota ID strategy",
                    operation.unwrap_or(' ')
                )))
            }
        };

        // Check for negative result
        if computed_value < 0 {
            return Err(Error::Failed(format!(
                "Quota ID computation resulted in negative value: {}",
                computed_value
            )));
        }

        // Combine with suffix and parse as final ID
        let id_string = format!("{}{}", computed_value, suffix);
        id_string.parse::<u64>().map_err(|e| {
            Error::Failed(format!(
                "Failed to parse computed quota ID '{}' as number: {}",
                id_string, e
            ))
        })
    }

    /// Parse a variable expression like "UID-1483800000" or "GID" or "UID+100"
    ///
    /// Returns (variable_name, operation, offset)
    fn parse_variable_expression(expr: &str) -> Result<(&str, Option<char>, i64), Error> {
        // Check for + or - operator
        if let Some(pos) = expr.find('-') {
            let var_name = &expr[..pos];
            let offset_str = &expr[pos + 1..];
            let offset = offset_str.parse::<i64>().map_err(|e| {
                Error::Misconfigured(format!(
                    "Failed to parse offset '{}' in quota ID strategy: {}",
                    offset_str, e
                ))
            })?;
            Ok((var_name, Some('-'), offset))
        } else if let Some(pos) = expr.find('+') {
            let var_name = &expr[..pos];
            let offset_str = &expr[pos + 1..];
            let offset = offset_str.parse::<i64>().map_err(|e| {
                Error::Misconfigured(format!(
                    "Failed to parse offset '{}' in quota ID strategy: {}",
                    offset_str, e
                ))
            })?;
            Ok((var_name, Some('+'), offset))
        } else {
            // No operation, just the variable name
            Ok((expr, None, 0))
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
    /// [quota_engines.lustre_main.id_strategies]
    /// home = { format = "{$UID-1483800000}01" }
    /// scratch = { format = "{$UID-1483800000}02" }
    /// projects = { format = "{$GID}" }
    /// ```
    #[serde(default)]
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
        let strategy = self.config.id_strategies.get(volume).ok_or_else(|| {
            Error::Misconfigured(format!(
                "No quota ID strategy defined for volume '{}' in Lustre engine configuration",
                volume
            ))
        })?;

        let uid = self.get_uid(mapping.local_user()).await?;
        let gid = self.get_gid(mapping.local_group()).await?;

        strategy.compute_id(uid, gid)
    }

    /// Compute the quota ID for a project on a specific volume
    async fn compute_project_quota_id(
        &self,
        volume: &Volume,
        mapping: &ProjectMapping,
    ) -> Result<u64, Error> {
        let strategy = self.config.id_strategies.get(volume).ok_or_else(|| {
            Error::Misconfigured(format!(
                "No quota ID strategy defined for volume '{}' in Lustre engine configuration",
                volume
            ))
        })?;

        // For project quotas, we use the GID of the project's group
        // We don't have a UID in this context, so we'll use 0 as a placeholder
        let project_name = mapping.project().project();
        let gid = self.get_gid(&project_name).await?;

        strategy.compute_id(0, gid)
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
            format: "{$GID}".to_string(),
        };

        let result = strategy.compute_id(1483800125, 5000);
        assert!(result.is_ok());
        #[allow(clippy::unwrap_used)]
        {
            assert_eq!(result.unwrap(), 5000);
        }
    }

    #[test]
    fn test_quota_id_strategy_uid() {
        let strategy = LustreIdStrategy {
            format: "{$UID}".to_string(),
        };

        let result = strategy.compute_id(1483800125, 5000);
        assert!(result.is_ok());
        #[allow(clippy::unwrap_used)]
        {
            assert_eq!(result.unwrap(), 1483800125);
        }
    }

    #[test]
    fn test_quota_id_strategy_uid_with_offset_and_suffix() {
        let strategy = LustreIdStrategy {
            format: "{$UID-1483800000}01".to_string(),
        };

        let result = strategy.compute_id(1483800125, 5000);
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
            format: "{$UID-1483800000}02".to_string(),
        };

        let result = strategy.compute_id(1483800125, 5000);
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
            format: "{$GID+1000}".to_string(),
        };

        let result = strategy.compute_id(1483800125, 5000);
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
            format: "{$UID-2000000000}".to_string(),
        };

        let result = strategy.compute_id(1483800125, 5000);
        assert!(result.is_err());
        #[allow(clippy::unwrap_used)]
        {
            assert!(result.unwrap_err().to_string().contains("negative value"));
        }
    }

    #[test]
    fn test_quota_id_strategy_invalid_variable() {
        let strategy = LustreIdStrategy {
            format: "{$INVALID}".to_string(),
        };

        let result = strategy.compute_id(1483800125, 5000);
        assert!(result.is_err());
        #[allow(clippy::unwrap_used)]
        {
            assert!(result.unwrap_err().to_string().contains("Unknown variable"));
        }
    }

    #[test]
    fn test_quota_id_strategy_missing_variable() {
        let strategy = LustreIdStrategy {
            format: "12345".to_string(),
        };

        let result = strategy.compute_id(1483800125, 5000);
        assert!(result.is_err());
        #[allow(clippy::unwrap_used)]
        {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("must contain a variable"));
        }
    }

    #[test]
    fn test_quota_id_strategy_invalid_offset() {
        let strategy = LustreIdStrategy {
            format: "{$UID-notanumber}".to_string(),
        };

        let result = strategy.compute_id(1483800125, 5000);
        assert!(result.is_err());
        #[allow(clippy::unwrap_used)]
        {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Failed to parse offset"));
        }
    }
}
