// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use once_cell::sync::Lazy;
use templemeads::storage::QuotaLimit;
use templemeads::Error;
use tokio::sync::RwLock;

use crate::volumeconfig::FilesystemConfig;

use std::collections::HashSet;

#[derive(Default, Debug)]
struct Database {
    filesystem_config: Option<FilesystemConfig>,
}

impl Database {
    ///
    /// Create a new database with sensible defaults
    ///
    fn new() -> Self {
        Self {
            filesystem_config: None,
        }
    }
}

static CACHE: Lazy<RwLock<Database>> = Lazy::new(|| RwLock::new(Database::new()));

///
/// Get the filesystem configuration
///
pub async fn get_filesystem_config() -> Result<FilesystemConfig, Error> {
    let cache = CACHE.read().await;
    cache
        .filesystem_config
        .clone()
        .ok_or_else(|| Error::Misconfigured("Filesystem configuration has not been set".to_owned()))
}

///
/// Set the filesystem configuration
///
pub async fn set_filesystem_config(mut config: FilesystemConfig) -> Result<(), Error> {
    tracing::debug!("Setting filesystem configuration to {:?}", config);

    // Validate the config before storing
    config.validate()?;

    let mut cache = CACHE.write().await;

    tracing::info!(
        "Setting filesystem configuration with {} user volume(s) and {} project volume(s)",
        config.get_user_volumes().len(),
        config.get_project_volumes().len()
    );

    let mut quota_engines = HashSet::new();

    for (volume, volume_config) in config.get_user_volumes() {
        tracing::info!("  - User volume: {}", volume);
        tracing::info!("    - Paths: {:?}", volume_config.path_configs());

        if volume_config.has_quota_engine() {
            let engine_name = match volume_config.quota_engine_name() {
                Some(engine_name) => {
                    quota_engines.insert(engine_name.to_string());
                    engine_name
                }
                None => {
                    continue;
                }
            };

            tracing::info!("    - Quota engine: {}", engine_name);
            tracing::info!(
                "    - Max quota: {}",
                volume_config.max_quota().unwrap_or(&QuotaLimit::Unlimited)
            );
            tracing::info!(
                "    - Default quota: {}",
                volume_config
                    .default_quota()
                    .unwrap_or(&QuotaLimit::Unlimited)
            )
        };
    }

    for (volume, volume_config) in config.get_project_volumes() {
        tracing::info!("  - Project volume: {}", volume);
        tracing::info!("    - Paths: {:?}", volume_config.path_configs());

        if volume_config.has_quota_engine() {
            let engine_name = match volume_config.quota_engine_name() {
                Some(engine_name) => {
                    quota_engines.insert(engine_name.to_string());
                    engine_name
                }
                None => {
                    continue;
                }
            };

            tracing::info!("    - Quota engine: {}", engine_name);
            tracing::info!(
                "    - Max quota: {}",
                volume_config.max_quota().unwrap_or(&QuotaLimit::Unlimited)
            );
            tracing::info!(
                "    - Default quota: {}",
                volume_config
                    .default_quota()
                    .unwrap_or(&QuotaLimit::Unlimited)
            );
        };
    }

    for engine_name in quota_engines {
        tracing::info!("Configured quota engine: {}", engine_name);
        let engine_config = config.get_quota_engine(&engine_name)?;
        tracing::info!("  - Config: {:?}", engine_config);
        engine_config.initialize().await?;
    }

    cache.filesystem_config = Some(config);
    Ok(())
}
