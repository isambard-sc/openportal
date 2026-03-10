// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::Error;

use crate::grammar::{NamedType, ProjectIdentifier, UserIdentifier, UserMapping};
use crate::storage::{Quota, Volume};

impl NamedType for ProjectStorageReport {
    fn type_name() -> &'static str {
        "ProjectStorageReport"
    }
}

impl NamedType for Vec<ProjectStorageReport> {
    fn type_name() -> &'static str {
        "Vec<ProjectStorageReport>"
    }
}

/// A report of the storage quotas and usage for a single project,
/// including per-user quotas. The report reflects the state at the
/// time it was generated (stored in `generated_at`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectStorageReport {
    /// The project this report is for
    project: ProjectIdentifier,

    /// When this report was generated (always "now" — no historical data)
    generated_at: DateTime<Utc>,

    /// Project-level quotas keyed by volume name (e.g. "home", "scratch")
    project_quotas: HashMap<Volume, Quota>,

    /// Per-user quotas: portal UserIdentifier → (volume → quota)
    user_quotas: HashMap<UserIdentifier, HashMap<Volume, Quota>>,

    /// Mapping from portal UserIdentifier to local username
    users: HashMap<UserIdentifier, String>,
}

impl ProjectStorageReport {
    /// Create a new, empty report for the given project, timestamped now.
    pub fn new(project: &ProjectIdentifier) -> Self {
        Self {
            project: project.clone(),
            generated_at: Utc::now(),
            project_quotas: HashMap::new(),
            user_quotas: HashMap::new(),
            users: HashMap::new(),
        }
    }

    /// Return the project identifier.
    pub fn project(&self) -> &ProjectIdentifier {
        &self.project
    }

    /// Return when the report was generated.
    pub fn generated_at(&self) -> &DateTime<Utc> {
        &self.generated_at
    }

    /// Set the project-level quotas.
    pub fn set_project_quotas(&mut self, quotas: HashMap<Volume, Quota>) {
        self.project_quotas = quotas;
    }

    /// Return the project-level quotas.
    pub fn project_quotas(&self) -> &HashMap<Volume, Quota> {
        &self.project_quotas
    }

    /// Add (or replace) the per-volume quotas for a single user.
    pub fn add_user_quotas(&mut self, user: &UserIdentifier, quotas: HashMap<Volume, Quota>) {
        if !quotas.is_empty() {
            self.user_quotas.insert(user.clone(), quotas);
        }
    }

    /// Return the per-user quotas.
    pub fn user_quotas(&self) -> &HashMap<UserIdentifier, HashMap<Volume, Quota>> {
        &self.user_quotas
    }

    /// Record the mapping from a portal user to their local username.
    pub fn add_mapping(&mut self, mapping: &UserMapping) -> Result<(), Error> {
        self.users
            .insert(mapping.user().clone(), mapping.local_user().to_string());
        Ok(())
    }

    /// Bulk-add user mappings from a slice of UserMappings.
    pub fn add_mappings(&mut self, mappings: &[UserMapping]) -> Result<(), Error> {
        for mapping in mappings {
            match self.add_mapping(mapping) {
                Ok(_) => (),
                Err(e) => {
                    tracing::warn!("Failed to add mapping: {}", e);
                }
            }
        }
        Ok(())
    }

    /// Return the portal-user → local-username map.
    pub fn users(&self) -> &HashMap<UserIdentifier, String> {
        &self.users
    }

    /// Return true if the report contains no quota data at all.
    pub fn is_empty(&self) -> bool {
        self.project_quotas.is_empty() && self.user_quotas.is_empty()
    }

    /// Serialise to a JSON string.
    pub fn to_json(&self) -> Result<String, Error> {
        serde_json::to_string(self)
            .context("Failed to serialise ProjectStorageReport to JSON")
            .map_err(Error::from)
    }

    /// Deserialise from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, Error> {
        serde_json::from_str(json)
            .context("Failed to deserialise ProjectStorageReport from JSON")
            .map_err(Error::from)
    }
}

impl std::fmt::Display for ProjectStorageReport {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        writeln!(
            f,
            "Storage report for {} (generated {})",
            self.project,
            self.generated_at.format("%Y-%m-%d %H:%M:%S UTC")
        )?;

        if self.project_quotas.is_empty() && self.user_quotas.is_empty() {
            return writeln!(f, "  (no quota data)");
        }

        if !self.project_quotas.is_empty() {
            writeln!(f, "  Project quotas:")?;
            let mut volumes: Vec<&Volume> = self.project_quotas.keys().collect();
            volumes.sort_by_key(|v| v.name());
            for volume in volumes {
                if let Some(quota) = self.project_quotas.get(volume) {
                    writeln!(f, "    {}: {}", volume, quota)?;
                }
            }
        }

        if !self.user_quotas.is_empty() {
            writeln!(f, "  User quotas:")?;
            let mut users: Vec<&UserIdentifier> = self.user_quotas.keys().collect();
            users.sort_by_key(|u| u.to_string());
            for user in users {
                let local = self
                    .users
                    .get(user)
                    .map(|s| s.as_str())
                    .unwrap_or("unknown");
                writeln!(f, "    {} ({}):", user, local)?;
                if let Some(quotas) = self.user_quotas.get(user) {
                    let mut volumes: Vec<&Volume> = quotas.keys().collect();
                    volumes.sort_by_key(|v| v.name());
                    for volume in volumes {
                        if let Some(quota) = quotas.get(volume) {
                            writeln!(f, "      {}: {}", volume, quota)?;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
