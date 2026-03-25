// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::Error;

use crate::grammar::{
    Date, DateRange, NamedType, PortalIdentifier, ProjectIdentifier, UserIdentifier, UserMapping,
};
use crate::storage::{Quota, Volume};

impl NamedType for StorageReport {
    fn type_name() -> &'static str {
        "StorageReport"
    }
}

impl NamedType for Vec<StorageReport> {
    fn type_name() -> &'static str {
        "Vec<StorageReport>"
    }
}

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

/// An internal point-in-time snapshot of storage quota data for a single
/// project. Used to store historical entries inside `ProjectStorageReport`.
/// Not exposed through the public API — callers always receive
/// `ProjectStorageReport` even for individual days.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DailyStorageReport {
    project: ProjectIdentifier,
    generated_at: DateTime<Utc>,
    project_quotas: HashMap<Volume, Quota>,
    user_quotas: HashMap<UserIdentifier, HashMap<Volume, Quota>>,
}

impl DailyStorageReport {
    #[allow(dead_code)]
    pub fn project(&self) -> &ProjectIdentifier {
        &self.project
    }

    pub fn generated_at(&self) -> &DateTime<Utc> {
        &self.generated_at
    }

    /// Update the project identifier and rebuild `user_quotas` keys so that
    /// every `UserIdentifier` reflects the new project and portal.
    pub(crate) fn remap_project(
        &mut self,
        new_project: &ProjectIdentifier,
    ) -> Result<(), crate::error::Error> {
        self.project = new_project.clone();

        let old_quotas = std::mem::take(&mut self.user_quotas);
        let mut new_quotas = HashMap::with_capacity(old_quotas.len());

        for (uid, quotas) in old_quotas {
            let new_uid = UserIdentifier::parse(&format!(
                "{}.{}.{}",
                uid.username(),
                new_project.project(),
                new_project.portal()
            ))
            .with_context(|| {
                format!(
                    "remap_project: failed to rebuild UserIdentifier for user {}",
                    uid
                )
            })?;
            new_quotas.insert(new_uid, quotas);
        }

        self.user_quotas = new_quotas;
        Ok(())
    }
}

/// Convert a `ProjectStorageReport` to a `DailyStorageReport` by stripping
/// the historical `daily_reports` map.
impl From<&ProjectStorageReport> for DailyStorageReport {
    fn from(report: &ProjectStorageReport) -> Self {
        Self {
            project: report.project.clone(),
            generated_at: report.generated_at,
            project_quotas: report.project_quotas.clone(),
            user_quotas: report.user_quotas.clone(),
        }
    }
}

/// Promote a `DailyStorageReport` back to a `ProjectStorageReport` with an
/// empty historical map. Used when returning historical snapshots to callers.
impl From<DailyStorageReport> for ProjectStorageReport {
    fn from(snapshot: DailyStorageReport) -> Self {
        Self {
            project: snapshot.project,
            generated_at: snapshot.generated_at,
            project_quotas: snapshot.project_quotas,
            user_quotas: snapshot.user_quotas,
            users: HashMap::new(),
            daily_reports: HashMap::new(),
        }
    }
}

/// A report of the storage quotas and usage for a single project, including
/// per-user quotas. The top-level fields always hold the **most recent**
/// snapshot. Older point-in-time snapshots are stored in `daily_reports`,
/// keyed by calendar date (UTC), with at most one snapshot per date (the
/// newest seen for that date). The date of the top-level snapshot is never
/// duplicated in `daily_reports`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectStorageReport {
    /// The project this report is for
    project: ProjectIdentifier,

    /// When the top-level snapshot was generated
    generated_at: DateTime<Utc>,

    /// Project-level quotas keyed by volume name (e.g. "home", "scratch")
    project_quotas: HashMap<Volume, Quota>,

    /// Per-user quotas: portal UserIdentifier → (volume → quota)
    user_quotas: HashMap<UserIdentifier, HashMap<Volume, Quota>>,

    /// Mapping from portal UserIdentifier to local username
    users: HashMap<UserIdentifier, String>,

    /// Historical snapshots keyed by date (UTC). Each entry is the newest
    /// snapshot seen for that day. The current top-level date is excluded.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    daily_reports: HashMap<Date, DailyStorageReport>,
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
            daily_reports: HashMap::new(),
        }
    }

    /// Return the project identifier.
    pub fn project(&self) -> &ProjectIdentifier {
        &self.project
    }

    /// Return when the top-level snapshot was generated.
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

    /// Return the portal users as a sorted list of UserIdentifiers.
    pub fn users(&self) -> Vec<UserIdentifier> {
        let mut users: Vec<UserIdentifier> = self.users.keys().cloned().collect();
        users.sort_by_cached_key(|u| u.to_string());
        users
    }

    /// Return the full portal-user → local-username map.
    pub fn user_mapping(&self) -> HashMap<UserIdentifier, String> {
        self.users.clone()
    }

    /// Return true if the top-level snapshot contains no quota data at all.
    /// Historical entries in `daily_reports` are not considered.
    pub fn is_empty(&self) -> bool {
        self.project_quotas.is_empty() && self.user_quotas.is_empty()
    }

    /// Return all snapshots sorted by date (oldest first), each promoted to a
    /// `ProjectStorageReport` with an empty history map. Includes both the
    /// historical entries and the current top-level snapshot.
    ///
    /// When `with_usage_only` is `true` (the default in Python), only snapshots
    /// that have quota data (`!is_empty()`) are returned. When `false`, every
    /// calendar date in the range [earliest, latest] is included; dates with no
    /// snapshot are represented by an empty `ProjectStorageReport`.
    pub fn daily_reports(&self, with_usage_only: bool) -> Vec<ProjectStorageReport> {
        // Collect historical snapshots as (date, report) pairs
        let mut snapshots: Vec<(Date, ProjectStorageReport)> = self
            .daily_reports
            .iter()
            .map(|(date, snapshot)| {
                let mut report = ProjectStorageReport::from(snapshot.clone());
                report.users = self.users.clone();
                (date.clone(), report)
            })
            .collect();

        // Append the current top-level snapshot
        let current_date = Date::from_chrono(&self.generated_at.date_naive());
        snapshots.push((
            current_date.clone(),
            ProjectStorageReport {
                project: self.project.clone(),
                generated_at: self.generated_at,
                project_quotas: self.project_quotas.clone(),
                user_quotas: self.user_quotas.clone(),
                users: self.users.clone(),
                daily_reports: HashMap::new(),
            },
        ));

        snapshots.sort_by_key(|(d, _)| d.clone());

        if with_usage_only {
            return snapshots
                .into_iter()
                .filter(|(_, r)| !r.is_empty())
                .map(|(_, r)| r)
                .collect();
        }

        // Fill in every calendar date between earliest and latest
        if snapshots.is_empty() {
            return Vec::new();
        }

        let earliest = snapshots.first().map(|(d, _)| d.clone()).unwrap_or(current_date.clone());
        let latest = snapshots.last().map(|(d, _)| d.clone()).unwrap_or(current_date);
        let snapshot_map: HashMap<Date, ProjectStorageReport> =
            snapshots.into_iter().collect();

        let mut result = Vec::new();
        let mut date = earliest;
        loop {
            if let Some(report) = snapshot_map.get(&date) {
                result.push(report.clone());
            } else {
                result.push(ProjectStorageReport {
                    project: self.project.clone(),
                    generated_at: chrono::NaiveDateTime::new(
                        date.to_chrono(),
                        chrono::NaiveTime::MIN,
                    )
                    .and_utc(),
                    project_quotas: HashMap::new(),
                    user_quotas: HashMap::new(),
                    users: HashMap::new(),
                    daily_reports: HashMap::new(),
                });
            }
            let next = date.next();
            if next <= latest {
                date = next;
            } else {
                break;
            }
        }
        result
    }

    /// Return the snapshot for a specific calendar date as a
    /// `ProjectStorageReport`. If `date` matches the current top-level date,
    /// returns the top-level data (without nested history). Returns an empty
    /// report if no snapshot exists for the requested date.
    pub fn get_report(&self, date: &Date) -> ProjectStorageReport {
        let current_date = Date::from_chrono(&self.generated_at.date_naive());
        if *date == current_date {
            ProjectStorageReport {
                project: self.project.clone(),
                generated_at: self.generated_at,
                project_quotas: self.project_quotas.clone(),
                user_quotas: self.user_quotas.clone(),
                users: self.users.clone(),
                daily_reports: HashMap::new(),
            }
        } else {
            self.daily_reports
                .get(date)
                .cloned()
                .map(|snapshot| {
                    let mut report = ProjectStorageReport::from(snapshot);
                    report.users = self.users.clone();
                    report
                })
                .unwrap_or_else(|| ProjectStorageReport::new(&self.project))
        }
    }

    /// Return a copy of this report containing only historical snapshots whose
    /// date falls within `range` (inclusive on both ends). The top-level
    /// (current) snapshot fields are preserved unchanged.
    pub fn filter(&self, range: &DateRange) -> Self {
        let daily_reports = self
            .daily_reports
            .iter()
            .filter(|(date, _)| *date >= range.start_date() && *date <= range.end_date())
            .map(|(date, report)| (date.clone(), report.clone()))
            .collect();

        Self {
            project: self.project.clone(),
            generated_at: self.generated_at,
            project_quotas: self.project_quotas.clone(),
            user_quotas: self.user_quotas.clone(),
            users: self.users.clone(),
            daily_reports,
        }
    }

    /// Remap this report to a new project identifier.
    ///
    /// Updates the top-level `project` field and rebuilds the `users` and
    /// `user_quotas` maps so that every `UserIdentifier` key reflects the new
    /// project and portal (i.e. `username.old_project.old_portal` becomes
    /// `username.new_project.new_portal`).  Historical snapshots in
    /// `daily_reports` are updated in the same way.
    pub fn remap_project(&mut self, new_project: &ProjectIdentifier) -> Result<(), Error> {
        self.project = new_project.clone();

        let old_users = std::mem::take(&mut self.users);
        let mut new_users = HashMap::with_capacity(old_users.len());
        for (uid, local) in old_users {
            let new_uid = UserIdentifier::parse(&format!(
                "{}.{}.{}",
                uid.username(),
                new_project.project(),
                new_project.portal()
            ))
            .with_context(|| {
                format!(
                    "remap_project: failed to rebuild UserIdentifier for user {}",
                    uid
                )
            })?;
            new_users.insert(new_uid, local);
        }
        self.users = new_users;

        let old_quotas = std::mem::take(&mut self.user_quotas);
        let mut new_quotas = HashMap::with_capacity(old_quotas.len());
        for (uid, quotas) in old_quotas {
            let new_uid = UserIdentifier::parse(&format!(
                "{}.{}.{}",
                uid.username(),
                new_project.project(),
                new_project.portal()
            ))
            .with_context(|| {
                format!(
                    "remap_project: failed to rebuild UserIdentifier for user {}",
                    uid
                )
            })?;
            new_quotas.insert(new_uid, quotas);
        }
        self.user_quotas = new_quotas;

        for snapshot in self.daily_reports.values_mut() {
            snapshot.remap_project(new_project)?;
        }

        Ok(())
    }

    /// Remap this report to a new portal, keeping the project name unchanged.
    ///
    /// Convenience wrapper around [`ProjectStorageReport::remap_project`] that
    /// constructs the new `ProjectIdentifier` as
    /// `self.project.project().new_portal`.
    pub fn remap_portal(&mut self, new_portal: &PortalIdentifier) -> Result<(), Error> {
        let new_project = ProjectIdentifier::parse(&format!(
            "{}.{}",
            self.project.project(),
            new_portal.portal()
        ))
        .with_context(|| {
            format!(
                "remap_portal: failed to rebuild ProjectIdentifier for {}",
                self.project
            )
        })?;
        self.remap_project(&new_project)
    }

    /// Remap the local username strings for a set of users.
    ///
    /// `new_usermapping` maps each `UserIdentifier` (as it currently appears
    /// in this report's `users` map) to a new local-username string.  Only
    /// users present in both `self.users` and `new_usermapping` are updated;
    /// others are left unchanged.
    ///
    /// Returns an error if the remapping would cause two distinct users to
    /// share the same local-username string.
    pub fn remap_users(
        &mut self,
        new_usermapping: &HashMap<UserIdentifier, String>,
    ) -> Result<(), Error> {
        // Check that the remapping is injective.
        let mut seen: HashMap<String, &UserIdentifier> = HashMap::with_capacity(self.users.len());
        for (uid, old_local) in &self.users {
            let new_local = new_usermapping
                .get(uid)
                .map(String::as_str)
                .unwrap_or(old_local.as_str());
            if let Some(other_uid) = seen.insert(new_local.to_string(), uid) {
                return Err(Error::InvalidState(format!(
                    "remap_users would merge users '{}' and '{}' into the same local \
                     username '{}'",
                    uid, other_uid, new_local
                )));
            }
        }

        for (uid, local) in self.users.iter_mut() {
            if let Some(new_local) = new_usermapping.get(uid) {
                *local = new_local.clone();
            }
        }

        Ok(())
    }

    /// Combine a slice of `ProjectStorageReport`s into a single report using
    /// the merge semantics: newest snapshot wins at the top level, older
    /// snapshots are retained in `daily_reports` (one per date, newest wins).
    pub fn combine(reports: &[ProjectStorageReport]) -> Result<Self, Error> {
        if reports.is_empty() {
            return Err(Error::InvalidState("No reports to combine".to_string()));
        }

        let mut combined = reports[0].clone();

        for report in &reports[1..] {
            if report.project != combined.project {
                return Err(Error::Incompatible(format!(
                    "Cannot combine reports for different projects: {} and {}",
                    report.project, combined.project
                )));
            }
            combined += report.clone();
        }

        Ok(combined)
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

/// Merge two reports for the same project. The newer snapshot (by
/// `generated_at`) becomes the top-level state. The older snapshot is
/// stored in `daily_reports` under its date, unless it falls on the same
/// calendar day as the newer snapshot, in which case it is discarded
/// (the newer one already represents that day). Historical entries from
/// both reports are merged, keeping the newest snapshot per date.
impl std::ops::Add<ProjectStorageReport> for ProjectStorageReport {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        if self.project != other.project {
            tracing::warn!(
                "Cannot merge storage reports for different projects: {} and {}",
                self.project,
                other.project
            );
            return self;
        }

        let (mut newest, older) = if self.generated_at >= other.generated_at {
            (self, other)
        } else {
            (other, self)
        };

        // Merge users from both sides into the top-level users map.
        // The newest report's entries take precedence (already in newest.users).
        for (user, local) in &older.users {
            newest
                .users
                .entry(user.clone())
                .or_insert_with(|| local.clone());
        }

        let newest_date = Date::from_chrono(&newest.generated_at.date_naive());
        let older_date = Date::from_chrono(&older.generated_at.date_naive());

        // Store older's top-level snapshot in daily_reports if it belongs to
        // a different date, keeping the newest snapshot for that date.
        if older_date != newest_date {
            let older_snapshot = DailyStorageReport::from(&older);
            newest
                .daily_reports
                .entry(older_date)
                .and_modify(|existing| {
                    if older.generated_at > existing.generated_at {
                        *existing = older_snapshot.clone();
                    }
                })
                .or_insert(older_snapshot);
        }

        // Merge historical entries from older, skipping any that fall on the
        // newest top-level date.
        for (date, snapshot) in older.daily_reports {
            if date == newest_date {
                continue;
            }
            newest
                .daily_reports
                .entry(date)
                .and_modify(|existing| {
                    if snapshot.generated_at > existing.generated_at {
                        *existing = snapshot.clone();
                    }
                })
                .or_insert(snapshot);
        }

        newest
    }
}

impl std::ops::AddAssign<ProjectStorageReport> for ProjectStorageReport {
    fn add_assign(&mut self, other: Self) {
        *self = self.clone() + other;
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
            writeln!(f, "  (no quota data)")?;
        } else {
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
        }

        if !self.daily_reports.is_empty() {
            let mut dates: Vec<&Date> = self.daily_reports.keys().collect();
            dates.sort();
            writeln!(f, "  Historical snapshots ({} day(s)):", dates.len())?;
            for date in dates {
                if let Some(snap) = self.daily_reports.get(date) {
                    writeln!(
                        f,
                        "    {} ({})",
                        date,
                        snap.generated_at().format("%H:%M:%S UTC")
                    )?;
                }
            }
        }

        Ok(())
    }
}

/// A portal-level storage report containing per-project storage reports for
/// all projects associated with a portal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageReport {
    portal: PortalIdentifier,
    reports: HashMap<ProjectIdentifier, ProjectStorageReport>,
}

impl StorageReport {
    /// Create a new, empty report for the given portal.
    pub fn new(portal: &PortalIdentifier) -> Self {
        Self {
            portal: portal.clone(),
            reports: HashMap::new(),
        }
    }

    /// Return the portal identifier.
    pub fn portal(&self) -> &PortalIdentifier {
        &self.portal
    }

    /// Return the sorted list of projects with reports.
    pub fn projects(&self) -> Vec<ProjectIdentifier> {
        let mut projects: Vec<ProjectIdentifier> = self.reports.keys().cloned().collect();
        projects.sort_by_cached_key(|p| p.to_string());
        projects
    }

    /// Return the combined portal-user → local-username map across all
    /// contained project reports.
    pub fn user_mapping(&self) -> HashMap<UserIdentifier, String> {
        self.reports
            .values()
            .flat_map(|r| r.user_mapping())
            .collect()
    }

    /// Return the storage report for a project, or an empty report if not
    /// present.
    pub fn get_report(&self, project: &ProjectIdentifier) -> ProjectStorageReport {
        self.reports
            .get(project)
            .cloned()
            .unwrap_or_else(|| ProjectStorageReport::new(project))
    }

    /// Add or replace the per-project report. Returns an error if the project
    /// does not belong to this portal.
    pub fn set_report(&mut self, report: ProjectStorageReport) -> Result<(), Error> {
        if report.project().portal_identifier() != *self.portal() {
            return Err(Error::InvalidState(format!(
                "Report for wrong portal: {}. This report is for {}",
                report.project().portal_identifier(),
                self.portal
            )));
        }
        self.reports.insert(report.project().clone(), report);
        Ok(())
    }

    /// Return true if there are no project reports.
    pub fn is_empty(&self) -> bool {
        self.reports.is_empty()
    }

    /// Return a copy of this report with every contained `ProjectStorageReport`
    /// filtered to only the historical snapshots that fall within `range`
    /// (inclusive). The top-level snapshot fields of each project report are
    /// preserved unchanged.
    pub fn filter(&self, range: &DateRange) -> Self {
        let reports = self
            .reports
            .iter()
            .map(|(project, report)| (project.clone(), report.filter(range)))
            .collect();

        Self {
            portal: self.portal.clone(),
            reports,
        }
    }

    /// Remap all projects in this report to a new portal.
    ///
    /// Updates `self.portal` and remaps every contained `ProjectStorageReport`
    /// so that its project identifier keeps the same project name but uses the
    /// new portal, e.g. `project.portal` → `project.new_portal`.
    pub fn remap_portal(&mut self, new_portal: &PortalIdentifier) -> Result<(), Error> {
        self.portal = new_portal.clone();

        let old_reports = std::mem::take(&mut self.reports);
        let mut new_reports = HashMap::with_capacity(old_reports.len());

        for (old_proj_id, mut proj_report) in old_reports {
            let new_proj_id = ProjectIdentifier::parse(&format!(
                "{}.{}",
                old_proj_id.project(),
                new_portal.portal()
            ))
            .with_context(|| {
                format!(
                    "remap_portal: failed to rebuild ProjectIdentifier for {}",
                    old_proj_id
                )
            })?;
            proj_report.remap_project(&new_proj_id)?;
            new_reports.insert(new_proj_id, proj_report);
        }

        self.reports = new_reports;
        Ok(())
    }

    /// Remap a single project within this report from `old_project` to
    /// `new_project`.
    ///
    /// Finds the contained `ProjectStorageReport` keyed by `old_project`,
    /// delegates to [`ProjectStorageReport::remap_project`] with `new_project`,
    /// and re-inserts it under the new key.  Does nothing if no report exists
    /// for `old_project`.
    pub fn remap_project(
        &mut self,
        old_project: &ProjectIdentifier,
        new_project: &ProjectIdentifier,
    ) -> Result<(), Error> {
        let mut proj_report = match self.reports.remove(old_project) {
            Some(r) => r,
            None => return Ok(()),
        };
        proj_report.remap_project(new_project)?;
        self.reports.insert(new_project.clone(), proj_report);
        Ok(())
    }

    /// Remap local username strings across all contained project reports.
    ///
    /// Delegates to [`ProjectStorageReport::remap_users`] for each project.
    /// Returns an error if the remapping would cause a clash within any
    /// individual project report.
    pub fn remap_users(
        &mut self,
        new_usermapping: &HashMap<UserIdentifier, String>,
    ) -> Result<(), Error> {
        for report in self.reports.values_mut() {
            report.remap_users(new_usermapping)?;
        }
        Ok(())
    }

    /// Combine a slice of `StorageReport`s for the same portal into a single
    /// report, merging per-project history.
    pub fn combine(reports: &[StorageReport]) -> Result<Self, Error> {
        if reports.is_empty() {
            return Err(Error::InvalidState("No reports to combine".to_string()));
        }

        let mut combined = StorageReport::new(&reports[0].portal);

        for report in reports.iter() {
            if report.portal() != combined.portal() {
                return Err(Error::Incompatible(format!(
                    "Cannot combine reports from incompatible portals: {} and {}",
                    report.portal(),
                    combined.portal()
                )));
            }
            combined += report.clone();
        }

        Ok(combined)
    }

    /// Serialise to a JSON string.
    pub fn to_json(&self) -> Result<String, Error> {
        serde_json::to_string(self)
            .context("Failed to serialise StorageReport to JSON")
            .map_err(Error::from)
    }

    /// Deserialise from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, Error> {
        serde_json::from_str(json)
            .context("Failed to deserialise StorageReport from JSON")
            .map_err(Error::from)
    }
}

impl std::ops::Add<StorageReport> for StorageReport {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        if self.portal != other.portal {
            tracing::warn!(
                "Cannot merge storage reports for different portals: {} and {}",
                self.portal,
                other.portal
            );
            return self;
        }

        let mut new_report = self.clone();
        new_report += other;
        new_report
    }
}

impl std::ops::AddAssign<StorageReport> for StorageReport {
    fn add_assign(&mut self, other: Self) {
        if self.portal != other.portal {
            tracing::warn!(
                "Cannot merge storage reports for different portals: {} and {}",
                self.portal,
                other.portal
            );
            return;
        }

        for (project, report) in other.reports {
            match self.reports.get_mut(&project) {
                Some(existing) => *existing += report,
                None => {
                    self.reports.insert(project, report);
                }
            }
        }
    }
}

impl std::fmt::Display for StorageReport {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        writeln!(f, "Storage report for portal {}", self.portal)?;

        if self.reports.is_empty() {
            return writeln!(f, "  (no project reports)");
        }

        let mut projects = self.reports.keys().collect::<Vec<_>>();
        projects.sort_by_cached_key(|p| p.to_string());

        for project in projects {
            if let Some(report) = self.reports.get(project) {
                writeln!(f, "{}", report)?;
                writeln!(f, "----------------------------------------")?;
            }
        }

        Ok(())
    }
}
