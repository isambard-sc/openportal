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
                if config.get_id_strategy(volume).is_none() {
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
