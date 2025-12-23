// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Quota engine framework for managing filesystem quotas across different storage backends.
//!
//! This module provides an abstraction layer for setting and retrieving storage quotas
//! on different filesystem types (Lustre, Ceph, VAST, etc.). Each filesystem type
//! implements the `QuotaEngine` trait to provide backend-specific quota management.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use templemeads::grammar::{ProjectMapping, UserMapping};
use templemeads::storage::{Quota, QuotaLimit, StorageUsage, Volume};

use crate::volumeconfig::{ProjectVolumeConfig, UserVolumeConfig};

/// Core trait that all quota engines must implement.
///
/// This trait defines the interface for managing storage quotas on different
/// filesystem backends. Each implementation handles the specifics of interacting
/// with its particular filesystem type.
pub trait QuotaEngine: Send + Sync {
    /// Set a storage quota for a user on a specific volume.
    ///
    /// After setting the quota, this function returns the current quota and usage
    /// information, allowing callers to atomically verify that the request was
    /// properly actioned.
    ///
    /// # Arguments
    ///
    /// * `mapping` - The user mapping containing user and project information
    /// * `volume_config` - The volume configuration containing paths and settings
    /// * `limit` - The quota limit to set (can be unlimited)
    ///
    /// # Returns
    ///
    /// The current quota with usage information after the set operation
    fn set_user_quota(
        &self,
        mapping: &UserMapping,
        volume_config: &UserVolumeConfig,
        limit: &QuotaLimit,
    ) -> impl std::future::Future<Output = Result<Quota>> + Send;

    /// Set a storage quota for a project on a specific volume.
    ///
    /// After setting the quota, this function returns the current quota and usage
    /// information, allowing callers to atomically verify that the request was
    /// properly actioned.
    ///
    /// # Arguments
    ///
    /// * `mapping` - The project mapping containing project information
    /// * `volume_config` - The volume configuration containing paths and settings
    /// * `limit` - The quota limit to set (can be unlimited)
    ///
    /// # Returns
    ///
    /// The current quota with usage information after the set operation
    fn set_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume_config: &ProjectVolumeConfig,
        limit: &QuotaLimit,
    ) -> impl std::future::Future<Output = Result<Quota>> + Send;

    /// Get the current storage quota for a user on a specific volume.
    ///
    /// # Arguments
    ///
    /// * `mapping` - The user mapping containing user and project information
    /// * `volume_config` - The volume configuration containing paths and settings
    ///
    /// # Returns
    ///
    /// The current quota with usage information
    fn get_user_quota(
        &self,
        mapping: &UserMapping,
        volume_config: &UserVolumeConfig,
    ) -> impl std::future::Future<Output = Result<Quota>> + Send;

    /// Get the current storage quota for a project on a specific volume.
    ///
    /// # Arguments
    ///
    /// * `mapping` - The project mapping containing project information
    /// * `volume_config` - The volume configuration containing paths and settings
    ///
    /// # Returns
    ///
    /// The current quota with usage information
    fn get_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume_config: &ProjectVolumeConfig,
    ) -> impl std::future::Future<Output = Result<Quota>> + Send;

    /// Get all user quotas on a specific volume.
    ///
    /// This returns quota information for all users that have quotas set
    /// on the specified volume.
    ///
    /// # Arguments
    ///
    /// * `volume_config` - The volume configuration to query quotas for
    ///
    /// # Returns
    ///
    /// A vector of user quota entries
    fn get_user_quotas(
        &self,
        volume_config: &UserVolumeConfig,
    ) -> impl std::future::Future<Output = Result<Vec<UserQuotaEntry>>> + Send;

    /// Get all project quotas on a specific volume.
    ///
    /// This returns quota information for all projects that have quotas set
    /// on the specified volume.
    ///
    /// # Arguments
    ///
    /// * `volume_config` - The volume configuration to query quotas for
    ///
    /// # Returns
    ///
    /// A vector of project quota entries
    fn get_project_quotas(
        &self,
        volume_config: &ProjectVolumeConfig,
    ) -> impl std::future::Future<Output = Result<Vec<ProjectQuotaEntry>>> + Send;
}

/// Represents a user quota entry with username and quota information
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserQuotaEntry {
    pub user: String,
    pub quota: Quota,
}

impl UserQuotaEntry {
    pub fn new(user: impl Into<String>, quota: Quota) -> Self {
        Self {
            user: user.into(),
            quota,
        }
    }
}

/// Represents a project quota entry with project identifier and quota information
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectQuotaEntry {
    pub project: String,
    pub quota: Quota,
}

impl ProjectQuotaEntry {
    pub fn new(project: impl Into<String>, quota: Quota) -> Self {
        Self {
            project: project.into(),
            quota,
        }
    }
}

/// Collection of quota engines mapped by volume.
///
/// This struct manages multiple quota engines, each associated with a specific
/// storage volume. It provides methods to look up engines by volume and iterate
/// over all configured volumes.
///
/// Uses Arc internally to allow cheap cloning and sharing across async tasks.
#[derive(Default, Clone)]
pub struct QuotaEngines {
    engines: HashMap<Volume, Arc<dyn QuotaEngine>>,
}

impl QuotaEngines {
    /// Create a new empty QuotaEngines collection
    pub fn new() -> Self {
        Self {
            engines: HashMap::new(),
        }
    }

    /// Create QuotaEngines from a configuration map
    ///
    /// # Arguments
    ///
    /// * `config` - Map of volumes to their quota engine configurations
    ///
    /// # Returns
    ///
    /// A QuotaEngines instance with all configured engines initialized
    pub fn from_config(config: HashMap<Volume, QuotaEngineConfig>) -> Result<Self> {
        let mut engines = HashMap::new();

        for (volume, engine_config) in config {
            let engine = engine_config.create_engine()?;
            engines.insert(volume, Arc::from(engine));
        }

        Ok(Self { engines })
    }

    /// Get the quota engine for a specific volume
    ///
    /// # Arguments
    ///
    /// * `volume` - The volume to look up
    ///
    /// # Returns
    ///
    /// The quota engine for this volume, or None if not configured
    pub fn get(&self, volume: &Volume) -> Option<&dyn QuotaEngine> {
        self.engines.get(volume).map(|e| e.as_ref())
    }

    /// Get an iterator over all configured volumes
    pub fn volumes(&self) -> impl Iterator<Item = &Volume> {
        self.engines.keys()
    }

    /// Check if a volume has a configured quota engine
    pub fn has_volume(&self, volume: &Volume) -> bool {
        self.engines.contains_key(volume)
    }

    /// Get the number of configured quota engines
    pub fn len(&self) -> usize {
        self.engines.len()
    }

    /// Check if there are no configured quota engines
    pub fn is_empty(&self) -> bool {
        self.engines.is_empty()
    }

    /// Add a quota engine for a specific volume
    ///
    /// # Arguments
    ///
    /// * `volume` - The volume this engine manages
    /// * `engine` - The quota engine implementation
    pub fn add(&mut self, volume: Volume, engine: Arc<dyn QuotaEngine>) {
        self.engines.insert(volume, engine);
    }

    /// Add a quota engine for a specific volume from a boxed implementation
    ///
    /// # Arguments
    ///
    /// * `volume` - The volume this engine manages
    /// * `engine` - The quota engine implementation (boxed)
    pub fn add_boxed(&mut self, volume: Volume, engine: Box<dyn QuotaEngine>) {
        self.engines.insert(volume, Arc::from(engine));
    }
}

impl std::fmt::Debug for QuotaEngines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuotaEngines")
            .field("volumes", &self.engines.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Configuration for creating quota engines.
///
/// This enum contains variants for each supported filesystem type,
/// with each variant holding the backend-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuotaEngineConfig {
    /// Configuration for Lustre filesystem quota engine
    Lustre(LustreEngineConfig),
    // Future backends can be added here:
    // Ceph(CephEngineConfig),
    // Vast(VastEngineConfig),
}

impl QuotaEngineConfig {
    /// Create a quota engine instance from this configuration
    pub fn create_engine(&self) -> Result<Box<dyn QuotaEngine>> {
        match self {
            QuotaEngineConfig::Lustre(config) => Ok(Box::new(LustreEngine::new(config.clone())?)),
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
    fn build_command(&self) -> Vec<String> {
        self.config
            .lfs_command
            .split_whitespace()
            .map(|s| s.to_string())
            .collect()
    }
}

impl QuotaEngine for LustreEngine {
    async fn set_user_quota(
        &self,
        mapping: &UserMapping,
        volume_config: &UserVolumeConfig,
        limit: &QuotaLimit,
    ) -> Result<Quota> {
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

    async fn get_user_quotas(
        &self,
        volume_config: &UserVolumeConfig,
    ) -> Result<Vec<UserQuotaEntry>> {
        // TODO: Implement actual Lustre user quota listing
        // This may require iterating through users or parsing `lfs quota` output

        tracing::info!(
            "LustreEngine::get_user_quotas: volume with {} path(s)",
            volume_config.path_configs().len()
        );

        // Placeholder implementation
        Ok(Vec::new())
    }

    async fn get_project_quotas(
        &self,
        volume_config: &ProjectVolumeConfig,
    ) -> Result<Vec<ProjectQuotaEntry>> {
        // TODO: Implement actual Lustre project quota listing
        // This may require iterating through project IDs or parsing `lfs quota` output

        tracing::info!(
            "LustreEngine::get_project_quotas: volume with {} path(s)",
            volume_config.path_configs().len()
        );

        // Placeholder implementation
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use templemeads::storage::StorageSize;

    #[test]
    fn test_lustre_config_default() {
        let config = LustreEngineConfig::default();
        assert_eq!(config.lfs_command, "lfs");
        assert_eq!(config.command_timeout_secs, 30);
    }

    #[test]
    fn test_create_lustre_engine() {
        let config = LustreEngineConfig::default();
        let result = LustreEngine::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_quota_engine_config_create() {
        let config = QuotaEngineConfig::Lustre(LustreEngineConfig::default());
        let result = config.create_engine();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_user_quota_entry() {
        let entry = UserQuotaEntry::new(
            "testuser",
            Quota::limited(StorageSize::from_gigabytes(100.0)),
        );
        assert_eq!(entry.user, "testuser");
        assert!(entry.quota.limit().is_limited());
    }

    #[tokio::test]
    async fn test_project_quota_entry() {
        let entry = ProjectQuotaEntry::new("testproject", Quota::unlimited());
        assert_eq!(entry.project, "testproject");
        assert!(entry.quota.is_unlimited());
    }

    #[test]
    fn test_quota_engines_new() {
        let engines = QuotaEngines::new();
        assert!(engines.is_empty());
        assert_eq!(engines.len(), 0);
    }

    #[test]
    fn test_quota_engines_add() {
        let mut engines = QuotaEngines::new();
        let volume = Volume::new("home");
        let engine = Arc::new(LustreEngine::new(LustreEngineConfig::default()).unwrap());

        engines.add(volume.clone(), engine);

        assert!(!engines.is_empty());
        assert_eq!(engines.len(), 1);
        assert!(engines.has_volume(&volume));
        assert!(engines.get(&volume).is_some());
    }

    #[test]
    fn test_quota_engines_add_boxed() {
        let mut engines = QuotaEngines::new();
        let volume = Volume::new("home");
        let engine = Box::new(LustreEngine::new(LustreEngineConfig::default()).unwrap());

        engines.add_boxed(volume.clone(), engine);

        assert!(!engines.is_empty());
        assert_eq!(engines.len(), 1);
        assert!(engines.has_volume(&volume));
        assert!(engines.get(&volume).is_some());
    }

    #[test]
    fn test_quota_engines_from_config() {
        let mut config = HashMap::new();
        config.insert(
            Volume::new("home"),
            QuotaEngineConfig::Lustre(LustreEngineConfig::default()),
        );
        config.insert(
            Volume::new("scratch"),
            QuotaEngineConfig::Lustre(LustreEngineConfig::default()),
        );

        let result = QuotaEngines::from_config(config);
        assert!(result.is_ok());

        let engines = result.unwrap();
        assert_eq!(engines.len(), 2);
        assert!(engines.has_volume(&Volume::new("home")));
        assert!(engines.has_volume(&Volume::new("scratch")));
    }

    #[test]
    fn test_quota_engines_volumes() {
        let mut config = HashMap::new();
        config.insert(
            Volume::new("home"),
            QuotaEngineConfig::Lustre(LustreEngineConfig::default()),
        );
        config.insert(
            Volume::new("scratch"),
            QuotaEngineConfig::Lustre(LustreEngineConfig::default()),
        );

        let engines = QuotaEngines::from_config(config).unwrap();
        let volumes: Vec<_> = engines.volumes().collect();

        assert_eq!(volumes.len(), 2);
        assert!(volumes.contains(&&Volume::new("home")));
        assert!(volumes.contains(&&Volume::new("scratch")));
    }
}
