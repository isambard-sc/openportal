// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Volume configuration for filesystem agent.
//!
//! This module provides configuration structures for defining filesystem volumes,
//! including paths, permissions, and quota engine associations. Volumes can be
//! configured for user home directories or project shared directories.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use templemeads::grammar::{ProjectMapping, UserMapping, UserOrProjectMapping};
use templemeads::storage::Volume;
use templemeads::Error;

use crate::quotaengine::QuotaEngineConfig;

/// Helper function for default user subpath template
fn default_user_subpath() -> String {
    "{project}/{user}".to_string()
}

/// Helper function for default project subpath template
fn default_project_subpath() -> String {
    "{project}".to_string()
}

/// Helper function for default user permissions
fn default_user_permissions() -> StringOrVec {
    StringOrVec::Single("0755".to_string())
}

/// Helper function for default project permissions
fn default_project_permissions() -> StringOrVec {
    StringOrVec::Single("2770".to_string())
}

/// Top-level filesystem configuration.
///
/// This is the main configuration structure that gets deserialized from TOML.
/// It contains all quota engine definitions and volume configurations.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FilesystemConfig {
    /// Named quota engine configurations that can be referenced by volumes
    #[serde(default)]
    quota_engines: HashMap<String, QuotaEngineConfig>,

    /// User volume configurations (e.g., home directories)
    #[serde(default)]
    user_volumes: HashMap<Volume, UserVolumeConfig>,

    /// Project volume configurations (e.g., shared project directories)
    #[serde(default)]
    project_volumes: HashMap<Volume, ProjectVolumeConfig>,
}

impl FilesystemConfig {
    /// Create a new empty filesystem configuration
    pub fn new() -> Self {
        Self {
            quota_engines: HashMap::new(),
            user_volumes: HashMap::new(),
            project_volumes: HashMap::new(),
        }
    }

    /// Validate the configuration and apply automatic defaults
    ///
    /// This performs several checks:
    /// - Ensures at most one user volume has is_home = true
    /// - Auto-sets is_home = true if only one user volume exists
    /// - Validates that all quota_engine references exist
    /// - Validates that roots and permissions arrays have matching lengths
    pub fn validate(&mut self) -> Result<(), Error> {
        // Check at most one is_home=true across user volumes
        let home_count = self.user_volumes.values().filter(|v| v.is_home()).count();

        if home_count > 1 {
            return Err(Error::Misconfigured(
                "Multiple user volumes have is_home=true. Only one user volume can be the home directory.".to_string()
            ));
        }

        // Auto-set is_home if only one user volume
        if self.user_volumes.len() == 1 && home_count == 0 {
            if let Some(vol) = self.user_volumes.values_mut().next() {
                vol.is_home = true;
                tracing::info!(
                    "Automatically setting is_home=true for the only user volume configured"
                );
            }
        }

        // Validate quota engine references in user volumes
        for (name, vol) in &self.user_volumes {
            if let Some(engine_name) = vol.quota_engine_name() {
                if !self.quota_engines.contains_key(engine_name) {
                    return Err(Error::Misconfigured(format!(
                        "User volume '{}' references unknown quota engine: '{}'",
                        name, engine_name
                    )));
                }
            }

            // Validate array lengths match
            vol.validate()?;
        }

        // Validate quota engine references in project volumes
        for (name, vol) in &self.project_volumes {
            if let Some(engine_name) = vol.quota_engine_name() {
                if !self.quota_engines.contains_key(engine_name) {
                    return Err(Error::Misconfigured(format!(
                        "Project volume '{}' references unknown quota engine: '{}'",
                        name, engine_name
                    )));
                }
            }

            // Validate array lengths match
            vol.validate()?;
        }

        Ok(())
    }

    /// Get the home user volume configuration
    pub fn home_volume(&self) -> Result<UserVolumeConfig, Error> {
        Ok(self
            .user_volumes
            .iter()
            .find(|(_, v)| v.is_home())
            .ok_or_else(|| Error::Misconfigured("No user home volume configured".to_string()))?
            .1
            .clone())
    }

    /// Get the user volume configuration with the given name
    pub fn get_user_volume(&self, name: &Volume) -> Result<UserVolumeConfig, Error> {
        Ok(self
            .user_volumes
            .get(name)
            .ok_or_else(|| Error::NotFound(format!("User volume '{}' not found", name)))?
            .clone())
    }

    /// Get the project volume configuration with the given name
    pub fn get_project_volume(&self, name: &Volume) -> Result<ProjectVolumeConfig, Error> {
        Ok(self
            .project_volumes
            .get(name)
            .ok_or_else(|| Error::NotFound(format!("Project volume '{}' not found", name)))?
            .clone())
    }

    /// Return all of the user volumes
    pub fn get_user_volumes(&self) -> HashMap<Volume, UserVolumeConfig> {
        self.user_volumes.clone()
    }

    /// Return all of the project volumes
    pub fn get_project_volumes(&self) -> HashMap<Volume, ProjectVolumeConfig> {
        self.project_volumes.clone()
    }

    /// Return the named quota engine configurations
    pub fn get_quota_engine(&self, name: &str) -> Result<QuotaEngineConfig, Error> {
        self.quota_engines
            .get(name)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("Quota engine '{}' not found", name)))
    }
}

impl Default for FilesystemConfig {
    fn default() -> Self {
        Self::new()
    }
}

///
/// Configuration for a specific UserDirectory
///
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PathConfig {
    root: String,
    subpath: String,
    permission: String,
    link: Option<String>,
}

impl PathConfig {
    pub fn new(root: String, subpath: String, permission: String, link: Option<String>) -> Self {
        Self {
            root,
            subpath,
            permission,
            link,
        }
    }

    pub fn permission(&self) -> &str {
        &self.permission
    }

    pub fn project_path(&self, mapping: &ProjectMapping) -> Result<PathBuf, Error> {
        let project_name = mapping.project().project();

        if project_name.is_empty() {
            return Err(Error::MissingProject(
                "Project name is empty in project mapping".to_string(),
            ));
        }

        // we remove the {user} placeholder for project paths
        let expanded_subpath = self
            .subpath
            .replace("{project}", &project_name)
            .replace("{user}", "")
            .replace("//", "/");

        Ok(Path::new(&format!("{}/{}", self.root, expanded_subpath)).to_path_buf())
    }

    pub fn path(&self, mapping: UserOrProjectMapping) -> Result<PathBuf, Error> {
        match mapping {
            UserOrProjectMapping::User(user_mapping) => {
                let project_name = user_mapping.local_group();
                let user_name = user_mapping.local_user();

                if project_name.is_empty() {
                    return Err(Error::MissingProject(
                        "Project name is empty in user mapping".to_string(),
                    ));
                }

                if user_name.is_empty() {
                    return Err(Error::MissingUser(
                        "User name is empty in user mapping".to_string(),
                    ));
                }

                let expanded_subpath = self
                    .subpath
                    .replace("{project}", project_name)
                    .replace("{user}", user_name);

                Ok(Path::new(&format!("{}/{}", self.root, expanded_subpath)).to_path_buf())
            }
            UserOrProjectMapping::Project(project_mapping) => {
                let project_name = project_mapping.project().project();

                if project_name.is_empty() {
                    return Err(Error::MissingProject(
                        "Project name is empty in project mapping".to_string(),
                    ));
                }

                let expanded_subpath = self.subpath.replace("{project}", &project_name);

                Ok(Path::new(&format!("{}/{}", self.root, expanded_subpath)).to_path_buf())
            }
        }
    }

    pub fn link_path(&self, mapping: UserOrProjectMapping) -> Result<Option<PathBuf>, Error> {
        if let Some(link_template) = &self.link {
            match mapping {
                UserOrProjectMapping::User(user_mapping) => {
                    let project_name = user_mapping.local_group();
                    let user_name = user_mapping.local_user();

                    if project_name.is_empty() {
                        return Err(Error::MissingProject(
                            "Project name is empty in user mapping".to_string(),
                        ));
                    }

                    if user_name.is_empty() {
                        return Err(Error::MissingUser(
                            "User name is empty in user mapping".to_string(),
                        ));
                    }

                    let expanded_link = link_template
                        .replace("{project}", project_name)
                        .replace("{user}", user_name);

                    Ok(Some(Path::new(&expanded_link).to_path_buf()))
                }
                UserOrProjectMapping::Project(project_mapping) => {
                    let project_name = project_mapping.project().project();

                    if project_name.is_empty() {
                        return Err(Error::MissingProject(
                            "Project name is empty in project mapping".to_string(),
                        ));
                    }

                    let expanded_link = link_template.replace("{project}", &project_name);

                    Ok(Some(Path::new(&expanded_link).to_path_buf()))
                }
            }
        } else {
            Ok(None)
        }
    }
}

/// Configuration for a user volume (e.g., home directories).
///
/// User volumes contain user-specific directories, typically with a structure
/// like /home/{project}/{user} or /scratch/{project}/{user}.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserVolumeConfig {
    /// Root directories for this volume (can specify multiple for shared quotas)
    /// Example: ["/home", "/scratch"]
    roots: Vec<String>,

    /// Subpath template for directory structure within each root
    /// Placeholders: {project}, {user}
    /// Default: "{project}/{user}"
    #[serde(default = "default_user_subpath")]
    subpath: String,

    /// Permissions for directories (single value or one per root)
    /// Default: "0755"
    #[serde(default = "default_user_permissions")]
    permissions: StringOrVec,

    /// Whether this is the primary home directory
    /// Only one user volume can have is_home=true
    /// Auto-set to true if only one user volume exists
    /// Default: false
    #[serde(default)]
    is_home: bool,

    /// Optional name of quota engine to use (references quota_engines map)
    quota_engine: Option<String>,

    /// Optional symlinks to create (empty string = no link, one per root)
    /// Example: ["", "/fastwork/{project}/{user}"] for two roots
    #[serde(default)]
    links: Vec<String>,
}

impl UserVolumeConfig {
    pub fn new(
        roots: Vec<String>,
        subpath: String,
        permissions: StringOrVec,
        is_home: bool,
        quota_engine: Option<String>,
        links: Vec<String>,
    ) -> Result<Self, Error> {
        let instance = Self {
            roots,
            subpath,
            permissions,
            is_home,
            quota_engine,
            links,
        };

        instance.validate()?;
        Ok(instance)
    }

    pub fn validate(&self) -> Result<(), Error> {
        let num_roots = self.roots.len();

        if num_roots == 0 {
            return Err(Error::Misconfigured(
                "User volume must have at least one root directory".to_string(),
            ));
        }

        // Check permissions matches roots
        match &self.permissions {
            StringOrVec::Single(_) => {} // Single value is valid for any number of roots
            StringOrVec::Vec(perms) => {
                if perms.len() != num_roots {
                    return Err(Error::Misconfigured(format!(
                        "User volume has {} roots but {} permissions values",
                        num_roots,
                        perms.len()
                    )));
                }
            }
        }

        // Check links matches roots (if provided)
        if !self.links.is_empty() && self.links.len() != num_roots {
            return Err(Error::Misconfigured(format!(
                "User volume has {} roots but {} link values",
                num_roots,
                self.links.len()
            )));
        }

        // make sure that if this is the home volume, then there
        // is only a single path
        if self.is_home && num_roots != 1 {
            return Err(Error::Misconfigured(
                "User home volume must have exactly one root directory".to_string(),
            ));
        }

        Ok(())
    }

    /// Get whether this is the home volume
    pub fn is_home(&self) -> bool {
        self.is_home && self.roots.len() == 1
    }

    /// Return the home path for the passed user
    pub fn home_path(&self, mapping: &UserMapping) -> Result<PathBuf, Error> {
        if !self.is_home {
            return Err(Error::Misconfigured(
                "Attempted to get home path from non-home user volume".to_string(),
            ));
        }

        Ok(self.path_configs()[0]
            .path(mapping.clone().into())?
            .to_path_buf())
    }

    /// Get the quota engine name
    pub fn quota_engine_name(&self) -> Option<&str> {
        self.quota_engine.as_deref()
    }

    /// Return all of the paths for this volume
    pub fn path_configs(&self) -> Vec<PathConfig> {
        let num_roots = self.roots.len();
        let mut paths = Vec::with_capacity(num_roots);

        for i in 0..num_roots {
            let permission = match &self.permissions {
                StringOrVec::Single(s) => s.clone(),
                StringOrVec::Vec(v) => v[i].clone(),
            };

            let link = if !self.links.is_empty() {
                let link_str = self.links[i].trim();
                if link_str.is_empty() {
                    None
                } else {
                    Some(link_str.to_string())
                }
            } else {
                None
            };

            paths.push(PathConfig::new(
                self.roots[i].clone(),
                self.subpath.clone(),
                permission,
                link,
            ));
        }

        paths
    }
}

/// Configuration for a project volume (e.g., shared project directories).
///
/// Project volumes contain project-level shared directories, typically with
/// a structure like /projects/{project} or /work/{project}.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProjectVolumeConfig {
    /// Root directories for this volume (can specify multiple for shared quotas)
    /// Example: ["/projects", "/work"]
    roots: Vec<String>,

    /// Subpath template for directory structure within each root
    /// Placeholders: {project}
    /// Default: "{project}"
    #[serde(default = "default_project_subpath")]
    subpath: String,

    /// Permissions for directories (single value or one per root)
    /// Default: "2770"
    #[serde(default = "default_project_permissions")]
    permissions: StringOrVec,

    /// Optional name of quota engine to use (references quota_engines map)
    quota_engine: Option<String>,

    /// Optional symlinks to create (empty string = no link, one per root)
    /// Example: ["", "/fastwork/{project}"] for two roots
    #[serde(default)]
    links: Vec<String>,
}

impl ProjectVolumeConfig {
    pub fn new(
        roots: Vec<String>,
        subpath: String,
        permissions: StringOrVec,
        quota_engine: Option<String>,
        links: Vec<String>,
    ) -> Result<Self, Error> {
        let instance = Self {
            roots,
            subpath,
            permissions,
            quota_engine,
            links,
        };

        instance.validate()?;
        Ok(instance)
    }

    pub fn validate(&self) -> Result<(), Error> {
        let num_roots = self.roots.len();

        if num_roots == 0 {
            return Err(Error::Misconfigured(
                "Project volume must have at least one root directory".to_string(),
            ));
        }

        // Check permissions matches roots
        match &self.permissions {
            StringOrVec::Single(_) => {} // Single value is valid for any number of roots
            StringOrVec::Vec(perms) => {
                if perms.len() != num_roots {
                    return Err(Error::Misconfigured(format!(
                        "Project volume has {} roots but {} permissions values",
                        num_roots,
                        perms.len()
                    )));
                }
            }
        }

        // Check links matches roots (if provided)
        if !self.links.is_empty() && self.links.len() != num_roots {
            return Err(Error::Misconfigured(format!(
                "Project volume has {} roots but {} link values",
                num_roots,
                self.links.len()
            )));
        }

        Ok(())
    }

    /// Get the quota engine name
    pub fn quota_engine_name(&self) -> Option<&str> {
        self.quota_engine.as_deref()
    }

    /// Return all of the paths for this volume
    pub fn path_configs(&self) -> Vec<PathConfig> {
        let num_roots = self.roots.len();
        let mut paths = Vec::with_capacity(num_roots);

        for i in 0..num_roots {
            let permission = match &self.permissions {
                StringOrVec::Single(s) => s.clone(),
                StringOrVec::Vec(v) => v[i].clone(),
            };

            let link = if !self.links.is_empty() {
                let link_str = self.links[i].trim();
                if link_str.is_empty() {
                    None
                } else {
                    Some(link_str.to_string())
                }
            } else {
                None
            };

            paths.push(PathConfig::new(
                self.roots[i].clone(),
                self.subpath.clone(),
                permission,
                link,
            ));
        }
        paths
    }
}

/// Helper type to allow either a single string or a vector of strings in TOML
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum StringOrVec {
    Single(String),
    Vec(Vec<String>),
}
