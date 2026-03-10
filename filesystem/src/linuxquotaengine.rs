// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Concrete implementation of the Linux quota engine.
//!
//! Uses the standard `setquota` and `repquota` utilities to manage
//! per-user and per-group quotas on any Linux filesystem that supports
//! the quotactl interface (ext4, xfs, etc.).
//!
//! Both commands are configurable so that they can be prefixed with
//! e.g. `"docker exec slurmctld"` to operate inside a container.
//!
//! # TOML configuration example
//!
//! ```toml
//! [quota_engines.linuxquota]
//! type        = "linux"
//! filesystem  = "/dev/sda1"
//! setquota    = "docker exec slurmctld setquota"
//! repquota    = "docker exec slurmctld repquota"
//! ```

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use templemeads::grammar::{ProjectMapping, UserMapping};
use templemeads::job::assert_not_expired;
use templemeads::storage::{Quota, QuotaLimit, StorageSize, StorageUsage, Volume};
use templemeads::Error;
use tokio::process::Command;

use crate::volumeconfig::{ProjectVolumeConfig, UserVolumeConfig};

fn default_setquota_command() -> String {
    "setquota".to_string()
}

fn default_repquota_command() -> String {
    "repquota".to_string()
}

/// Configuration for the Linux quota engine.
///
/// Both `setquota` and `repquota` default to the standard system binaries
/// but can be overridden with e.g. `"docker exec slurmctld setquota"` to
/// redirect operations into a container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinuxQuotaEngineConfig {
    /// The filesystem device or mount point to manage quotas on,
    /// e.g. `/dev/sda1` or `/home`.
    filesystem: String,

    /// The `setquota` command (default: `"setquota"`).
    #[serde(default = "default_setquota_command")]
    setquota: String,

    /// The `repquota` command (default: `"repquota"`).
    #[serde(default = "default_repquota_command")]
    repquota: String,
}

/// Linux quota engine that calls `setquota` / `repquota`.
pub struct LinuxEngine {
    config: LinuxQuotaEngineConfig,
}

impl LinuxEngine {
    pub fn new(config: LinuxQuotaEngineConfig) -> Result<Self, Error> {
        if config.filesystem.trim().is_empty() {
            return Err(Error::Misconfigured(
                "LinuxQuotaEngine requires a non-empty 'filesystem' setting".to_string(),
            ));
        }
        Ok(Self { config })
    }

    /// No-op initialisation — Linux quotas require no engine-level setup.
    pub async fn initialize(&self) -> Result<(), Error> {
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Run an external command (which may include a multi-token prefix such as
    /// `"docker exec slurmctld"`) with the given extra arguments.
    ///
    /// Returns the captured stdout on success, or an [`Error`] if the process
    /// exits non-zero.
    async fn run_command(
        &self,
        program: &str,
        args: &[&str],
        expires: &chrono::DateTime<Utc>,
    ) -> Result<String, Error> {
        assert_not_expired(expires)?;

        let parts: Vec<&str> = program.split_whitespace().collect();
        let (prog, initial_args) = parts.split_first().ok_or_else(|| {
            Error::Misconfigured(format!("Linux quota command is empty: '{}'", program))
        })?;

        let cmd_str = format!("{} {}", program, args.join(" "));
        tracing::info!("LinuxQuotaEngine executing: {}", cmd_str);

        let mut cmd = Command::new(prog);
        cmd.args(initial_args);
        cmd.args(args);

        let output = cmd
            .output()
            .await
            .map_err(|e| Error::Failed(format!("Failed to spawn '{}': {}", cmd_str, e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Failed(format!(
                "Command '{}' failed (exit {:?}): {}",
                cmd_str,
                output.status.code(),
                stderr.trim()
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Convert a [`QuotaLimit`] to kilobytes for `setquota`.
    ///
    /// Returns `0` for [`QuotaLimit::Unlimited`] (which `setquota` treats
    /// as "no limit").
    fn limit_to_kb(limit: &QuotaLimit) -> u64 {
        match limit {
            QuotaLimit::Unlimited => 0,
            QuotaLimit::Limited(size) => {
                // round up to the nearest kilobyte
                let bytes = size.as_bytes();
                (bytes + 1023) / 1024
            }
        }
    }

    /// Parse `repquota` output and find the quota for `name`.
    ///
    /// `repquota` output looks like (columns vary if over soft quota):
    ///
    /// ```text
    ///                    Block limits               File limits
    /// User            used    soft    hard  grace    used  soft  hard  grace
    /// ------------------------------------------------------------------
    /// root      --       0       0       0              3     0     0
    /// alice     --   12345   20000   25000             12   100   150
    /// bob       +-   25001   20000   25000  6days        5   100   150
    /// ```
    ///
    /// Column layout (0-indexed, after splitting on whitespace):
    /// * `[0]`  name
    /// * `[1]`  flags (`--`, `+-`, `-+`, `++`)
    /// * `[2]`  block-used   (KB)
    /// * `[3]`  block-soft   (KB, 0 = unlimited)
    /// * `[4]`  block-hard   (KB, 0 = unlimited)
    /// * `[5]`  either block-grace (if flags[0] == '+') OR inode-used
    ///
    /// We only need columns 2 (usage) and 4 (hard limit).
    fn parse_quota_for_name(output: &str, name: &str) -> Result<Quota, Error> {
        for line in output.lines() {
            let tokens: Vec<&str> = line.split_whitespace().collect();

            // Need at least: name flags bused bsoft bhard
            if tokens.len() < 5 {
                continue;
            }

            if tokens[0] != name {
                continue;
            }

            // tokens[2] = block used (KB)
            let used_kb: f64 = match tokens[2].parse() {
                Ok(v) => v,
                Err(_) => continue, // malformed line; try the next
            };

            // tokens[4] = block hard limit (KB), 0 means unlimited
            let hard_kb: f64 = match tokens[4].parse() {
                Ok(v) => v,
                Err(_) => continue,
            };

            let usage = StorageUsage::new(StorageSize::from_kilobytes(used_kb));
            let limit = if hard_kb == 0.0 {
                QuotaLimit::Unlimited
            } else {
                QuotaLimit::Limited(StorageSize::from_kilobytes(hard_kb))
            };

            return Ok(Quota::with_usage(limit, usage));
        }

        // Name not found in output — no quota has been set; treat as unlimited / no usage.
        Ok(Quota::with_usage(
            QuotaLimit::Unlimited,
            StorageUsage::from(0),
        ))
    }

    // -----------------------------------------------------------------------
    // User quota methods
    // -----------------------------------------------------------------------

    pub async fn set_user_quota(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        volume_config: &UserVolumeConfig,
        limit: &QuotaLimit,
        expires: &chrono::DateTime<Utc>,
    ) -> Result<Quota, Error> {
        let user = mapping.local_user();

        // Validate against any configured maximum.
        if let Some(max_quota) = volume_config.max_quota() {
            if limit > max_quota {
                return Err(Error::Failed(format!(
                    "Requested quota limit ({}) exceeds maximum allowed quota ({}) for user {} on volume {}",
                    limit, max_quota, user, volume
                )));
            }
        }

        let kb = Self::limit_to_kb(limit);
        let inode_limit = volume_config.default_inode_limit().unwrap_or(0);
        let fs = self.config.filesystem.as_str();

        tracing::info!(
            "LinuxQuotaEngine::set_user_quota: user={}, volume={}, limit={}, kb={}, inodes={}",
            user,
            volume,
            limit,
            kb,
            inode_limit
        );

        let kb_str = kb.to_string();
        let inode_str = inode_limit.to_string();

        // setquota -u <user> <bsoft> <bhard> <isoft> <ihard> <filesystem>
        self.run_command(
            &self.config.setquota,
            &["-u", user, &kb_str, &kb_str, &inode_str, &inode_str, fs],
            expires,
        )
        .await?;

        self.get_user_quota(mapping, volume, volume_config, expires)
            .await
    }

    pub async fn get_user_quota(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        _volume_config: &UserVolumeConfig,
        expires: &chrono::DateTime<Utc>,
    ) -> Result<Quota, Error> {
        let user = mapping.local_user();
        let fs = self.config.filesystem.as_str();

        tracing::info!(
            "LinuxQuotaEngine::get_user_quota: user={}, volume={}",
            user,
            volume
        );

        let output = self
            .run_command(&self.config.repquota, &["-u", fs], expires)
            .await?;

        Self::parse_quota_for_name(&output, user)
    }

    pub async fn clear_user_quota(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        _volume_config: &UserVolumeConfig,
        expires: &chrono::DateTime<Utc>,
    ) -> Result<(), Error> {
        let user = mapping.local_user();
        let fs = self.config.filesystem.as_str();

        tracing::info!(
            "LinuxQuotaEngine::clear_user_quota: user={}, volume={}",
            user,
            volume
        );

        // setquota -u <user> 0 0 0 0 <filesystem>  (all zeros = unlimited)
        self.run_command(
            &self.config.setquota,
            &["-u", user, "0", "0", "0", "0", fs],
            expires,
        )
        .await?;

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Project (group) quota methods
    // -----------------------------------------------------------------------

    pub async fn set_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
        limit: &QuotaLimit,
        expires: &chrono::DateTime<Utc>,
    ) -> Result<Quota, Error> {
        let group = mapping.local_group();

        // Validate against any configured maximum.
        if let Some(max_quota) = volume_config.max_quota() {
            if limit > max_quota {
                return Err(Error::Failed(format!(
                    "Requested quota limit ({}) exceeds maximum allowed quota ({}) for project {} on volume {}",
                    limit, max_quota, mapping.project(), volume
                )));
            }
        }

        let kb = Self::limit_to_kb(limit);
        let inode_limit = volume_config.default_inode_limit().unwrap_or(0);
        let fs = self.config.filesystem.as_str();

        tracing::info!(
            "LinuxQuotaEngine::set_project_quota: group={}, volume={}, limit={}, kb={}, inodes={}",
            group,
            volume,
            limit,
            kb,
            inode_limit
        );

        let kb_str = kb.to_string();
        let inode_str = inode_limit.to_string();

        // setquota -g <group> <bsoft> <bhard> <isoft> <ihard> <filesystem>
        self.run_command(
            &self.config.setquota,
            &["-g", group, &kb_str, &kb_str, &inode_str, &inode_str, fs],
            expires,
        )
        .await?;

        self.get_project_quota(mapping, volume, volume_config, expires)
            .await
    }

    pub async fn get_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        _volume_config: &ProjectVolumeConfig,
        expires: &chrono::DateTime<Utc>,
    ) -> Result<Quota, Error> {
        let group = mapping.local_group();
        let fs = self.config.filesystem.as_str();

        tracing::info!(
            "LinuxQuotaEngine::get_project_quota: group={}, volume={}",
            group,
            volume
        );

        let output = self
            .run_command(&self.config.repquota, &["-g", fs], expires)
            .await?;

        Self::parse_quota_for_name(&output, group)
    }

    pub async fn clear_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        _volume_config: &ProjectVolumeConfig,
        expires: &chrono::DateTime<Utc>,
    ) -> Result<(), Error> {
        let group = mapping.local_group();
        let fs = self.config.filesystem.as_str();

        tracing::info!(
            "LinuxQuotaEngine::clear_project_quota: group={}, volume={}",
            group,
            volume
        );

        // setquota -g <group> 0 0 0 0 <filesystem>  (all zeros = unlimited)
        self.run_command(
            &self.config.setquota,
            &["-g", group, "0", "0", "0", "0", fs],
            expires,
        )
        .await?;

        Ok(())
    }
}
