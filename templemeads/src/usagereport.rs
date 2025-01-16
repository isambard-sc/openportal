// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::grammar::{
    Date, DateRange, NamedType, PortalIdentifier, ProjectIdentifier, UserIdentifier,
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

impl NamedType for DailyUsageReport {
    fn type_name() -> &'static str {
        "DailyUsageReport"
    }
}

impl NamedType for Vec<DailyUsageReport> {
    fn type_name() -> &'static str {
        "Vec<DailyUsageReport>"
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

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UserUsageReport {
    usage: Usage,
}

impl std::fmt::Display for UserUsageReport {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.usage)
    }
}

impl UserUsageReport {
    pub fn new(usage: Usage) -> Self {
        Self { usage }
    }

    pub fn usage(&self) -> Usage {
        self.usage
    }

    pub fn total_usage(&self) -> Usage {
        self.usage
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProjectUsageReport {
    users: HashMap<UserIdentifier, UserUsageReport>,
}

impl std::fmt::Display for ProjectUsageReport {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let mut users = self.users.keys().collect::<Vec<_>>();

        users.sort_by_cached_key(|a| a.to_string());

        for user in users {
            writeln!(f, "{}: {}", user, self.users[user])?;
        }

        writeln!(f, "Total: {}", self.total_usage())
    }
}

impl ProjectUsageReport {
    pub fn usage(&self, user: &UserIdentifier) -> UserUsageReport {
        self.users.get(user).cloned().unwrap_or_default()
    }

    pub fn users(&self) -> Vec<UserIdentifier> {
        self.users.keys().cloned().collect()
    }

    pub fn total_usage(&self) -> Usage {
        let mut total = Usage::default();
        for user in self.users.values() {
            total += user.usage();
        }
        total
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DailyUsageReport {
    projects: HashMap<ProjectIdentifier, ProjectUsageReport>,
}

impl std::fmt::Display for DailyUsageReport {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let mut projects = self.projects.keys().collect::<Vec<_>>();
        projects.sort_by_cached_key(|a| a.to_string());

        for project in projects {
            writeln!(f, "{}", project)?;
            writeln!(f, "{}", self.projects[project])?;
        }

        for (project, usage) in &self.projects {
            writeln!(f, "{}: {}", project, usage)?;
        }

        writeln!(f, "Daily total: {}", self.total_usage())
    }
}

impl DailyUsageReport {
    pub fn usage(&self, project: &ProjectIdentifier) -> ProjectUsageReport {
        self.projects.get(project).cloned().unwrap_or_default()
    }

    pub fn projects(&self) -> Vec<ProjectIdentifier> {
        self.projects.keys().cloned().collect()
    }

    pub fn total_usage(&self) -> Usage {
        let mut total = Usage::default();
        for project in self.projects.values() {
            total += project.total_usage();
        }
        total
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageReport {
    portal: PortalIdentifier,
    date_range: DateRange,
    daily_reports: HashMap<Date, DailyUsageReport>,
}

impl UsageReport {
    pub fn new(portal: &PortalIdentifier, date_range: &DateRange) -> Self {
        Self {
            portal: portal.clone(),
            date_range: date_range.clone(),
            daily_reports: HashMap::new(),
        }
    }

    pub fn portal(&self) -> &PortalIdentifier {
        &self.portal
    }

    pub fn date_range(&self) -> &DateRange {
        &self.date_range
    }

    pub fn usage(&self, date: &Date) -> DailyUsageReport {
        self.daily_reports.get(date).cloned().unwrap_or_default()
    }

    pub fn total_usage(&self) -> Usage {
        let mut total = Usage::default();
        for daily_report in self.daily_reports.values() {
            total += daily_report.total_usage();
        }
        total
    }
}
