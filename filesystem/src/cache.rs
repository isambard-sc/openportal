// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use once_cell::sync::Lazy;
use templemeads::Error;
use tokio::sync::RwLock;

use crate::filesystem;

#[derive(Debug, Clone, Default)]
struct Database {
    home_root: String,
    home_permissions: String,

    project_roots: Vec<String>,
    project_permissions: Vec<String>,
    project_links: Vec<Option<String>>,
}

impl Database {
    ///
    /// Create a new database with sensible defaults
    ///
    fn new() -> Self {
        Self {
            home_root: "/home".to_owned(),
            home_permissions: "0755".to_owned(),
            project_roots: vec!["/project".to_owned()],
            project_permissions: vec!["2770".to_owned()],
            project_links: vec![None],
        }
    }
}

static CACHE: Lazy<RwLock<Database>> = Lazy::new(|| RwLock::new(Database::new()));

///
/// Return the root for all home directories
///
pub async fn get_home_root() -> Result<String, Error> {
    let cache = CACHE.read().await;
    Ok(cache.home_root.clone())
}

///
/// Set the root for all home directories
///
pub async fn set_home_root(root: &str) -> Result<(), Error> {
    let check_root = filesystem::clean_and_check_path(root, true).await?;

    if check_root != root {
        tracing::info!("Home {} was checked, and maps to {}", root, check_root);
    }

    let mut cache = CACHE.write().await;
    cache.home_root = root.to_owned();
    Ok(())
}

///
/// Return the root for all project directories
///
pub async fn get_project_roots() -> Result<Vec<String>, Error> {
    let cache = CACHE.read().await;
    Ok(cache.project_roots.clone())
}

///
/// Set the root for all project directories
///
pub async fn set_project_roots(roots: &Vec<String>) -> Result<(), Error> {
    let mut cache = CACHE.write().await;

    cache.project_roots.clear();

    for root in roots {
        let check_root = filesystem::clean_and_check_path(root, true).await?;

        if check_root != *root {
            tracing::info!("Project {} was checked, and maps to {}", root, check_root);
        }

        tracing::info!("Adding project root {}", root);
        cache.project_roots.push(root.clone());
    }

    Ok(())
}

///
/// Return the permissions for all home directories
///
pub async fn get_home_permissions() -> Result<String, Error> {
    let cache = CACHE.read().await;
    Ok(cache.home_permissions.clone())
}

///
/// Set the permissions for all home directories
///
pub async fn set_home_permissions(permissions: &str) -> Result<(), Error> {
    // ensure this is a valid permission string
    let _ = filesystem::clean_and_check_permissions(permissions).await?;
    tracing::info!("Setting home permissions to {}", permissions);

    let mut cache = CACHE.write().await;
    cache.home_permissions = permissions.to_owned();
    Ok(())
}

///
/// Return the permissions for all project directories
///
pub async fn get_project_permissions() -> Result<Vec<String>, Error> {
    let cache = CACHE.read().await;
    Ok(cache.project_permissions.clone())
}

///
/// Set the permissions for all project directories
///
pub async fn set_project_permissions(permissions: &Vec<String>) -> Result<(), Error> {
    let mut cache = CACHE.write().await;

    cache.project_permissions.clear();

    for permission in permissions {
        // ensure this is a valid permission string
        let _ = filesystem::clean_and_check_permissions(permission).await?;
        tracing::info!("Adding project permissions {}", permission);
        cache.project_permissions.push(permission.to_owned());
    }

    Ok(())
}

///
/// Return the links for all project directories
///
pub async fn get_project_links() -> Result<Vec<Option<String>>, Error> {
    let cache = CACHE.read().await;
    Ok(cache.project_links.clone())
}

///
/// Set the links for all project directories
///
pub async fn set_project_links(links: &Vec<String>) -> Result<(), Error> {
    let mut cache = CACHE.write().await;

    cache.project_links.clear();

    for link in links {
        let link = link.trim();

        if link.is_empty() {
            tracing::info!("No link for this project directory.");
            cache.project_links.push(None);
        } else {
            tracing::info!("Linking this project directory to {}", link);
            cache.project_links.push(Some(link.to_owned()));
        }
    }

    Ok(())
}
