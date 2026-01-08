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
use templemeads::storage::{Quota, QuotaLimit, Volume};
use templemeads::Error;

use crate::lustreengine::{LustreEngine, LustreEngineConfig};
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
        volume: &Volume,
        volume_config: &UserVolumeConfig,
        limit: &QuotaLimit,
    ) -> Result<Quota, Error> {
        match self {
            QuotaEngineConfig::Lustre(config) => {
                let engine = LustreEngine::new(config.clone())?;
                engine
                    .set_user_quota(mapping, volume, volume_config, limit)
                    .await
            }
        }
    }

    ///
    /// Set a project quota
    ///
    pub async fn set_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
        limit: &QuotaLimit,
    ) -> Result<Quota, Error> {
        match self {
            QuotaEngineConfig::Lustre(config) => {
                let engine = LustreEngine::new(config.clone())?;
                Ok(engine
                    .set_project_quota(mapping, volume, volume_config, limit)
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
        volume: &Volume,
        volume_config: &UserVolumeConfig,
    ) -> Result<Quota, Error> {
        match self {
            QuotaEngineConfig::Lustre(config) => {
                let engine = LustreEngine::new(config.clone())?;
                Ok(engine
                    .get_user_quota(mapping, volume, volume_config)
                    .await?)
            }
        }
    }

    ///
    /// Get a project quota
    ///
    pub async fn get_project_quota(
        &self,
        mapping: &ProjectMapping,
        volume: &Volume,
        volume_config: &ProjectVolumeConfig,
    ) -> Result<Quota, Error> {
        match self {
            QuotaEngineConfig::Lustre(config) => {
                let engine = LustreEngine::new(config.clone())?;
                Ok(engine
                    .get_project_quota(mapping, volume, volume_config)
                    .await?)
            }
        }
    }

    ///
    /// Verify that this engine is properly configured for the given volume.
    ///
    /// For Lustre engines, this checks that an ID strategy exists for the volume.
    ///
    pub fn verify_volume_config(&self, volume: &Volume) -> Result<(), Error> {
        match self {
            QuotaEngineConfig::Lustre(config) => {
                if !config.id_strategies.contains_key(volume) {
                    return Err(Error::Misconfigured(format!(
                        "Lustre engine is missing ID strategy for volume '{}'",
                        volume
                    )));
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lustreengine::LustreIdStrategy;
    use std::collections::HashMap;

    #[test]
    fn test_verify_volume_config_success() {
        // Create a Lustre engine with an ID strategy for "home"
        let mut id_strategies = HashMap::new();
        id_strategies.insert(
            Volume::new("home"),
            LustreIdStrategy::new("{UID-1483800000}01"),
        );

        let config = QuotaEngineConfig::Lustre(LustreEngineConfig {
            lfs_command: "lfs".to_string(),
            command_timeout_secs: 30,
            id_strategies,
        });

        let volume = Volume::new("home");
        let result = config.verify_volume_config(&volume);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_volume_config_missing_strategy() {
        // Create a Lustre engine without any ID strategies
        let config = QuotaEngineConfig::Lustre(LustreEngineConfig {
            lfs_command: "lfs".to_string(),
            command_timeout_secs: 30,
            id_strategies: HashMap::new(),
        });

        let volume = Volume::new("home");
        let result = config.verify_volume_config(&volume);
        assert!(result.is_err());

        if let Err(Error::Misconfigured(msg)) = result {
            assert!(msg.contains("missing ID strategy"));
            assert!(msg.contains("home"));
        } else {
            panic!("Expected Misconfigured error");
        }
    }

    #[test]
    fn test_verify_volume_config_wrong_volume() {
        // Create a Lustre engine with an ID strategy for "home" only
        let mut id_strategies = HashMap::new();
        id_strategies.insert(
            Volume::new("home"),
            LustreIdStrategy::new("{UID-1483800000}01"),
        );

        let config = QuotaEngineConfig::Lustre(LustreEngineConfig {
            lfs_command: "lfs".to_string(),
            command_timeout_secs: 30,
            id_strategies,
        });

        // Try to verify a different volume
        let volume = Volume::new("work");
        let result = config.verify_volume_config(&volume);
        assert!(result.is_err());

        if let Err(Error::Misconfigured(msg)) = result {
            assert!(msg.contains("missing ID strategy"));
            assert!(msg.contains("work"));
        } else {
            panic!("Expected Misconfigured error");
        }
    }
}
