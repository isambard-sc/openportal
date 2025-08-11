// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::Error;

use crate::grammar::{
    Allocation, Date, NamedType, Node, PortalIdentifier, ProjectIdentifier, UserIdentifier,
    UserMapping,
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

#[derive(Copy, Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    seconds: u64,
}

impl std::iter::Sum for Usage {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::default(), |a, b| Self {
            seconds: a.seconds + b.seconds,
        })
    }
}

impl std::fmt::Display for Usage {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.seconds() >= 60 {
            true => match self.minutes() >= 60.0 {
                true => match self.hours() >= 24.0 {
                    true => match self.days() >= 7.0 {
                        true => match self.weeks() >= 4.5 {
                            true => match self.months() >= 12.0 {
                                true => write!(f, "{:.2} years", self.years()),
                                false => write!(f, "{:.2} months", self.months()),
                            },
                            false => write!(f, "{:.2} weeks", self.weeks()),
                        },
                        false => write!(f, "{:.2} days", self.days()),
                    },
                    false => write!(f, "{:.2} hours", self.hours()),
                },
                false => write!(f, "{} minutes", self.minutes()),
            },
            false => write!(f, "{} seconds", self.seconds()),
        }
    }
}

impl Usage {
    pub fn parse(duration: &str) -> Result<Self, Error> {
        let mut units = 1; // seconds

        let parts: Vec<&str> = duration.split_whitespace().collect();

        if parts.is_empty() {
            tracing::error!(
                "get_limit failed to parse '{}'. No duration found",
                duration
            );
            return Err(Error::Parse(format!(
                "get_limit failed to parse '{}'. No duration found",
                duration
            )));
        }

        if parts.len() > 1 {
            units = match parts[1].to_ascii_lowercase().as_str() {
                "seconds" | "second" | "s" => 1,
                "minutes" | "minute" | "m" => 60,
                "hours" | "hour" | "h" => 3600,
                "days" | "day" | "d" => 86400,
                _ => {
                    tracing::error!(
                                "get_limit failed to parse '{}'. Units should be seconds, minutes, hours or days",
                                &parts[1..].join(" "),
                            );
                    return Err(Error::Parse(format!(
                                "get_limit failed to parse '{}'. Units should be seconds, minutes, hours or days",
                                &parts[1..].join(" "),
                            )));
                }
            };
        }

        let seconds = parts[0]
            .parse::<u64>()
            .with_context(|| format!("Failed to parse seconds from '{}'", duration))?;

        Ok(Self {
            seconds: seconds * units,
        })
    }

    pub fn new(seconds: u64) -> Self {
        Self { seconds }
    }

    pub fn from_seconds(seconds: u64) -> Self {
        Self { seconds }
    }

    pub fn from_minutes(minutes: f64) -> Self {
        match minutes < 0.0 {
            true => Self::default(),
            false => Self {
                seconds: (minutes * 60.0) as u64,
            },
        }
    }

    pub fn from_hours(hours: f64) -> Self {
        match hours < 0.0 {
            true => Self::default(),
            false => Self {
                seconds: (hours * 3600.0) as u64,
            },
        }
    }

    pub fn from_days(days: f64) -> Self {
        match days < 0.0 {
            true => Self::default(),
            false => Self {
                seconds: (days * 86400.0) as u64,
            },
        }
    }

    pub fn from_weeks(weeks: f64) -> Self {
        match weeks < 0.0 {
            true => Self::default(),
            false => Self {
                seconds: (weeks * 604800.0) as u64,
            },
        }
    }

    pub fn from_months(months: f64) -> Self {
        match months < 0.0 {
            true => Self::default(),
            false => Self {
                seconds: (months * 2628000.0) as u64,
            },
        }
    }

    pub fn from_years(years: f64) -> Self {
        match years < 0.0 {
            true => Self::default(),
            false => Self {
                seconds: (years * 31536000.0) as u64,
            },
        }
    }

    pub fn seconds(&self) -> u64 {
        self.seconds
    }

    pub fn minutes(&self) -> f64 {
        self.seconds as f64 / 60.0
    }

    pub fn hours(&self) -> f64 {
        self.seconds as f64 / 3600.0
    }

    pub fn days(&self) -> f64 {
        self.seconds as f64 / 86400.0
    }

    pub fn weeks(&self) -> f64 {
        self.seconds as f64 / 604800.0
    }

    pub fn months(&self) -> f64 {
        self.seconds as f64 / 2628000.0
    }

    pub fn years(&self) -> f64 {
        self.seconds as f64 / 31536000.0
    }
}

// add the += operator for Usage
impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.seconds += other.seconds;
    }
}

// add the -= operator for Usage
impl std::ops::SubAssign for Usage {
    fn sub_assign(&mut self, other: Self) {
        self.seconds -= other.seconds;
    }
}

// add the *= operator for Usage
impl std::ops::MulAssign<f64> for Usage {
    fn mul_assign(&mut self, rhs: f64) {
        self.seconds = (self.seconds as f64 * rhs) as u64;
    }
}

// add the /= operator for Usage
impl std::ops::DivAssign<f64> for Usage {
    fn div_assign(&mut self, rhs: f64) {
        if rhs == 0.0 {
            self.seconds = 0;
            return;
        }

        self.seconds = (self.seconds as f64 / rhs) as u64;
    }
}

// add the + operator for Usage
impl std::ops::Add for Usage {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            seconds: self.seconds + other.seconds,
        }
    }
}

// add the - operator for Usage
impl std::ops::Sub for Usage {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        let mut seconds = self.seconds as i64 - other.seconds as i64;
        if seconds < 0 {
            seconds = 0;
        }

        Self {
            seconds: seconds as u64,
        }
    }
}

// add the * operator for Usage
impl std::ops::Mul<f64> for Usage {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self {
        Self {
            seconds: (self.seconds as f64 * rhs) as u64,
        }
    }
}

// add the / operator for Usage
impl std::ops::Div<f64> for Usage {
    type Output = Self;

    fn div(self, rhs: f64) -> Self {
        if rhs == 0.0 {
            return Self::default();
        }

        Self {
            seconds: (self.seconds as f64 / rhs) as u64,
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

    pub fn set_usage(&mut self, local_user: &str, usage: Usage) {
        self.reports.insert(local_user.to_string(), usage);
    }

    pub fn add_unattributed_usage(&mut self, usage: Usage) {
        self.add_usage("unknown", usage);
    }

    pub fn set_unattributed_usage(&mut self, usage: Usage) {
        self.set_usage("unknown", usage);
    }

    pub fn set_complete(&mut self) {
        self.is_complete = true;
    }

    pub fn is_complete(&self) -> bool {
        self.is_complete
    }
}

impl std::ops::Add<DailyProjectUsageReport> for DailyProjectUsageReport {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        let mut new_report = self.clone();

        for (user, usage) in other.reports {
            new_report.add_usage(&user, usage);
        }

        new_report.is_complete = false; // combine reports are never complete

        new_report
    }
}

impl std::ops::AddAssign<DailyProjectUsageReport> for DailyProjectUsageReport {
    fn add_assign(&mut self, other: Self) {
        for (user, usage) in other.reports {
            self.add_usage(&user, usage);
        }

        self.is_complete = false; // combine reports are never complete
    }
}

impl std::ops::Mul<f64> for DailyProjectUsageReport {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self {
        let mut new_report = self.clone();
        for usage in new_report.reports.values_mut() {
            *usage *= rhs;
        }
        new_report
    }
}

impl std::ops::Div<f64> for DailyProjectUsageReport {
    type Output = Self;

    fn div(self, rhs: f64) -> Self {
        let mut new_report = self.clone();
        for usage in new_report.reports.values_mut() {
            *usage /= rhs;
        }
        new_report
    }
}

impl std::ops::MulAssign<f64> for DailyProjectUsageReport {
    fn mul_assign(&mut self, rhs: f64) {
        for usage in self.reports.values_mut() {
            *usage *= rhs;
        }
    }
}

impl std::ops::DivAssign<f64> for DailyProjectUsageReport {
    fn div_assign(&mut self, rhs: f64) {
        for usage in self.reports.values_mut() {
            *usage /= rhs;
        }
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

impl std::ops::Add<ProjectUsageReport> for ProjectUsageReport {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        if self.project != other.project {
            tracing::warn!(
                "Cannot add reports for different projects: {} and {}",
                self.project,
                other.project
            );
            return self;
        }

        let mut new_report = self.clone();

        for (date, report) in other.reports {
            match new_report.reports.get_mut(&date) {
                Some(existing_report) => {
                    for (user, usage) in report.reports {
                        existing_report.add_usage(&user, usage);
                    }
                }
                None => {
                    new_report.reports.insert(date, report);
                }
            }
        }

        new_report
    }
}

impl std::ops::AddAssign<ProjectUsageReport> for ProjectUsageReport {
    fn add_assign(&mut self, other: Self) {
        if self.project != other.project {
            tracing::warn!(
                "Cannot add reports for different projects: {} and {}",
                self.project,
                other.project
            );
            return;
        }

        for (date, report) in other.reports {
            match self.reports.get_mut(&date) {
                Some(existing_report) => {
                    for (user, usage) in report.reports {
                        existing_report.add_usage(&user, usage);
                    }
                }
                None => {
                    self.reports.insert(date, report);
                }
            }
        }
    }
}

impl std::ops::Mul<f64> for ProjectUsageReport {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self {
        let mut new_report = self.clone();
        for report in new_report.reports.values_mut() {
            for usage in report.reports.values_mut() {
                *usage *= rhs;
            }
        }
        new_report
    }
}

impl std::ops::Div<f64> for ProjectUsageReport {
    type Output = Self;

    fn div(self, rhs: f64) -> Self {
        let mut new_report = self.clone();
        for report in new_report.reports.values_mut() {
            for usage in report.reports.values_mut() {
                *usage /= rhs;
            }
        }
        new_report
    }
}

impl std::ops::MulAssign<f64> for ProjectUsageReport {
    fn mul_assign(&mut self, rhs: f64) {
        for report in self.reports.values_mut() {
            for usage in report.reports.values_mut() {
                *usage *= rhs;
            }
        }
    }
}

impl std::ops::DivAssign<f64> for ProjectUsageReport {
    fn div_assign(&mut self, rhs: f64) {
        for report in self.reports.values_mut() {
            for usage in report.reports.values_mut() {
                *usage /= rhs;
            }
        }
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
            Some(local_user) => self.reports.values().map(|r| r.usage(local_user)).sum(),
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

    pub fn add_report(&mut self, date: &Date, report: &DailyProjectUsageReport) {
        match self.reports.get_mut(date) {
            Some(existing_report) => {
                *existing_report += report.clone();
            }
            None => {
                self.reports.insert(date.clone(), report.clone());
            }
        }
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

    pub fn combine(reports: &[ProjectUsageReport]) -> Result<Self, Error> {
        if reports.is_empty() {
            return Err(Error::InvalidState("No reports to combine".to_string()));
        }

        let mut combined = ProjectUsageReport::new(&reports[0].project);

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

    pub fn set_day_complete(&mut self, date: &Date) {
        if let Some(report) = self.reports.get_mut(date) {
            report.set_complete();
        }
    }

    pub fn set_complete(&mut self) {
        for report in self.reports.values_mut() {
            report.set_complete();
        }
    }

    pub fn to_usage_report(&self) -> UsageReport {
        let mut r = UsageReport::new(&self.project.portal_identifier());
        r.reports.insert(self.project.clone(), self.clone());
        r
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

impl std::ops::Add<UsageReport> for UsageReport {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        if self.portal != other.portal {
            tracing::warn!(
                "Cannot add reports for different portals: {} and {}",
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

impl std::ops::AddAssign<UsageReport> for UsageReport {
    fn add_assign(&mut self, other: Self) {
        if self.portal != other.portal {
            tracing::warn!(
                "Cannot add reports for different portals: {} and {}",
                self.portal,
                other.portal
            );
            return;
        }

        for report in other.reports {
            match self.reports.get_mut(&report.0) {
                Some(existing_report) => {
                    *existing_report += report.1;
                }
                None => {
                    self.reports.insert(report.0, report.1);
                }
            }
        }
    }
}

impl std::ops::Mul<f64> for UsageReport {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self {
        let mut new_report = self.clone();
        for report in new_report.reports.values_mut() {
            *report = report.clone() * rhs;
        }
        new_report
    }
}

impl std::ops::Div<f64> for UsageReport {
    type Output = Self;

    fn div(self, rhs: f64) -> Self {
        let mut new_report = self.clone();
        for report in new_report.reports.values_mut() {
            *report = report.clone() / rhs;
        }
        new_report
    }
}

impl std::ops::MulAssign<f64> for UsageReport {
    fn mul_assign(&mut self, rhs: f64) {
        for report in self.reports.values_mut() {
            *report *= rhs;
        }
    }
}

impl std::ops::DivAssign<f64> for UsageReport {
    fn div_assign(&mut self, rhs: f64) {
        for report in self.reports.values_mut() {
            *report /= rhs;
        }
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

    pub fn combine(reports: &[UsageReport]) -> Result<Self, Error> {
        if reports.is_empty() {
            return Err(Error::InvalidState("No reports to combine".to_string()));
        }

        let mut combined = UsageReport::new(&reports[0].portal);

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
}

impl Allocation {
    pub fn to_node_hours(&self, node: &Node) -> Result<Usage, Error> {
        if let Some(size) = self.size() {
            if self.is_node_hours() {
                return Ok(Usage::from_hours(size));
            } else if self.is_cpu_hours() {
                if node.cores() == 0 {
                    return Err(Error::InvalidState(
                        "Node has no cores, cannot convert CPU hours to node hours".to_string(),
                    ));
                }

                return Ok(Usage::from_hours(size / node.cores() as f64));
            } else if self.is_gpu_hours() {
                if node.gpus() == 0 {
                    return Err(Error::InvalidState(
                        "Node has no GPUs, cannot convert GPU hours to node hours".to_string(),
                    ));
                }

                return Ok(Usage::from_hours(size / node.gpus() as f64));
            } else if self.is_core_hours() {
                if node.cores() == 0 {
                    return Err(Error::InvalidState(
                        "Node has no cores, cannot convert core hours to node hours".to_string(),
                    ));
                }

                return Ok(Usage::from_hours(size / node.cores() as f64));
            } else if self.is_gb_hours() {
                if node.memory_gb() == 0.0 {
                    return Err(Error::InvalidState(
                        "Node has no memory, cannot convert GB hours to node hours".to_string(),
                    ));
                }

                return Ok(Usage::from_hours(size / (node.memory_gb())));
            }
        }

        Err(Error::InvalidState(format!(
            "Cannot convert allocation '{}' to node hours.",
            self
        )))
    }

    pub fn to_cpu_hours(&self, node: &Node) -> Result<Usage, Error> {
        Ok(self.to_node_hours(node)? * node.cpus() as f64)
    }

    pub fn to_gpu_hours(&self, node: &Node) -> Result<Usage, Error> {
        Ok(self.to_node_hours(node)? * node.gpus() as f64)
    }

    pub fn to_core_hours(&self, node: &Node) -> Result<Usage, Error> {
        Ok(self.to_node_hours(node)? * node.cores() as f64)
    }

    pub fn to_gb_hours(&self, node: &Node) -> Result<Usage, Error> {
        Ok(self.to_node_hours(node)? * node.memory_gb())
    }

    pub fn from_node_hours(usage: &Usage) -> Result<Self, Error> {
        Allocation::from_size_and_units(usage.hours(), "NHR")
    }

    pub fn from_cpu_hours(usage: &Usage, node: &Node) -> Result<Self, Error> {
        Allocation::from_size_and_units(usage.hours() / node.cpus() as f64, "NHR")
    }

    pub fn from_gpu_hours(usage: &Usage, node: &Node) -> Result<Self, Error> {
        Allocation::from_size_and_units(usage.hours() / node.gpus() as f64, "NHR")
    }

    pub fn from_core_hours(usage: &Usage, node: &Node) -> Result<Self, Error> {
        Allocation::from_size_and_units(usage.hours() / node.cores() as f64, "NHR")
    }

    pub fn from_gb_hours(usage: &Usage, node: &Node) -> Result<Self, Error> {
        Allocation::from_size_and_units(usage.hours() / node.memory_gb(), "NHR")
    }
}
