// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use once_cell::sync::Lazy;
use templemeads::Error;
use tokio::sync::RwLock;

use crate::volumeconfig::FilesystemConfig;

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
    cache.filesystem_config.clone().ok_or_else(|| {
        Error::Misconfigured("Filesystem configuration has not been set".to_owned())
    })
}

///
/// Set the filesystem configuration
///
pub async fn set_filesystem_config(mut config: FilesystemConfig) -> Result<(), Error> {
    // Validate the config before storing
    config.validate()?;

    let mut cache = CACHE.write().await;

    tracing::info!(
        "Setting filesystem configuration with {} user volume(s) and {} project volume(s)",
        config.get_user_volumes().len(),
        config.get_project_volumes().len()
    );

    for (volume, _) in config.get_user_volumes() {
        tracing::info!("  - User volume: {}", volume);
    }

    for (volume, _) in config.get_project_volumes() {
        tracing::info!("  - Project volume: {}", volume);
    }

    cache.filesystem_config = Some(config);
    Ok(())
}
