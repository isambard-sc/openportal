// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Quota engine framework for managing filesystem quotas across different storage backends.
//!
//! This module provides an abstraction layer for setting and retrieving storage quotas
//! on different filesystem types (Lustre, Ceph, VAST, etc.). Each filesystem type
//! implements the `QuotaEngine` trait to provide backend-specific quota management.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use templemeads::grammar::{ProjectMapping, UserMapping};
use templemeads::storage::{Quota, QuotaLimit, StorageUsage};
use templemeads::Error;

use crate::volumeconfig::{ProjectVolumeConfig, UserVolumeConfig};

/// Configuration for creating quota engines.
///
/// This enum contains variants for each supported filesystem type,
/// with each variant holding the backend-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum QuotaEngineConfig {
    #[serde(rename = "lustre")]
    Lustre(LustreEngineConfig),
    // Future backends can be added here:
    // Ceph(CephEngineConfig),
    // Vast(VastEngineConfig),
}

impl QuotaEngineConfig {
    ///
    /// Set a user quota
    ///
    pub async fn set_user_quota(
        &self,
        mapping: &UserMapping,
        volume_config: &UserVolumeConfig,
        limit: &QuotaLimit,
    ) -> Result<Quota, Error> {
        match self {
            QuotaEngineConfig::Lustre(config) => {
                let engine = LustreEngine::new(config.clone())?;
                engine.set_user_quota(mapping, volume_config, limit).await
            }
        }
    }

    ///
    /// Set a project quota
    ///
    pub async fn set_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume_config: &ProjectVolumeConfig,
        limit: &QuotaLimit,
    ) -> Result<Quota, Error> {
        match self {
            QuotaEngineConfig::Lustre(config) => {
                let engine = LustreEngine::new(config.clone())?;
                Ok(engine
                    .set_project_quota(mapping, volume_config, limit)
                    .await?)
            }
        }
    }

    ///
    /// Get a user quota
    ///
    pub async fn get_user_quota(
        &self,
        mapping: &UserMapping,
        volume_config: &UserVolumeConfig,
    ) -> Result<Quota, Error> {
        match self {
            QuotaEngineConfig::Lustre(config) => {
                let engine = LustreEngine::new(config.clone())?;
                Ok(engine.get_user_quota(mapping, volume_config).await?)
            }
        }
    }

    ///
    /// Get a project quota
    ///
    pub async fn get_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume_config: &ProjectVolumeConfig,
    ) -> Result<Quota, Error> {
        match self {
            QuotaEngineConfig::Lustre(config) => {
                let engine = LustreEngine::new(config.clone())?;
                Ok(engine.get_project_quota(mapping, volume_config).await?)
            }
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

    async fn set_user_quota(
        &self,
        mapping: &UserMapping,
        volume_config: &UserVolumeConfig,
        limit: &QuotaLimit,
    ) -> Result<Quota, Error> {
        // TODO: Implement actual Lustre user quota setting
        // This will use: lfs setquota -u <user> -b <soft> -B <hard> <path>
        // Then immediately query to get current usage

        let user = mapping.local_user();
        // Use the first path config's path for quota operations
        let path = volume_config.path_configs()[0].path(mapping.clone().into())?;

        tracing::info!(
            "LustreEngine::set_user_quota: user={}, path={}, limit={}",
            user,
            path.display(),
            limit
        );

        // make sure that the limit does not exceed the maximum quota for this volume
        if let Some(max_quota) = volume_config.max_quota() {
            match limit {
                QuotaLimit::Limited(size) if size > max_quota => {
                    return Err(Error::Failed(format!(
                        "Requested quota limit ({}) exceeds maximum allowed quota ({}) for user {} on volume",
                        size, max_quota, user
                    )));
                }
                _ => {}
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

    async fn set_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume_config: &ProjectVolumeConfig,
        limit: &QuotaLimit,
    ) -> Result<Quota> {
        // TODO: Implement actual Lustre project quota setting
        // This will use: lfs setquota -p <project_id> -b <soft> -B <hard> <path>
        // Lustre uses numeric project IDs, so we'll need to map project names to IDs
        // Then immediately query to get current usage

        let project = mapping.project().project();
        // Use the first path config's path for quota operations
        let path = volume_config.path_configs()[0].path(mapping.clone().into())?;

        tracing::info!(
            "LustreEngine::set_project_quota: project={}, path={}, limit={}",
            project,
            path.display(),
            limit
        );

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

    async fn get_user_quota(
        &self,
        mapping: &UserMapping,
        volume_config: &UserVolumeConfig,
    ) -> Result<Quota> {
        // TODO: Implement actual Lustre user quota retrieval
        // This will use: lfs quota -u <user> <path>
        // Parse the output to extract limit and usage

        let user = mapping.local_user();
        // Use the first path config's path for quota operations
        let path = volume_config.path_configs()[0].path(mapping.clone().into())?;

        tracing::info!(
            "LustreEngine::get_user_quota: user={}, path={}",
            user,
            path.display()
        );

        // Placeholder implementation
        anyhow::bail!(
            "No quota found for user {} on path {}",
            user,
            path.display()
        )
    }

    async fn get_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume_config: &ProjectVolumeConfig,
    ) -> Result<Quota> {
        // TODO: Implement actual Lustre project quota retrieval
        // This will use: lfs quota -p <project_id> <path>
        // Parse the output to extract limit and usage

        let project = mapping.project().project();
        // Use the first path config's path for quota operations
        let path = volume_config.path_configs()[0].path(mapping.clone().into())?;

        tracing::info!(
            "LustreEngine::get_project_quota: project={}, path={}",
            project,
            path.display()
        );

        // Placeholder implementation
        anyhow::bail!(
            "No quota found for project {} on path {}",
            project,
            path.display()
        )
    }
}
