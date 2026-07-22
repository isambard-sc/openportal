// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Award state — one JSON file per project, under a configured
//! `state_dir`. Unlike `op-cloudaccount`'s assignment state, this is read
//! fresh from disk on every call rather than cached in memory: approval
//! (see `main::run_cli_command`) happens via a separate one-off CLI
//! invocation while the main `run` server process is running
//! continuously, so a write-through cache here would go stale the moment
//! the CLI process edits a file. See
//! `docs/plans/op-cloudportal-design.md` §5 for the reasoning.

use once_cell::sync::Lazy;
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;

use serde::{Deserialize, Serialize};
use templemeads::grammar::{AwardDetails, Note, ProjectIdentifier, ProjectMapping, UserMapping};
use templemeads::portal_identifier::PortalIdentifier;
use templemeads::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AwardStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwardRecord {
    project: ProjectIdentifier,
    details: AwardDetails,
    offering: String,
    status: AwardStatus,
    #[serde(default)]
    provisioned_users: Vec<String>,
}

impl AwardRecord {
    pub fn project(&self) -> &ProjectIdentifier {
        &self.project
    }

    pub fn details(&self) -> &AwardDetails {
        &self.details
    }

    pub fn offering(&self) -> &str {
        &self.offering
    }

    /// Members (by email) from `AwardDetails.members` that have not yet
    /// been provisioned on the resolved `op-cloudaccount`.
    pub fn unprovisioned_members(&self) -> Vec<String> {
        self.details
            .members()
            .unwrap_or_default()
            .into_keys()
            .filter(|email| !self.provisioned_users.contains(email))
            .collect()
    }

    fn mapping(&self) -> Result<ProjectMapping, Error> {
        ProjectMapping::new(&self.project, &self.project.to_string())
    }
}

static STATE_DIR: Lazy<RwLock<Option<PathBuf>>> = Lazy::new(|| RwLock::new(None));

pub async fn initialise(state_dir: &Path) -> Result<(), Error> {
    tokio::fs::create_dir_all(state_dir).await.map_err(|e| {
        Error::Failed(format!(
            "Cannot create cloudportal state-dir '{}': {}",
            state_dir.display(),
            e
        ))
    })?;

    *STATE_DIR.write().await = Some(state_dir.to_path_buf());

    Ok(())
}

async fn state_dir() -> Result<PathBuf, Error> {
    STATE_DIR.read().await.clone().ok_or_else(|| {
        Error::Misconfigured("cloudportal state store has not been initialised".to_string())
    })
}

fn record_path(state_dir: &Path, project: &ProjectIdentifier) -> PathBuf {
    state_dir.join(format!("{}.json", project))
}

async fn read_record(project: &ProjectIdentifier) -> Result<Option<AwardRecord>, Error> {
    let path = record_path(&state_dir().await?, project);

    match tokio::fs::read_to_string(&path).await {
        Ok(contents) => Ok(Some(serde_json::from_str(&contents)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Failed(format!(
            "Cannot read Award state file '{}': {}",
            path.display(),
            e
        ))),
    }
}

async fn write_record(record: &AwardRecord) -> Result<(), Error> {
    let dir = state_dir().await?;
    let path = record_path(&dir, &record.project);
    let tmp_path = path.with_extension("json.tmp");

    let contents = serde_json::to_string_pretty(record)?;

    tokio::fs::write(&tmp_path, contents).await.map_err(|e| {
        Error::Failed(format!(
            "Cannot write Award state file '{}': {}",
            tmp_path.display(),
            e
        ))
    })?;

    tokio::fs::rename(&tmp_path, &path).await.map_err(|e| {
        Error::Failed(format!(
            "Cannot finalise Award state file '{}': {}",
            path.display(),
            e
        ))
    })?;

    Ok(())
}

/// Every Award record currently on disk. Used by the CLI (`list-pending`)
/// and the background provisioning poller (`approved_unprovisioned`).
async fn read_all_records() -> Result<Vec<AwardRecord>, Error> {
    let dir = state_dir().await?;
    let mut records = Vec::new();

    let mut entries = tokio::fs::read_dir(&dir).await.map_err(|e| {
        Error::Failed(format!(
            "Cannot read cloudportal state-dir '{}': {}",
            dir.display(),
            e
        ))
    })?;

    while let Some(entry) = entries.next_entry().await.map_err(Error::IO)? {
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => match serde_json::from_str::<AwardRecord>(&contents) {
                Ok(record) => records.push(record),
                Err(e) => {
                    tracing::warn!(
                        "Could not parse Award state file '{}': {}. Skipping.",
                        path.display(),
                        e
                    );
                }
            },
            Err(e) => {
                tracing::warn!(
                    "Could not read Award state file '{}': {}. Skipping.",
                    path.display(),
                    e
                );
            }
        }
    }

    Ok(records)
}

/// Create (or, if it already exists, idempotently return) an Award.
pub async fn create_award(
    project: &ProjectIdentifier,
    details: &AwardDetails,
    offering: &str,
) -> Result<ProjectMapping, Error> {
    if let Some(existing) = read_record(project).await? {
        return existing.mapping();
    }

    let record = AwardRecord {
        project: project.clone(),
        details: details.clone(),
        offering: offering.to_string(),
        status: AwardStatus::Pending,
        provisioned_users: Vec::new(),
    };

    write_record(&record).await?;
    record.mapping()
}

pub async fn update_award(
    project: &ProjectIdentifier,
    details: &AwardDetails,
) -> Result<ProjectMapping, Error> {
    let mut record = read_record(project).await?.ok_or_else(|| {
        Error::NotFound(format!(
            "No Award '{}' has been created on this portal",
            project
        ))
    })?;

    record.details = record.details.merge(details)?;

    write_record(&record).await?;
    record.mapping()
}

pub async fn remove_award(project: &ProjectIdentifier) -> Result<ProjectMapping, Error> {
    let dir = state_dir().await?;
    let mapping = ProjectMapping::new(project, &project.to_string())?;
    let path = record_path(&dir, project);

    match tokio::fs::remove_file(&path).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(Error::Failed(format!(
                "Cannot remove Award state file '{}': {}",
                path.display(),
                e
            )))
        }
    }

    Ok(mapping)
}

pub async fn get_award(project: &ProjectIdentifier) -> Result<AwardDetails, Error> {
    let record = read_record(project).await?.ok_or_else(|| {
        Error::NotFound(format!(
            "No Award '{}' has been created on this portal",
            project
        ))
    })?;

    Ok(record.details)
}

pub async fn get_awards(portal: &PortalIdentifier) -> Result<Vec<AwardDetails>, Error> {
    Ok(read_all_records()
        .await?
        .into_iter()
        .filter(|r| r.project.portal() == portal.portal())
        .map(|r| r.details)
        .collect())
}

pub async fn get_projects(portal: &PortalIdentifier) -> Result<Vec<ProjectMapping>, Error> {
    read_all_records()
        .await?
        .iter()
        .filter(|r| r.project.portal() == portal.portal())
        .map(|r| r.mapping())
        .collect()
}

pub async fn get_project_mapping(project: &ProjectIdentifier) -> Result<ProjectMapping, Error> {
    let record = read_record(project).await?.ok_or_else(|| {
        Error::NotFound(format!(
            "No Award '{}' has been created on this portal",
            project
        ))
    })?;

    record.mapping()
}

/// Users derived from `AwardDetails.members` - see `identity.rs` for how
/// an email is turned into a `UserMapping`.
pub async fn get_users(project: &ProjectIdentifier) -> Result<Vec<UserMapping>, Error> {
    let record = read_record(project).await?.ok_or_else(|| {
        Error::NotFound(format!(
            "No Award '{}' has been created on this portal",
            project
        ))
    })?;

    let local_group = project.to_string();

    record
        .details
        .members()
        .unwrap_or_default()
        .into_keys()
        .map(|email| crate::identity::user_mapping_for_email(project, &email, &local_group))
        .collect()
}

pub async fn get_offering(project: &ProjectIdentifier) -> Result<String, Error> {
    let record = read_record(project).await?.ok_or_else(|| {
        Error::NotFound(format!(
            "No Award '{}' has been created on this portal",
            project
        ))
    })?;

    Ok(record.offering)
}

/// Every Award with `status: pending` - used by the `list-pending` CLI subcommand.
pub async fn list_pending() -> Result<Vec<AwardRecord>, Error> {
    Ok(read_all_records()
        .await?
        .into_iter()
        .filter(|r| r.status == AwardStatus::Pending)
        .collect())
}

/// Every Award with `status: approved` that still has unprovisioned
/// members - used by the background provisioning poller.
pub async fn approved_unprovisioned() -> Result<Vec<AwardRecord>, Error> {
    Ok(read_all_records()
        .await?
        .into_iter()
        .filter(|r| r.status == AwardStatus::Approved && !r.unprovisioned_members().is_empty())
        .collect())
}

/// A pure file edit - flips `status` to `approved`. Does not itself talk
/// to `op-cloudaccount`; the background poller in `main.rs` does the
/// actual provisioning on its next cycle (design doc §7).
pub async fn approve(project: &ProjectIdentifier) -> Result<(), Error> {
    let mut record = read_record(project).await?.ok_or_else(|| {
        Error::NotFound(format!(
            "No Award '{}' has been created on this portal",
            project
        ))
    })?;

    record.status = AwardStatus::Approved;
    write_record(&record).await
}

pub async fn reject(project: &ProjectIdentifier, reason: Option<&str>) -> Result<(), Error> {
    let mut record = read_record(project).await?.ok_or_else(|| {
        Error::NotFound(format!(
            "No Award '{}' has been created on this portal",
            project
        ))
    })?;

    record.status = AwardStatus::Rejected;

    if let Some(reason) = reason {
        record.details.add_note(Note::new("cloud-operator", reason));
    }

    write_record(&record).await
}

/// Record that `email` has now been provisioned on `op-cloudaccount` for
/// `project` - called by the background poller as each member succeeds.
pub async fn mark_provisioned(project: &ProjectIdentifier, email: &str) -> Result<(), Error> {
    let mut record = read_record(project).await?.ok_or_else(|| {
        Error::NotFound(format!(
            "No Award '{}' has been created on this portal",
            project
        ))
    })?;

    if !record.provisioned_users.iter().any(|u| u == email) {
        record.provisioned_users.push(email.to_string());
    }

    write_record(&record).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use templemeads::grammar::ProjectTemplate;

    fn test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("op-cloudportal-test-state-{}", name))
    }

    fn award(name: &str) -> AwardDetails {
        let mut details = AwardDetails::new();
        details.set_name(name);
        details.set_template(ProjectTemplate::parse("aws").unwrap());
        details
    }

    // A single consolidated test - `STATE_DIR` is one process-wide static,
    // so multiple `#[tokio::test]` functions each calling `initialise()`
    // race each other if cargo runs them concurrently (which it does by
    // default). One test function avoids that entirely.
    #[tokio::test]
    async fn test_state_store() {
        let dir = test_dir("state-store");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        initialise(&dir).await.unwrap();

        let portal = PortalIdentifier::parse("testportal").unwrap();

        // --- round trip, idempotent create, and merge-on-update ---
        let project = ProjectIdentifier::parse("roundtrip.testportal").unwrap();

        create_award(&project, &award("My Project"), "aws")
            .await
            .unwrap();

        assert_eq!(
            get_award(&project).await.unwrap().name(),
            Some("My Project".to_string())
        );
        assert_eq!(get_projects(&portal).await.unwrap().len(), 1);
        assert_eq!(get_awards(&portal).await.unwrap().len(), 1);

        // create_award is idempotent - calling it again with different
        // details must not overwrite the existing record
        create_award(&project, &award("Different Name"), "aws")
            .await
            .unwrap();
        assert_eq!(
            get_award(&project).await.unwrap().name(),
            Some("My Project".to_string())
        );

        // update_project merges rather than replaces
        let mut update = AwardDetails::new();
        update.set_description("a description added later");
        update_award(&project, &update).await.unwrap();

        let merged = get_award(&project).await.unwrap();
        assert_eq!(merged.name(), Some("My Project".to_string()));
        assert_eq!(
            merged.description(),
            Some("a description added later".to_string())
        );

        remove_award(&project).await.unwrap();
        assert!(get_award(&project).await.is_err());

        // --- approve/reject are pure file edits ---
        let pending_project = ProjectIdentifier::parse("pending.testportal").unwrap();
        let approved_project = ProjectIdentifier::parse("approved.testportal").unwrap();
        let rejected_project = ProjectIdentifier::parse("rejected.testportal").unwrap();

        for project in [&pending_project, &approved_project, &rejected_project] {
            create_award(project, &award("Project"), "aws")
                .await
                .unwrap();
        }

        approve(&approved_project).await.unwrap();
        reject(&rejected_project, Some("not eligible"))
            .await
            .unwrap();

        let pending = list_pending().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].project(), &pending_project);

        let rejected_details = get_award(&rejected_project).await.unwrap();
        assert!(rejected_details
            .notes()
            .iter()
            .any(|n| n.text() == "not eligible"));

        // --- provisioning tracking ---
        let prov_project = ProjectIdentifier::parse("provtest.testportal").unwrap();
        let mut details = award("Project");
        details.add_member("alice@example.com", "member").unwrap();
        details.add_member("bob@example.com", "member").unwrap();

        create_award(&prov_project, &details, "aws").await.unwrap();

        // not yet approved - shouldn't show up for provisioning
        assert!(!approved_unprovisioned()
            .await
            .unwrap()
            .iter()
            .any(|r| r.project() == &prov_project));

        approve(&prov_project).await.unwrap();

        let approved = approved_unprovisioned().await.unwrap();
        let prov_record = approved
            .iter()
            .find(|r| r.project() == &prov_project)
            .unwrap();
        assert_eq!(prov_record.unprovisioned_members().len(), 2);

        mark_provisioned(&prov_project, "alice@example.com")
            .await
            .unwrap();

        let approved = approved_unprovisioned().await.unwrap();
        let prov_record = approved
            .iter()
            .find(|r| r.project() == &prov_project)
            .unwrap();
        assert_eq!(
            prov_record.unprovisioned_members(),
            vec!["bob@example.com".to_string()]
        );

        mark_provisioned(&prov_project, "bob@example.com")
            .await
            .unwrap();

        // fully provisioned - no longer needs polling
        assert!(!approved_unprovisioned()
            .await
            .unwrap()
            .iter()
            .any(|r| r.project() == &prov_project));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
