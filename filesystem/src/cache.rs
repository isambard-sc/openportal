// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use once_cell::sync::Lazy;
use templemeads::Error;
use tokio::sync::RwLock;

use crate::quotaengine::QuotaEngines;
use crate::volumeconfig::FilesystemConfig;

#[derive(Default)]
struct Database {
    filesystem_config: Option<FilesystemConfig>,
    user_quota_engines: Option<QuotaEngines>,
    project_quota_engines: Option<QuotaEngines>,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            .field("filesystem_config", &self.filesystem_config)
            .field("user_quota_engines", &self.user_quota_engines)
            .field("project_quota_engines", &self.project_quota_engines)
            .finish()
    }
}

impl Database {
    ///
    /// Create a new database with sensible defaults
    ///
    fn new() -> Self {
        Self {
            filesystem_config: None,
            user_quota_engines: None,
            project_quota_engines: None,
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

///
/// Get the user quota engines
///
pub async fn get_user_quota_engines() -> Result<QuotaEngines, Error> {
    let cache = CACHE.read().await;
    cache.user_quota_engines.clone().ok_or_else(|| {
        Error::Misconfigured("User quota engines have not been configured".to_owned())
    })
}

///
/// Set the user quota engines
///
pub async fn set_user_quota_engines(engines: QuotaEngines) -> Result<(), Error> {
    let mut cache = CACHE.write().await;

    tracing::info!(
        "Setting user quota engines for {} volume(s)",
        engines.len()
    );

    for volume in engines.volumes() {
        tracing::info!("  - User quotas configured for volume: {}", volume);
    }

    cache.user_quota_engines = Some(engines);
    Ok(())
}

///
/// Get the project quota engines
///
pub async fn get_project_quota_engines() -> Result<QuotaEngines, Error> {
    let cache = CACHE.read().await;
    cache.project_quota_engines.clone().ok_or_else(|| {
        Error::Misconfigured("Project quota engines have not been configured".to_owned())
    })
}

///
/// Set the project quota engines
///
pub async fn set_project_quota_engines(engines: QuotaEngines) -> Result<(), Error> {
    let mut cache = CACHE.write().await;

    tracing::info!(
        "Setting project quota engines for {} volume(s)",
        engines.len()
    );

    for volume in engines.volumes() {
        tracing::info!("  - Project quotas configured for volume: {}", volume);
    }

    cache.project_quota_engines = Some(engines);
    Ok(())
}
