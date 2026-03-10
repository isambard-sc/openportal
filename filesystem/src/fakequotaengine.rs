// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Fake quota engine for local testing without real quota infrastructure.
//!
//! Quota limits are persisted as plain-text files in a local `quota_dir`
//! (host-side, written directly by this process).  Actual disk usage is
//! measured by running `du -sk` on each volume path — the `du` command is
//! configurable so it can be redirected into a Docker container just like
//! the other exec-prefix commands.
//!
//! This engine **does not enforce** quotas; it just records them and reports
//! current usage against them, which is sufficient for testing the full
//! OpenPortal quota plumbing on a Mac / Docker setup.
//!
//! # TOML configuration example
//!
//! ```toml
//! [quota_engines.fakequota]
//! type      = "fake"
//! quota_dir = "/tmp/openportal-fakequota"
//! du        = "docker exec slurmctld du"
//! ```

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use templemeads::grammar::{ProjectMapping, UserMapping};
use templemeads::job::assert_not_expired;
use templemeads::storage::{Quota, QuotaLimit, StorageSize, StorageUsage, Volume};
use templemeads::Error;
use tokio::process::Command;

use crate::volumeconfig::{ProjectVolumeConfig, UserVolumeConfig};

fn default_du_command() -> String {
    "du".to_string()
}

fn default_quota_dir() -> String {
    "/tmp/openportal-fakequota".to_string()
}

/// Configuration for the fake quota engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FakeQuotaEngineConfig {
    /// Directory on the agent host where quota limit files are stored.
    /// Defaults to `/tmp/openportal-fakequota`.
    #[serde(default = "default_quota_dir")]
    quota_dir: String,

    /// The `du` command used to measure disk usage.
    /// Can be prefixed for container execution, e.g.
    /// `"docker exec slurmctld du"`.
    #[serde(default = "default_du_command")]
    du: String,
}

/// Fake quota engine — stores limits in files, measures usage with `du`.
pub struct FakeEngine {
    config: FakeQuotaEngineConfig,
}

impl FakeEngine {
    pub fn new(config: FakeQuotaEngineConfig) -> Result<Self, Error> {
        Ok(Self { config })
    }

    /// Create the quota directory if it does not already exist.
    pub async fn initialize(&self) -> Result<(), Error> {
        tokio::fs::create_dir_all(&self.config.quota_dir)
            .await
            .map_err(|e| {
                Error::Failed(format!(
                    "FakeQuotaEngine: cannot create quota_dir '{}': {}",
                    self.config.quota_dir, e
                ))
            })
    }

    // -----------------------------------------------------------------------
    // Quota file helpers
    // -----------------------------------------------------------------------

    fn user_quota_path(&self, local_user: &str) -> PathBuf {
        Path::new(&self.config.quota_dir).join(format!("user_{}", local_user))
    }

    fn group_quota_path(&self, local_group: &str) -> PathBuf {
        Path::new(&self.config.quota_dir).join(format!("group_{}", local_group))
    }

    /// Read a quota limit from a file.  Returns `Unlimited` if the file does
    /// not exist (i.e. no quota has been set yet).
    async fn read_limit(&self, path: &Path) -> Result<QuotaLimit, Error> {
        match tokio::fs::read_to_string(path).await {
            Ok(contents) => QuotaLimit::parse(contents.trim()).map_err(|e| {
                Error::Parse(format!(
                    "Invalid quota value in '{}': {}",
                    path.display(),
                    e
                ))
            }),
            Err(_) => Ok(QuotaLimit::Unlimited),
        }
    }

    /// Write a quota limit to a file, creating the quota_dir if necessary.
    async fn write_limit(&self, path: &Path, limit: &QuotaLimit) -> Result<(), Error> {
        // Ensure the quota directory exists (in case initialize() was not called).
        tokio::fs::create_dir_all(&self.config.quota_dir)
            .await
            .map_err(|e| {
                Error::Failed(format!(
                    "FakeQuotaEngine: cannot create quota_dir '{}': {}",
                    self.config.quota_dir, e
                ))
            })?;

        tokio::fs::write(path, limit.to_string())
            .await
            .map_err(|e| {
                Error::Failed(format!(
                    "FakeQuotaEngine: cannot write '{}': {}",
                    path.display(),
                    e
                ))
            })
    }

    /// Delete a quota file (clear = back to unlimited).
    async fn delete_limit(&self, path: &Path) -> Result<(), Error> {
        if path.exists() {
            tokio::fs::remove_file(path).await.map_err(|e| {
                Error::Failed(format!(
                    "FakeQuotaEngine: cannot delete '{}': {}",
                    path.display(),
                    e
                ))
            })?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // du helper
    // -----------------------------------------------------------------------

    /// Run `du -sk <dir>` and return the result in bytes.
    /// Returns 0 if the directory does not exist or `du` fails.
    async fn du_bytes(
        &self,
        dir: &str,
        expires: &chrono::DateTime<Utc>,
    ) -> Result<u64, Error> {
        assert_not_expired(expires)?;

        let parts: Vec<&str> = self.config.du.split_whitespace().collect();
        let (prog, prefix_args) = match parts.split_first() {
            Some(pair) => pair,
            None => return Ok(0),
        };

        tracing::debug!("FakeQuotaEngine: {} -sk {}", self.config.du, dir);

        let output = Command::new(prog)
            .args(prefix_args)
            .args(["-sk", dir])
            .output()
            .await
            .map_err(|e| Error::Failed(format!("du failed on '{}': {}", dir, e)))?;

        if !output.status.success() {
            // Treat failures (e.g. directory not yet created) as zero usage.
            return Ok(0);
        }

        // `du -sk` output: "<kb>\t<path>"
        let kb: u64 = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        Ok(kb * 1024)
    }

    /// Sum `du` usage across every path in a user volume config.
    async fn user_usage(
        &self,
        mapping: &UserMapping,
        volume_config: &UserVolumeConfig,
        expires: &chrono::DateTime<Utc>,
    ) -> Result<StorageUsage, Error> {
        let mut total_bytes: u64 = 0;
        for path_config in volume_config.path_configs() {
            if let Ok(path) = path_config.path(mapping.clone().into()) {
                let bytes = self
                    .du_bytes(&path.to_string_lossy(), expires)
                    .await?;
                total_bytes = total_bytes.saturating_add(bytes);
            }
        }
        Ok(StorageUsage::new(StorageSize::from_bytes(total_bytes)))
    }

    /// Sum `du` usage across every path in a project volume config.
    async fn project_usage(
        &self,
        mapping: &ProjectMapping,
        volume_config: &ProjectVolumeConfig,
        expires: &chrono::DateTime<Utc>,
    ) -> Result<StorageUsage, Error> {
        let mut total_bytes: u64 = 0;
        for path_config in volume_config.path_configs() {
            if let Ok(path) = path_config.path(mapping.clone().into()) {
                let bytes = self
                    .du_bytes(&path.to_string_lossy(), expires)
                    .await?;
                total_bytes = total_bytes.saturating_add(bytes);
            }
        }
        Ok(StorageUsage::new(StorageSize::from_bytes(total_bytes)))
    }

    // -----------------------------------------------------------------------
    // User quota
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
        tracing::info!(
            "FakeQuotaEngine::set_user_quota: user={}, volume={}, limit={}",
            user,
            volume,
            limit
        );

        // Validate against any configured maximum.
        if let Some(max_quota) = volume_config.max_quota() {
            if limit > max_quota {
                return Err(Error::Failed(format!(
                    "Requested quota ({}) exceeds maximum ({}) for user {} on volume {}",
                    limit, max_quota, user, volume
                )));
            }
        }

        self.write_limit(&self.user_quota_path(user), limit).await?;
        self.get_user_quota(mapping, volume, volume_config, expires)
            .await
    }

    pub async fn get_user_quota(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        volume_config: &UserVolumeConfig,
        expires: &chrono::DateTime<Utc>,
    ) -> Result<Quota, Error> {
        let user = mapping.local_user();
        tracing::info!(
            "FakeQuotaEngine::get_user_quota: user={}, volume={}",
            user,
            volume
        );

        let limit = self.read_limit(&self.user_quota_path(user)).await?;
        let usage = self.user_usage(mapping, volume_config, expires).await?;
        Ok(Quota::with_usage(limit, usage))
    }

    pub async fn clear_user_quota(
        &self,
        mapping: &UserMapping,
        volume: &Volume,
        _volume_config: &UserVolumeConfig,
        _expires: &chrono::DateTime<Utc>,
    ) -> Result<(), Error> {
        let user = mapping.local_user();
        tracing::info!(
            "FakeQuotaEngine::clear_user_quota: user={}, volume={}",
            user,
            volume
        );
        self.delete_limit(&self.user_quota_path(user)).await
    }

    // -----------------------------------------------------------------------
    // Project quota
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
        tracing::info!(
            "FakeQuotaEngine::set_project_quota: group={}, volume={}, limit={}",
            group,
            volume,
            limit
        );

        // Validate against any configured maximum.
        if let Some(max_quota) = volume_config.max_quota() {
            if limit > max_quota {
                return Err(Error::Failed(format!(
                    "Requested quota ({}) exceeds maximum ({}) for project {} on volume {}",
                    limit,
                    max_quota,
                    mapping.project(),
                    volume
                )));
            }
        }

        self.write_limit(&self.group_quota_path(group), limit)
            .await?;
        self.get_project_quota(mapping, volume, volume_config, expires)
            .await
    }

    pub async fn get_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
        expires: &chrono::DateTime<Utc>,
    ) -> Result<Quota, Error> {
        let group = mapping.local_group();
        tracing::info!(
            "FakeQuotaEngine::get_project_quota: group={}, volume={}",
            group,
            volume
        );

        let limit = self.read_limit(&self.group_quota_path(group)).await?;
        let usage = self.project_usage(mapping, volume_config, expires).await?;
        Ok(Quota::with_usage(limit, usage))
    }

    pub async fn clear_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        _volume_config: &ProjectVolumeConfig,
        _expires: &chrono::DateTime<Utc>,
    ) -> Result<(), Error> {
        let group = mapping.local_group();
        tracing::info!(
            "FakeQuotaEngine::clear_project_quota: group={}, volume={}",
            group,
            volume
        );
        self.delete_limit(&self.group_quota_path(group)).await
    }
}
