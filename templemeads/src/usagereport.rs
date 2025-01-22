// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::Error;

use crate::grammar::{
    Date, NamedType, PortalIdentifier, ProjectIdentifier, UserIdentifier, UserMapping,
};

impl NamedType for Usage {
    fn type_name() -> &'static str {
        "Usage"
    }
}

impl NamedType for Vec<Usage> {
    fn type_name() -> &'static str {
        "Vec<Usage>"
    }
}

impl NamedType for UserUsageReport {
    fn type_name() -> &'static str {
        "UserUsageReport"
    }
}

impl NamedType for Vec<UserUsageReport> {
    fn type_name() -> &'static str {
        "Vec<UserUsageReport>"
    }
}

impl NamedType for DailyProjectUsageReport {
    fn type_name() -> &'static str {
        "DailyProjectUsageReport"
    }
}

impl NamedType for Vec<DailyProjectUsageReport> {
    fn type_name() -> &'static str {
        "Vec<DailyProjectUsageReport>"
    }
}

impl NamedType for ProjectUsageReport {
    fn type_name() -> &'static str {
        "ProjectUsageReport"
    }
}

impl NamedType for Vec<ProjectUsageReport> {
    fn type_name() -> &'static str {
        "Vec<ProjectUsageReport>"
    }
}

impl NamedType for UsageReport {
    fn type_name() -> &'static str {
        "UsageReport"
    }
}

impl NamedType for Vec<UsageReport> {
    fn type_name() -> &'static str {
        "Vec<UsageReport>"
    }
}

#[derive(Copy, Debug, Default, Clone, Serialize, Deserialize)]
pub struct Usage {
    node_seconds: u64,
}

impl std::iter::Sum for Usage {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::default(), |a, b| Self {
            node_seconds: a.node_seconds + b.node_seconds,
        })
    }
}

impl std::fmt::Display for Usage {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} node-hours", self.node_seconds as f64 / 3600.0)
    }
}

impl Usage {
    pub fn new(node_seconds: u64) -> Self {
        Self { node_seconds }
    }

    pub fn node_seconds(&self) -> u64 {
        self.node_seconds
    }
}

// add the += operator for Usage
impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.node_seconds += other.node_seconds;
    }
}

// add the + operator for Usage
impl std::ops::Add for Usage {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            node_seconds: self.node_seconds + other.node_seconds,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserUsageReport {
    user: UserIdentifier,
    usage: Usage,
}

impl std::fmt::Display for UserUsageReport {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}: {}", self.user, self.usage)
    }
}

impl UserUsageReport {
    pub fn new(user: &UserIdentifier, usage: Usage) -> Self {
        Self {
            user: user.clone(),
            usage,
        }
    }

    pub fn user(&self) -> &UserIdentifier {
        &self.user
    }

    pub fn usage(&self) -> &Usage {
        &self.usage
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DailyProjectUsageReport {
    reports: HashMap<String, Usage>,
    is_complete: bool,
}

impl std::ops::AddAssign for DailyProjectUsageReport {
    fn add_assign(&mut self, other: Self) {
        for (user, usage) in other.reports {
            *self.reports.entry(user).or_default() += usage;
        }
    }
}

impl std::fmt::Display for DailyProjectUsageReport {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let mut users = self.reports.keys().collect::<Vec<_>>();

        users.sort();

        for user in users {
            writeln!(f, "{}: {}", user, self.reports[user])?;
        }

        match self.is_complete() {
            true => writeln!(f, "Total: {}", self.total_usage()),
            false => writeln!(f, "Total: {} - incomplete", self.total_usage()),
        }
    }
}

impl DailyProjectUsageReport {
    pub fn usage(&self, local_user: &str) -> Usage {
        self.reports.get(local_user).cloned().unwrap_or_default()
    }

    pub fn local_users(&self) -> Vec<String> {
        self.reports.keys().cloned().collect()
    }

    pub fn total_usage(&self) -> Usage {
        self.reports.values().cloned().sum()
    }

    pub fn set_usage(&mut self, local_user: &str, usage: Usage) {
        self.reports.insert(local_user.to_string(), usage);
    }

    pub fn add_usage(&mut self, local_user: &str, usage: Usage) {
        *self.reports.entry(local_user.to_string()).or_default() += usage;
    }

    pub fn set_complete(&mut self) {
        self.is_complete = true;
    }

    pub fn is_complete(&self) -> bool {
        self.is_complete
    }
}

#[derive(Debug, Clone)]
pub struct ProjectUsageReport {
    project: ProjectIdentifier,
    reports: HashMap<Date, DailyProjectUsageReport>,
    users: HashMap<String, UserIdentifier>,

    inv_users: HashMap<UserIdentifier, String>,
}

#[derive(Serialize, Deserialize)]
struct ProjectUsageReportHelper {
    project: ProjectIdentifier,
    reports: HashMap<Date, DailyProjectUsageReport>,
    users: HashMap<String, UserIdentifier>,
}

impl serde::Serialize for ProjectUsageReport {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ProjectUsageReportHelper {
            project: self.project.clone(),
            reports: self.reports.clone(),
            users: self.users.clone(),
        }
        .serialize(serializer)
    }
}

// add the serde deserialization function, which will also rebuild the
// inv_users map
impl<'de> serde::Deserialize<'de> for ProjectUsageReport {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let helper = ProjectUsageReportHelper::deserialize(deserializer)?;

        let inv_users = helper
            .users
            .iter()
            .map(|(k, v)| (v.clone(), k.clone()))
            .collect();

        Ok(ProjectUsageReport {
            project: helper.project,
            reports: helper.reports,
            users: helper.users,
            inv_users,
        })
    }
}

// add the += operator for ProjectUsageReport
impl std::ops::AddAssign for ProjectUsageReport {
    fn add_assign(&mut self, other: Self) {
        for (date, report) in other.reports {
            if let Some(existing) = self.reports.get_mut(&date) {
                *existing += report;
            } else {
                self.reports.insert(date, report);
            }
        }

        for (local_user, user) in other.users {
            self.users.insert(local_user.clone(), user.clone());
            self.inv_users.insert(user, local_user);
        }
    }
}

// add the + operator for ProjectUsageReport
impl std::ops::Add for ProjectUsageReport {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        let mut new_report = self.clone();
        new_report += other;
        new_report
    }
}

impl std::fmt::Display for ProjectUsageReport {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        writeln!(f, "{}", self.project())?;

        let mut dates = self.reports.keys().collect::<Vec<_>>();

        dates.sort();

        for date in dates {
            writeln!(f, "{}", date)?;

            let report = self.reports.get(date).cloned().unwrap_or_default();

            for user in report.local_users() {
                if let Some(userid) = self.users.get(&user) {
                    writeln!(f, "{}: {}", userid, report.usage(&user))?;
                } else {
                    writeln!(f, "{} - unknown: {}", user, report.usage(&user))?;
                }
            }

            writeln!(f, "Daily total: {}", report.total_usage())?;
        }

        writeln!(f, "Total: {}", self.total_usage())
    }
}

pub enum UserType {
    UserMapping(UserMapping),
    UserIdentifier(UserIdentifier),
    LocalUser(String),
}

impl From<UserMapping> for UserType {
    fn from(mapping: UserMapping) -> Self {
        UserType::UserMapping(mapping)
    }
}

impl From<UserIdentifier> for UserType {
    fn from(user: UserIdentifier) -> Self {
        UserType::UserIdentifier(user)
    }
}

impl From<String> for UserType {
    fn from(user: String) -> Self {
        UserType::LocalUser(user)
    }
}

impl ProjectUsageReport {
    pub fn new(project: &ProjectIdentifier) -> Self {
        Self {
            project: project.clone(),
            reports: HashMap::new(),
            users: HashMap::new(),
            inv_users: HashMap::new(),
        }
    }

    pub fn usage(&self, date: &Date) -> ProjectUsageReport {
        let mut reports = HashMap::new();
        reports.insert(
            date.clone(),
            self.reports.get(date).cloned().unwrap_or_default(),
        );

        ProjectUsageReport {
            project: self.project.clone(),
            reports,
            users: self.users.clone(),
            inv_users: self.inv_users.clone(),
        }
    }

    pub fn dates(&self) -> Vec<Date> {
        self.reports.keys().cloned().collect()
    }

    pub fn project(&self) -> ProjectIdentifier {
        self.project.clone()
    }

    pub fn portal(&self) -> PortalIdentifier {
        self.project().portal_identifier()
    }

    pub fn users(&self) -> Vec<UserIdentifier> {
        self.users.values().cloned().collect()
    }

    pub fn unmapped_users(&self) -> Vec<String> {
        let unmapped_users: std::collections::HashSet<String> = self
            .reports
            .values()
            .flat_map(|r| r.local_users())
            .filter(|u| !self.users.contains_key(u))
            .collect();

        unmapped_users.into_iter().collect()
    }

    pub fn total_usage(&self) -> Usage {
        self.reports
            .values()
            .cloned()
            .map(|r| r.total_usage())
            .sum()
    }

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

    pub fn add_mapping(&mut self, mapping: &UserMapping) -> Result<String, Error> {
        if mapping.user().project_identifier() != self.project() {
            return Err(Error::InvalidState(format!(
                "Mapping for wrong project: {}. This report is for {}",
                mapping,
                self.project()
            )));
        }

        // check that the mapping hasn't changed
        if let Some(user) = self.users.get(mapping.local_user()) {
            if user != mapping.user() {
                tracing::warn!(
                    "Mapping for {} already exists: {}. Ignoring {}",
                    mapping.local_user(),
                    user,
                    mapping.user(),
                );

                return Ok(mapping.local_user().to_string());
            }
        }

        if let Some(local_user) = self.inv_users.get(mapping.user()) {
            if local_user != mapping.local_user() {
                tracing::warn!(
                    "Mapping for {} already exists: {}. Ignoring {}",
                    mapping.user(),
                    local_user,
                    mapping.local_user(),
                );

                return Ok(local_user.to_string());
            }
        }

        self.users
            .insert(mapping.local_user().to_string(), mapping.user().clone());

        self.inv_users
            .insert(mapping.user().clone(), mapping.local_user().to_string());

        Ok(mapping.local_user().to_string())
    }

    pub fn set_usage(&mut self, user: &UserType, date: &Date, usage: Usage) -> Result<(), Error> {
        let local_user = match user {
            UserType::UserMapping(mapping) => self.add_mapping(mapping)?,
            UserType::UserIdentifier(user) => match self.inv_users.get(user) {
                Some(local_user) => local_user.clone(),
                None => {
                    tracing::warn!("Unknown user {}. Cannot record usage!", user);
                    return Err(Error::UnmanagedUser(format!(
                        "Unknown user {} - no mapping known",
                        user
                    )));
                }
            },
            UserType::LocalUser(local_user) => local_user.clone(),
        };

        if let Some(report) = self.reports.get_mut(date) {
            report.set_usage(&local_user, usage);
        } else {
            let mut report = DailyProjectUsageReport::default();
            report.set_usage(&local_user, usage);
            self.reports.insert(date.clone(), report);
        }

        Ok(())
    }

    pub fn add_usage(&mut self, user: &UserType, date: &Date, usage: Usage) -> Result<(), Error> {
        let local_user = match user {
            UserType::UserMapping(mapping) => self.add_mapping(mapping)?,
            UserType::UserIdentifier(user) => match self.inv_users.get(user) {
                Some(local_user) => local_user.clone(),
                None => {
                    tracing::warn!("Unknown user {}. Cannot record usage!", user);
                    return Err(Error::UnmanagedUser(format!(
                        "Unknown user {} - no mapping known",
                        user
                    )));
                }
            },
            UserType::LocalUser(local_user) => local_user.clone(),
        };

        if let Some(report) = self.reports.get_mut(date) {
            report.add_usage(&local_user, usage);
        } else {
            let mut report = DailyProjectUsageReport::default();
            report.add_usage(&local_user, usage);
            self.reports.insert(date.clone(), report);
        }

        Ok(())
    }

    pub fn set_completed(&mut self, date: &Date) {
        if let Some(report) = self.reports.get_mut(date) {
            report.set_complete();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageReport {
    portal: PortalIdentifier,
    reports: HashMap<ProjectIdentifier, ProjectUsageReport>,
}

impl UsageReport {
    pub fn new(portal: &PortalIdentifier) -> Self {
        Self {
            portal: portal.clone(),
            reports: HashMap::new(),
        }
    }

    pub fn portal(&self) -> &PortalIdentifier {
        &self.portal
    }

    pub fn projects(&self) -> Vec<ProjectIdentifier> {
        self.reports.keys().cloned().collect()
    }

    pub fn get_report(&self, project: &ProjectIdentifier) -> ProjectUsageReport {
        self.reports
            .get(project)
            .cloned()
            .unwrap_or(ProjectUsageReport {
                project: project.clone(),
                reports: HashMap::new(),
                users: HashMap::new(),
                inv_users: HashMap::new(),
            })
    }

    pub fn add(&mut self, report: &ProjectUsageReport) -> Result<(), Error> {
        if report.portal() != *self.portal() {
            return Err(Error::InvalidState(format!(
                "Report for wrong portal: {}. This report is for {}",
                report.portal(),
                self.portal()
            )));
        }

        if let Some(existing) = self.reports.get_mut(&report.project()) {
            *existing += report.clone();
        } else {
            self.reports
                .insert(report.project().clone(), report.clone());
        }

        Ok(())
    }

    pub fn total_usage(&self) -> Usage {
        self.reports
            .values()
            .cloned()
            .map(|r| r.total_usage())
            .sum()
    }
}
