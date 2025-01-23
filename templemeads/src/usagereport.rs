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
        let node_hours = self.node_seconds as f64 / 3600.0;

        match node_hours < 0.1 {
            true => write!(f, "{} node-seconds", self.node_seconds),
            false => match node_hours < 100.0 {
                true => write!(f, "{:.2} node-hours", node_hours),
                false => write!(f, "{:.1} node-hours", node_hours),
            },
        }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectUsageReport {
    project: ProjectIdentifier,
    reports: HashMap<Date, DailyProjectUsageReport>,
    users: HashMap<UserIdentifier, String>,
}

impl std::fmt::Display for ProjectUsageReport {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        writeln!(f, "{}", self.project())?;

        let mut dates = self.reports.keys().collect::<Vec<_>>();

        dates.sort();

        let mut users = HashMap::new();

        for (user, local_user) in &self.users {
            users.insert(local_user, user);
        }

        for date in dates {
            writeln!(f, "{}", date)?;

            let report = self.reports.get(date).cloned().unwrap_or_default();

            for user in report.local_users() {
                if let Some(userid) = users.get(&user) {
                    writeln!(f, "  {}: {}", userid, report.usage(&user))?;
                } else {
                    writeln!(f, "  {} - unknown: {}", user, report.usage(&user))?;
                }
            }

            writeln!(f, "Daily total: {}", report.total_usage())?;
            writeln!(f, "----------------------------------------")?;
        }

        writeln!(f, "========================================")?;
        writeln!(f, "Total: {}", self.total_usage())
    }
}

impl ProjectUsageReport {
    pub fn new(project: &ProjectIdentifier) -> Self {
        Self {
            project: project.clone(),
            reports: HashMap::new(),
            users: HashMap::new(),
        }
    }

    pub fn dates(&self) -> Vec<Date> {
        let mut dates: Vec<Date> = self.reports.keys().cloned().collect();

        dates.sort();

        dates
    }

    pub fn project(&self) -> ProjectIdentifier {
        self.project.clone()
    }

    pub fn portal(&self) -> PortalIdentifier {
        self.project().portal_identifier()
    }

    pub fn users(&self) -> Vec<UserIdentifier> {
        let mut users: Vec<UserIdentifier> = self.users.keys().cloned().collect();

        users.sort_by_cached_key(|u| u.to_string());

        users
    }

    pub fn unmapped_users(&self) -> Vec<String> {
        let mapped_users: std::collections::HashSet<String> =
            self.users.values().cloned().collect();

        let unmapped_users: std::collections::HashSet<String> = self
            .reports
            .values()
            .flat_map(|r| r.local_users())
            .filter(|u| !mapped_users.contains(u))
            .collect();

        let mut unmapped_users: Vec<String> = unmapped_users.into_iter().collect();

        unmapped_users.sort();

        unmapped_users
    }

    pub fn total_usage(&self) -> Usage {
        self.reports
            .values()
            .cloned()
            .map(|r| r.total_usage())
            .sum()
    }

    pub fn unmapped_usage(&self) -> Usage {
        let unmapped_users = self.unmapped_users();

        if unmapped_users.is_empty() {
            return Usage::default();
        }

        self.reports
            .values()
            .cloned()
            .map(|r| {
                r.local_users()
                    .into_iter()
                    .filter(|u| unmapped_users.contains(u))
                    .map(|u| r.usage(&u))
                    .sum()
            })
            .sum()
    }

    pub fn usage(&self, user: &UserIdentifier) -> Usage {
        // get the local username
        match self.users.get(user) {
            Some(local_user) => {
                return self.reports.values().map(|r| r.usage(local_user)).sum();
            }
            None => Usage::default(),
        }
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

    pub fn add_mapping(&mut self, mapping: &UserMapping) -> Result<(), Error> {
        if mapping.user().project_identifier() != self.project() {
            return Err(Error::InvalidState(format!(
                "Mapping for wrong project: {}. This report is for {}",
                mapping,
                self.project()
            )));
        }

        self.users
            .insert(mapping.user().clone(), mapping.local_user().to_string());

        Ok(())
    }

    pub fn set_report(&mut self, date: &Date, report: &DailyProjectUsageReport) {
        self.reports.insert(date.clone(), report.clone());
    }

    pub fn get_report(&self, date: &Date) -> ProjectUsageReport {
        match self.reports.get(date) {
            Some(report) => {
                let mut reports = HashMap::new();
                reports.insert(date.clone(), report.clone());

                ProjectUsageReport {
                    project: self.project.clone(),
                    reports,
                    users: self.users.clone(),
                }
            }
            None => ProjectUsageReport {
                project: self.project.clone(),
                reports: HashMap::new(),
                users: self.users.clone(),
            },
        }
    }

    pub fn is_complete(&self) -> bool {
        self.reports.values().all(|r| r.is_complete())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageReport {
    portal: PortalIdentifier,
    reports: HashMap<ProjectIdentifier, ProjectUsageReport>,
}

impl std::fmt::Display for UsageReport {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        writeln!(f, "{}", self.portal())?;

        let mut projects = self.reports.keys().collect::<Vec<_>>();

        projects.sort_by_cached_key(|p| p.to_string());

        for project in projects {
            writeln!(f, "{}", self.get_report(project))?;
            writeln!(f, "----------------------------------------")?;
        }

        writeln!(f, "Total: {}", self.total_usage())
    }
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
        let mut projects: Vec<ProjectIdentifier> = self.reports.keys().cloned().collect();

        projects.sort_by_cached_key(|p| p.to_string());

        projects
    }

    pub fn get_report(&self, project: &ProjectIdentifier) -> ProjectUsageReport {
        self.reports
            .get(project)
            .cloned()
            .unwrap_or(ProjectUsageReport {
                project: project.clone(),
                reports: HashMap::new(),
                users: HashMap::new(),
            })
    }

    pub fn set_report(&mut self, report: ProjectUsageReport) -> Result<(), Error> {
        match report.portal() == *self.portal() {
            true => {
                self.reports.insert(report.project(), report);
                Ok(())
            }
            false => Err(Error::InvalidState(format!(
                "Report for wrong portal: {}. This report is for {}",
                report.portal(),
                self.portal
            ))),
        }
    }

    pub fn total_usage(&self) -> Usage {
        self.reports
            .values()
            .cloned()
            .map(|r| r.total_usage())
            .sum()
    }
}
