// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;

use templemeads::Error;

use crate::grammar::{
    Allocation, Date, DateRange, Node, ProjectIdentifier, UserIdentifier, UserMapping,
};
use templemeads::named::NamedType;
use templemeads::portal_identifier::PortalIdentifier;

impl NamedType for Usage {
    fn type_name() -> String {
        "Usage".to_string()
    }
}

impl NamedType for UserUsageReport {
    fn type_name() -> String {
        "UserUsageReport".to_string()
    }
}

impl NamedType for DailyProjectUsageReport {
    fn type_name() -> String {
        "DailyProjectUsageReport".to_string()
    }
}

impl NamedType for ProjectUsageReport {
    fn type_name() -> String {
        "ProjectUsageReport".to_string()
    }
}

impl NamedType for UsageReport {
    fn type_name() -> String {
        "UsageReport".to_string()
    }
}

#[derive(Copy, Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Usage {
    seconds: u64,
}

impl std::iter::Sum for Usage {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::default(), |a, b| Self {
            seconds: a.seconds.saturating_add(b.seconds),
        })
    }
}

impl std::fmt::Display for Usage {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        // Returns "unit" or "units" depending on whether the value rounds to 1.000 at 3dp.
        fn unit(value: f64, singular: &'static str, plural: &'static str) -> &'static str {
            if (value - 1.0).abs() < 0.0005 {
                singular
            } else {
                plural
            }
        }
        match self.seconds() >= 60 {
            true => match self.minutes() >= 60.0 {
                true => match self.hours() >= 24.0 {
                    true => match self.days() >= 7.0 {
                        true => match self.weeks() >= 4.5 {
                            true => match self.months() >= 12.0 {
                                true => write!(
                                    f,
                                    "{:.3} {}",
                                    self.years(),
                                    unit(self.years(), "year", "years")
                                ),
                                false => write!(
                                    f,
                                    "{:.3} {}",
                                    self.months(),
                                    unit(self.months(), "month", "months")
                                ),
                            },
                            false => write!(
                                f,
                                "{:.3} {}",
                                self.weeks(),
                                unit(self.weeks(), "week", "weeks")
                            ),
                        },
                        false => {
                            write!(f, "{:.3} {}", self.days(), unit(self.days(), "day", "days"))
                        }
                    },
                    false => write!(
                        f,
                        "{:.3} {}",
                        self.hours(),
                        unit(self.hours(), "hour", "hours")
                    ),
                },
                false => write!(
                    f,
                    "{:.3} {}",
                    self.minutes(),
                    unit(self.minutes(), "minute", "minutes")
                ),
            },
            false => write!(
                f,
                "{} {}",
                self.seconds(),
                if self.seconds() == 1 {
                    "second"
                } else {
                    "seconds"
                }
            ),
        }
    }
}

/// Display adapter that always formats a [`Usage`] value in hours.
/// Obtained via [`Usage::in_hours`].
pub struct UsageHoursDisplay(Usage);

impl std::fmt::Display for UsageHoursDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:.3} hours", self.0.hours())
    }
}

impl Usage {
    pub fn parse(duration: &str) -> Result<Self, Error> {
        let mut units = 1; // seconds

        let parts: Vec<&str> = duration.split_whitespace().collect();

        // Split rather than indexed, so a missing count cannot panic - see
        // docs/specifications/security-review-2.md (finding R1).
        let Some((count_part, unit_parts)) = parts.split_first() else {
            tracing::error!(
                "get_limit failed to parse '{}'. No duration found",
                duration
            );
            return Err(Error::Parse(format!(
                "get_limit failed to parse '{}'. No duration found",
                duration
            )));
        };

        if let Some(unit_part) = unit_parts.first() {
            units = match unit_part.to_ascii_lowercase().as_str() {
                "seconds" | "second" | "s" => 1,
                "minutes" | "minute" | "m" => 60,
                "hours" | "hour" | "h" => 3600,
                "days" | "day" | "d" => 86400,
                _ => {
                    tracing::error!(
                                "get_limit failed to parse '{}'. Units should be seconds, minutes, hours or days",
                                unit_parts.join(" "),
                            );
                    return Err(Error::Parse(format!(
                                "get_limit failed to parse '{}'. Units should be seconds, minutes, hours or days",
                                unit_parts.join(" "),
                            )));
                }
            };
        }

        let seconds = count_part
            .parse::<u64>()
            .with_context(|| format!("Failed to parse seconds from '{}'", duration))?;

        Ok(Self {
            // Saturating: `seconds` is parsed from a peer-supplied string and
            // `units` can be 86400, so the product overflows well inside the
            // range `u64` accepts. See
            // docs/specifications/security-review-2.md (finding R33).
            seconds: seconds.saturating_mul(units),
        })
    }

    pub fn new(seconds: u64) -> Self {
        Self { seconds }
    }

    /// Returns a display adapter that formats this value in hours only,
    /// e.g. `format!("{}", usage.in_hours())` → `"1.500 hours"`.
    pub fn in_hours(&self) -> UsageHoursDisplay {
        UsageHoursDisplay(*self)
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

    pub fn is_zero(&self) -> bool {
        self.seconds == 0
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
        self.seconds = self.seconds.saturating_add(other.seconds);
    }
}

// add the -= operator for Usage
impl std::ops::SubAssign for Usage {
    fn sub_assign(&mut self, other: Self) {
        // Saturating, matching `Sub` below, which already clamped at zero. This
        // was a bare `-=`, so an underflow wrapped to near `u64::MAX` - silently
        // in release, where `overflow-checks` used to be off.
        self.seconds = self.seconds.saturating_sub(other.seconds);
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
            seconds: self.seconds.saturating_add(other.seconds),
        }
    }
}

// add the - operator for Usage
impl std::ops::Sub for Usage {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            // `saturating_sub` clamps at zero directly. The previous form went
            // via `i64`, which silently gave the wrong answer for any value
            // above `i64::MAX`.
            seconds: self.seconds.saturating_sub(other.seconds),
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

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UserUsageReport {
    #[ts(as = "String")]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DailyProjectUsageReport {
    reports: HashMap<String, Usage>,
    #[serde(default)]
    components: HashMap<String, HashMap<String, Usage>>,
    /// Per-user job counts. Empty when reading data from older instances.
    #[serde(default)]
    user_job_counts: HashMap<String, u64>,
    /// Per-user wait seconds. Empty when reading data from older instances.
    #[serde(default)]
    user_wait_seconds: HashMap<String, u64>,
    /// Scalar total — equals sum of user_job_counts when populated, otherwise
    /// carries the value from older instances that lack per-user maps.
    #[serde(default)]
    num_jobs: u64,
    /// Scalar total — equals sum of user_wait_seconds when populated.
    #[serde(default)]
    total_wait_seconds: u64,

    // ---- Requeue accounting -------------------------------------------------
    //
    // Slurm keeps one accounting record per *attempt* of a job, and a requeued
    // job has several. The fields above describe only the last attempt of each
    // job - the one `sacct` returns by default - so that what they report is
    // unchanged by the arrival of the earlier attempts. Everything the earlier
    // attempts consumed lands in the fields below instead.
    //
    // `total_usage() + total_requeue_usage()` is therefore a project's true
    // consumption, and `total_usage()` alone is what we have always reported.
    // Which of the two a project should be charged for is a policy question,
    // which is why both are carried. See
    // `docs/plans/slurm-requeue-accounting-design.md`.
    //
    // All are `serde(default)`, so a report from an instance that predates them
    // deserialises as "no requeues seen" rather than failing.
    /// Usage from attempts superseded by a requeue, per local user.
    #[serde(default)]
    requeue_reports: HashMap<String, Usage>,
    /// The same, broken down by resource component.
    #[serde(default)]
    requeue_components: HashMap<String, HashMap<String, Usage>>,
    /// Per-user count of requeue *events* (superseded attempts, not jobs).
    #[serde(default)]
    user_requeue_events: HashMap<String, u64>,
    /// Scalar total — equals sum of user_requeue_events when populated.
    #[serde(default)]
    num_requeue_events: u64,
    /// Per-user queue wait accumulated by superseded attempts.
    #[serde(default)]
    user_requeue_wait_seconds: HashMap<String, u64>,
    /// Scalar total — equals sum of user_requeue_wait_seconds when populated.
    #[serde(default)]
    requeue_wait_seconds: u64,
    /// Requeue events by the terminal state of the superseded attempt. Sums to
    /// `num_requeue_events`.
    #[serde(default)]
    requeue_states: HashMap<String, u64>,
    /// Requeue usage by the terminal state of the superseded attempt. Sums to
    /// `total_requeue_usage()`.
    #[serde(default)]
    requeue_state_usage: HashMap<String, Usage>,

    is_complete: bool,
}

impl std::fmt::Display for DailyProjectUsageReport {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let mut users = self.reports.keys().collect::<Vec<_>>();

        users.sort();

        for user in users {
            // `HashMap`'s `Index` panics on a missing key. `users` comes from
            // this same map's keys, so it cannot miss - looked up via `get`
            // anyway, since a panic in a `Display` impl would abort the
            // process. See docs/specifications/security-review-2.md (R1).
            let Some(report) = self.reports.get(user) else {
                continue;
            };

            let jobs = self.num_jobs_for_user(user);
            if jobs > 0 {
                writeln!(
                    f,
                    "{}: {} | {} {} | Average wait: {}",
                    user,
                    report,
                    jobs,
                    if jobs == 1 { "job" } else { "jobs" },
                    Usage::new(self.average_wait_seconds_for_user(user))
                )?;
            } else {
                writeln!(f, "{}: {}", user, report)?;
            }
        }

        match self.num_jobs() {
            0 => (),
            n => {
                if self.total_wait_seconds() > 0 {
                    writeln!(
                        f,
                        "Number of jobs: {} | Average wait: {}",
                        n,
                        Usage::new(self.total_wait_seconds() / n)
                    )?;
                } else {
                    writeln!(f, "Number of jobs: {}", n)?;
                }
            }
        }

        if self.num_requeue_events() > 0 || !self.total_requeue_usage().is_zero() {
            writeln!(
                f,
                "Requeued: {} {} | {} | Average requeue wait: {}",
                self.num_requeue_events(),
                if self.num_requeue_events() == 1 {
                    "event"
                } else {
                    "events"
                },
                self.total_requeue_usage(),
                Usage::new(self.average_requeue_wait_seconds())
            )?;
        }

        match self.is_complete() {
            true => writeln!(f, "Total: {}", self.total_usage()),
            false => writeln!(f, "Total: {} - incomplete", self.total_usage()),
        }
    }
}

/// Display adapter that formats all [`Usage`] values in a
/// [`DailyProjectUsageReport`] in hours. Obtained via
/// [`DailyProjectUsageReport::in_hours`].
pub struct DailyProjectUsageReportHoursDisplay<'a>(&'a DailyProjectUsageReport);

impl std::fmt::Display for DailyProjectUsageReportHoursDisplay<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let report = self.0;
        let mut users = report.reports.keys().collect::<Vec<_>>();

        users.sort();

        for user in users {
            let Some(user_report) = report.reports.get(user) else {
                continue;
            };

            let jobs = report.num_jobs_for_user(user);
            if jobs > 0 {
                writeln!(
                    f,
                    "{}: {} | {} {} | Average wait: {}",
                    user,
                    user_report.in_hours(),
                    jobs,
                    if jobs == 1 { "job" } else { "jobs" },
                    Usage::new(report.average_wait_seconds_for_user(user)).in_hours()
                )?;
            } else {
                writeln!(f, "{}: {}", user, user_report.in_hours())?;
            }
        }

        match report.num_jobs() {
            0 => (),
            n => {
                if report.total_wait_seconds() > 0 {
                    writeln!(
                        f,
                        "Number of jobs: {} | Average wait: {}",
                        n,
                        Usage::new(report.total_wait_seconds() / n).in_hours()
                    )?;
                } else {
                    writeln!(f, "Number of jobs: {}", n)?;
                }
            }
        }

        if report.num_requeue_events() > 0 || !report.total_requeue_usage().is_zero() {
            writeln!(
                f,
                "Requeued: {} {} | {} | Average requeue wait: {}",
                report.num_requeue_events(),
                if report.num_requeue_events() == 1 {
                    "event"
                } else {
                    "events"
                },
                report.total_requeue_usage().in_hours(),
                Usage::new(report.average_requeue_wait_seconds()).in_hours()
            )?;
        }

        match report.is_complete() {
            true => writeln!(f, "Total: {}", report.total_usage().in_hours()),
            false => writeln!(f, "Total: {} - incomplete", report.total_usage().in_hours()),
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

    pub fn num_jobs(&self) -> u64 {
        self.num_jobs
    }

    pub fn add_usage(&mut self, local_user: &str, usage: Usage) {
        *self.reports.entry(local_user.to_string()).or_default() += usage;
    }

    pub fn set_usage(&mut self, local_user: &str, usage: Usage) {
        self.reports.insert(local_user.to_string(), usage);
    }

    pub fn add_component_usage(&mut self, component: &str, local_user: &str, usage: Usage) {
        if usage.is_zero() {
            return;
        }

        let component_reports = self.components.entry(component.to_string()).or_default();

        *component_reports.entry(local_user.to_string()).or_default() += usage;
    }

    pub fn set_component_usage(&mut self, component: &str, local_user: &str, usage: Usage) {
        if usage.is_zero() {
            // remove the entry if it exists
            if let Some(component_reports) = self.components.get_mut(component) {
                component_reports.remove(local_user);
            }
            return;
        }

        let component_reports = self.components.entry(component.to_string()).or_default();

        component_reports.insert(local_user.to_string(), usage);
    }

    /// Add jobs attributed to a specific user. Updates both the per-user map
    /// and the scalar total so both are always consistent.
    pub fn add_jobs(&mut self, user: &str, count: u64) {
        *self.user_job_counts.entry(user.to_string()).or_default() += count;
        self.num_jobs += count;
    }

    /// Add wait seconds attributed to a specific user. Updates both the
    /// per-user map and the scalar total.
    pub fn add_wait_seconds(&mut self, user: &str, seconds: u64) {
        *self.user_wait_seconds.entry(user.to_string()).or_default() += seconds;
        self.total_wait_seconds = self.total_wait_seconds.saturating_add(seconds);
    }

    pub fn num_jobs_for_user(&self, user: &str) -> u64 {
        self.user_job_counts.get(user).copied().unwrap_or(0)
    }

    pub fn wait_seconds_for_user(&self, user: &str) -> u64 {
        self.user_wait_seconds.get(user).copied().unwrap_or(0)
    }

    pub fn average_wait_seconds_for_user(&self, user: &str) -> u64 {
        let jobs = self.num_jobs_for_user(user);
        match jobs {
            0 => 0,
            n => self.wait_seconds_for_user(user) / n,
        }
    }

    /// Returns true if the scalar totals equal the sums of the per-user maps.
    /// Always true for legacy data (both maps empty, scalars may be non-zero).
    pub fn is_consistent(&self) -> bool {
        if !self.requeues_are_consistent() {
            return false;
        }

        if self.user_job_counts.is_empty() && self.user_wait_seconds.is_empty() {
            return true; // legacy data — no maps to check against
        }
        let jobs_sum: u64 = self.user_job_counts.values().sum();
        let wait_sum: u64 = self.user_wait_seconds.values().sum();
        jobs_sum == self.num_jobs && wait_sum == self.total_wait_seconds
    }

    /// The same check for the requeue counters.
    ///
    /// Each map is checked only when it is populated. Absent maps are not a
    /// failure: legacy data has none of them, and a component report from
    /// `get_component` deliberately carries the requeue usage of one component
    /// without the per-state breakdown, which describes the whole report and
    /// cannot be apportioned to a single component.
    ///
    /// Where a per-state map *is* present it must account for every event and
    /// every second of requeue usage - which is why an unrecognised Slurm state
    /// has to be bucketed rather than dropped.
    fn requeues_are_consistent(&self) -> bool {
        if !self.user_requeue_events.is_empty() {
            let event_sum: u64 = self.user_requeue_events.values().sum();
            if event_sum != self.num_requeue_events {
                return false;
            }
        }

        if !self.user_requeue_wait_seconds.is_empty() {
            let wait_sum: u64 = self.user_requeue_wait_seconds.values().sum();
            if wait_sum != self.requeue_wait_seconds {
                return false;
            }
        }

        if !self.requeue_states.is_empty() {
            let state_sum: u64 = self.requeue_states.values().sum();
            if state_sum != self.num_requeue_events {
                return false;
            }
        }

        if !self.requeue_state_usage.is_empty() {
            let state_usage: Usage = self.requeue_state_usage.values().cloned().sum();
            if state_usage != self.total_requeue_usage() {
                return false;
            }
        }

        true
    }

    pub fn total_wait_seconds(&self) -> u64 {
        self.total_wait_seconds
    }

    pub fn average_wait_seconds(&self) -> u64 {
        match self.num_jobs {
            0 => 0,
            n => self.total_wait_seconds / n,
        }
    }

    /// Returns a display adapter that formats all usage values in hours only.
    pub fn in_hours(&self) -> DailyProjectUsageReportHoursDisplay<'_> {
        DailyProjectUsageReportHoursDisplay(self)
    }

    pub fn add_unattributed_usage(&mut self, usage: Usage) {
        self.add_usage("unknown", usage);
    }

    pub fn add_unattributed_component_usage(&mut self, component: &str, usage: Usage) {
        self.add_component_usage(component, "unknown", usage);
    }

    pub fn set_unattributed_usage(&mut self, usage: Usage) {
        self.set_usage("unknown", usage);
    }

    pub fn set_unattributed_component_usage(&mut self, component: &str, usage: Usage) {
        self.set_component_usage(component, "unknown", usage);
    }

    pub fn components(&self) -> Vec<String> {
        let mut components = self.components.keys().cloned().collect::<Vec<_>>();
        components.sort();
        components
    }

    // disable the clippy field_reassign_with_default warning
    // It is more robust to create a default and then overwrite
    // the fields that need to change via a clone
    #[allow(clippy::field_reassign_with_default)]
    pub fn get_component(&self, component: &str) -> DailyProjectUsageReport {
        let mut report = DailyProjectUsageReport::default();

        if let Some(reports) = self.components.get(component) {
            for (user, usage) in reports {
                report.set_usage(user, *usage);
            }
        }

        // the requeue usage for the same component, so that a caller asking
        // for "gpu" gets both the base and the requeue figure for GPUs
        if let Some(reports) = self.requeue_components.get(component) {
            report.requeue_reports = reports.clone();
        }

        report.user_job_counts = self.user_job_counts.clone();
        report.user_wait_seconds = self.user_wait_seconds.clone();
        report.num_jobs = self.num_jobs;
        report.total_wait_seconds = self.total_wait_seconds;

        report.user_requeue_events = self.user_requeue_events.clone();
        report.num_requeue_events = self.num_requeue_events;
        report.user_requeue_wait_seconds = self.user_requeue_wait_seconds.clone();
        report.requeue_wait_seconds = self.requeue_wait_seconds;

        // The per-state maps are deliberately not copied. They account for the
        // whole report's requeue events and usage, and there is no way to
        // apportion them to one component - copying them would leave a report
        // whose state breakdown claims more usage than the report contains.

        report.is_complete = self.is_complete;

        report
    }

    // ---- Requeue accounting -------------------------------------------------

    /// Usage this user consumed on attempts that were superseded by a requeue.
    pub fn requeue_usage(&self, local_user: &str) -> Usage {
        self.requeue_reports
            .get(local_user)
            .cloned()
            .unwrap_or_default()
    }

    /// Total usage consumed on attempts that were superseded by a requeue.
    pub fn total_requeue_usage(&self) -> Usage {
        self.requeue_reports.values().cloned().sum()
    }

    /// A project's true consumption: the usage we have always reported, plus
    /// the superseded attempts that were previously invisible.
    pub fn total_usage_including_requeues(&self) -> Usage {
        self.total_usage() + self.total_requeue_usage()
    }

    pub fn add_requeue_usage(&mut self, local_user: &str, usage: Usage) {
        *self
            .requeue_reports
            .entry(local_user.to_string())
            .or_default() += usage;
    }

    pub fn add_requeue_component_usage(&mut self, component: &str, local_user: &str, usage: Usage) {
        if usage.is_zero() {
            return;
        }

        let component_reports = self
            .requeue_components
            .entry(component.to_string())
            .or_default();

        *component_reports.entry(local_user.to_string()).or_default() += usage;
    }

    /// Record `count` requeue events for a user, whose superseded attempts
    /// ended in `state`. The scalar total, the per-user map and the per-state
    /// map are updated together so they cannot drift apart.
    pub fn add_requeue_events(&mut self, user: &str, state: &str, count: u64) {
        *self
            .user_requeue_events
            .entry(user.to_string())
            .or_default() += count;
        self.num_requeue_events = self.num_requeue_events.saturating_add(count);
        *self.requeue_states.entry(state.to_string()).or_default() += count;
    }

    /// Record usage against the terminal state of a superseded attempt. Kept
    /// separate from `add_requeue_events` because a superseded attempt spanning
    /// a window boundary has its usage counted in each window it overlaps - the
    /// part consumed there - while the requeue itself happened at one instant
    /// and is counted once.
    pub fn add_requeue_state_usage(&mut self, state: &str, usage: Usage) {
        if usage.is_zero() {
            return;
        }

        *self
            .requeue_state_usage
            .entry(state.to_string())
            .or_default() += usage;
    }

    pub fn add_requeue_wait_seconds(&mut self, user: &str, seconds: u64) {
        *self
            .user_requeue_wait_seconds
            .entry(user.to_string())
            .or_default() += seconds;
        self.requeue_wait_seconds = self.requeue_wait_seconds.saturating_add(seconds);
    }

    /// The number of requeue *events* - a job requeued four times contributes
    /// four. Deliberately not a count of jobs affected: an event count is
    /// additive over any date range, whereas counting distinct jobs would need
    /// grouping across query windows.
    pub fn num_requeue_events(&self) -> u64 {
        self.num_requeue_events
    }

    pub fn requeue_events_for_user(&self, user: &str) -> u64 {
        self.user_requeue_events.get(user).copied().unwrap_or(0)
    }

    /// Queue wait that was discarded by a requeue: the time each superseded
    /// attempt spent queueing before it ran, only for that run to be thrown
    /// away. A requeue costs a project both the compute it had done and the
    /// waiting it had already served, and this is the second of the two.
    ///
    /// Measured as `eligible -> start`, so the begin-time hold Slurm imposes
    /// after a requeue is excluded - Slurm advances `eligible` past it.
    pub fn requeue_wait_seconds(&self) -> u64 {
        self.requeue_wait_seconds
    }

    pub fn requeue_wait_seconds_for_user(&self, user: &str) -> u64 {
        self.user_requeue_wait_seconds
            .get(user)
            .copied()
            .unwrap_or(0)
    }

    /// Mean wait per requeue - not per job.
    pub fn average_requeue_wait_seconds(&self) -> u64 {
        match self.num_requeue_events {
            0 => 0,
            n => self.requeue_wait_seconds / n,
        }
    }

    /// Mean total queue wait per job, counting the waits of every attempt.
    ///
    /// The two terms do not overlap and neither double counts: a record is
    /// either a job's last attempt in a window or a superseded one, the last
    /// attempt's wait is counted in the window it started in, and a superseded
    /// attempt's wait is counted in the single window that holds its requeue.
    pub fn average_wait_seconds_including_requeues(&self) -> u64 {
        match self.num_jobs {
            0 => 0,
            n => {
                self.total_wait_seconds
                    .saturating_add(self.requeue_wait_seconds)
                    / n
            }
        }
    }

    /// The terminal states of superseded attempts, with their event counts,
    /// sorted by state. `NODE_FAIL` is a site problem, `PREEMPTED` a policy the
    /// project opted into, `CANCELLED` possibly the user's own doing - the flat
    /// requeue total cannot tell them apart.
    pub fn requeue_states(&self) -> Vec<(String, u64)> {
        let mut states: Vec<(String, u64)> = self
            .requeue_states
            .iter()
            .map(|(state, count)| (state.clone(), *count))
            .collect();
        states.sort();
        states
    }

    pub fn requeue_events_in_state(&self, state: &str) -> u64 {
        self.requeue_states.get(state).copied().unwrap_or(0)
    }

    pub fn requeue_usage_in_state(&self, state: &str) -> Usage {
        self.requeue_state_usage
            .get(state)
            .cloned()
            .unwrap_or_default()
    }

    pub fn requeue_components(&self) -> Vec<String> {
        let mut components = self.requeue_components.keys().cloned().collect::<Vec<_>>();
        components.sort();
        components
    }

    pub fn requeue_component_usage(&self, component: &str, local_user: &str) -> Usage {
        self.requeue_components
            .get(component)
            .and_then(|reports| reports.get(local_user))
            .cloned()
            .unwrap_or_default()
    }

    pub fn total_requeue_component_usage(&self, component: &str) -> Usage {
        match self.requeue_components.get(component) {
            Some(reports) => reports.values().cloned().sum(),
            None => Usage::default(),
        }
    }

    /// Scale the usage totals - base and requeue together. The two must always
    /// be scaled by the same factor, or `total_usage()` and
    /// `total_requeue_usage()` end up in different units and the sum a client
    /// makes of them is meaningless.
    fn scale_totals(&mut self, factor: f64) {
        for usage in self.reports.values_mut() {
            *usage *= factor;
        }
        for usage in self.requeue_reports.values_mut() {
            *usage *= factor;
        }
        for usage in self.requeue_state_usage.values_mut() {
            *usage *= factor;
        }
    }

    /// Scale the component breakdowns - base and requeue together, for the
    /// same reason as `scale_totals`.
    fn scale_components(&mut self, factor: f64) {
        for component_reports in self.components.values_mut() {
            for usage in component_reports.values_mut() {
                *usage *= factor;
            }
        }
        for component_reports in self.requeue_components.values_mut() {
            for usage in component_reports.values_mut() {
                *usage *= factor;
            }
        }
    }

    /// `scale_totals`, dividing. Spelled out rather than multiplying by a
    /// reciprocal: `Usage` truncates to whole seconds, so `3 / 3.0` and
    /// `3 * (1.0 / 3.0)` do not agree.
    fn divide_totals(&mut self, divisor: f64) {
        for usage in self.reports.values_mut() {
            *usage /= divisor;
        }
        for usage in self.requeue_reports.values_mut() {
            *usage /= divisor;
        }
        for usage in self.requeue_state_usage.values_mut() {
            *usage /= divisor;
        }
    }

    /// `scale_components`, dividing.
    fn divide_components(&mut self, divisor: f64) {
        for component_reports in self.components.values_mut() {
            for usage in component_reports.values_mut() {
                *usage /= divisor;
            }
        }
        for component_reports in self.requeue_components.values_mut() {
            for usage in component_reports.values_mut() {
                *usage /= divisor;
            }
        }
    }

    pub fn set_complete(&mut self) {
        self.is_complete = true;
    }

    pub fn is_complete(&self) -> bool {
        self.is_complete
    }

    /// Remap local username strings using a pre-built old → new map.
    /// Any username not present in `string_map` is left unchanged.
    pub(crate) fn remap_local_users(&mut self, string_map: &HashMap<String, String>) {
        let old_reports = std::mem::take(&mut self.reports);
        self.reports = old_reports
            .into_iter()
            .map(|(user, usage)| {
                let new_user = string_map.get(&user).cloned().unwrap_or(user);
                (new_user, usage)
            })
            .collect();

        let old_components = std::mem::take(&mut self.components);
        self.components = old_components
            .into_iter()
            .map(|(component, user_map)| {
                let new_user_map = user_map
                    .into_iter()
                    .map(|(user, usage)| {
                        let new_user = string_map.get(&user).cloned().unwrap_or(user);
                        (new_user, usage)
                    })
                    .collect();
                (component, new_user_map)
            })
            .collect();

        let old_counts = std::mem::take(&mut self.user_job_counts);
        self.user_job_counts = old_counts
            .into_iter()
            .map(|(user, count)| {
                let new_user = string_map.get(&user).cloned().unwrap_or(user);
                (new_user, count)
            })
            .collect();

        let old_waits = std::mem::take(&mut self.user_wait_seconds);
        self.user_wait_seconds = old_waits
            .into_iter()
            .map(|(user, secs)| {
                let new_user = string_map.get(&user).cloned().unwrap_or(user);
                (new_user, secs)
            })
            .collect();

        // The requeue maps are keyed the same way and need the same treatment.
        // `requeue_states` and `requeue_state_usage` are keyed by Slurm state
        // rather than by user, so they are deliberately left alone.
        let old_requeue = std::mem::take(&mut self.requeue_reports);
        self.requeue_reports = old_requeue
            .into_iter()
            .map(|(user, usage)| {
                let new_user = string_map.get(&user).cloned().unwrap_or(user);
                (new_user, usage)
            })
            .collect();

        let old_requeue_components = std::mem::take(&mut self.requeue_components);
        self.requeue_components = old_requeue_components
            .into_iter()
            .map(|(component, user_map)| {
                let new_user_map = user_map
                    .into_iter()
                    .map(|(user, usage)| {
                        let new_user = string_map.get(&user).cloned().unwrap_or(user);
                        (new_user, usage)
                    })
                    .collect();
                (component, new_user_map)
            })
            .collect();

        let old_requeue_events = std::mem::take(&mut self.user_requeue_events);
        self.user_requeue_events = old_requeue_events
            .into_iter()
            .map(|(user, count)| {
                let new_user = string_map.get(&user).cloned().unwrap_or(user);
                (new_user, count)
            })
            .collect();

        let old_requeue_waits = std::mem::take(&mut self.user_requeue_wait_seconds);
        self.user_requeue_wait_seconds = old_requeue_waits
            .into_iter()
            .map(|(user, secs)| {
                let new_user = string_map.get(&user).cloned().unwrap_or(user);
                (new_user, secs)
            })
            .collect();
    }
}

impl std::ops::Add<DailyProjectUsageReport> for DailyProjectUsageReport {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        let mut new_report = self.clone();

        for (user, usage) in other.reports {
            new_report.add_usage(&user, usage);
        }

        // now do the same for the components
        for (component, reports) in other.components {
            for (user, usage) in reports {
                new_report.add_component_usage(&component, &user, usage);
            }
        }

        for (user, count) in &other.user_job_counts {
            *new_report.user_job_counts.entry(user.clone()).or_default() += count;
        }
        for (user, secs) in &other.user_wait_seconds {
            *new_report
                .user_wait_seconds
                .entry(user.clone())
                .or_default() += secs;
        }
        // Saturating: these totals are summed from peer-supplied reports, and
        // `overflow-checks` is on in release, so a bare `+` is a process kill.
        new_report.num_jobs = self.num_jobs.saturating_add(other.num_jobs);
        new_report.total_wait_seconds = self
            .total_wait_seconds
            .saturating_add(other.total_wait_seconds);

        for (user, usage) in other.requeue_reports {
            new_report.add_requeue_usage(&user, usage);
        }
        for (component, reports) in other.requeue_components {
            for (user, usage) in reports {
                new_report.add_requeue_component_usage(&component, &user, usage);
            }
        }
        for (user, count) in &other.user_requeue_events {
            *new_report
                .user_requeue_events
                .entry(user.clone())
                .or_default() += count;
        }
        for (user, secs) in &other.user_requeue_wait_seconds {
            *new_report
                .user_requeue_wait_seconds
                .entry(user.clone())
                .or_default() += secs;
        }
        for (state, count) in &other.requeue_states {
            *new_report.requeue_states.entry(state.clone()).or_default() += count;
        }
        for (state, usage) in other.requeue_state_usage {
            new_report.add_requeue_state_usage(&state, usage);
        }
        new_report.num_requeue_events = self
            .num_requeue_events
            .saturating_add(other.num_requeue_events);
        new_report.requeue_wait_seconds = self
            .requeue_wait_seconds
            .saturating_add(other.requeue_wait_seconds);

        new_report.is_complete = false; // combine reports are never complete

        new_report
    }
}

impl std::ops::AddAssign<DailyProjectUsageReport> for DailyProjectUsageReport {
    fn add_assign(&mut self, other: Self) {
        for (user, usage) in other.reports {
            self.add_usage(&user, usage);
        }

        // now do the same for the components
        for (component, reports) in other.components {
            for (user, usage) in reports {
                self.add_component_usage(&component, &user, usage);
            }
        }

        for (user, count) in &other.user_job_counts {
            *self.user_job_counts.entry(user.clone()).or_default() += count;
        }
        for (user, secs) in &other.user_wait_seconds {
            *self.user_wait_seconds.entry(user.clone()).or_default() += secs;
        }
        self.num_jobs = self.num_jobs.saturating_add(other.num_jobs);
        self.total_wait_seconds = self
            .total_wait_seconds
            .saturating_add(other.total_wait_seconds);

        for (user, usage) in other.requeue_reports {
            self.add_requeue_usage(&user, usage);
        }
        for (component, reports) in other.requeue_components {
            for (user, usage) in reports {
                self.add_requeue_component_usage(&component, &user, usage);
            }
        }
        for (user, count) in &other.user_requeue_events {
            *self.user_requeue_events.entry(user.clone()).or_default() += count;
        }
        for (user, secs) in &other.user_requeue_wait_seconds {
            *self
                .user_requeue_wait_seconds
                .entry(user.clone())
                .or_default() += secs;
        }
        for (state, count) in &other.requeue_states {
            *self.requeue_states.entry(state.clone()).or_default() += count;
        }
        for (state, usage) in other.requeue_state_usage {
            self.add_requeue_state_usage(&state, usage);
        }
        self.num_requeue_events = self
            .num_requeue_events
            .saturating_add(other.num_requeue_events);
        self.requeue_wait_seconds = self
            .requeue_wait_seconds
            .saturating_add(other.requeue_wait_seconds);

        self.is_complete = false; // combine reports are never complete
    }
}

impl std::ops::Mul<f64> for DailyProjectUsageReport {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self {
        let mut new_report = self.clone();
        new_report.scale_totals(rhs);
        new_report.scale_components(rhs);
        new_report
    }
}

impl std::ops::Div<f64> for DailyProjectUsageReport {
    type Output = Self;

    fn div(self, rhs: f64) -> Self {
        let mut new_report = self.clone();
        new_report.divide_totals(rhs);
        new_report.divide_components(rhs);
        new_report
    }
}

impl std::ops::MulAssign<f64> for DailyProjectUsageReport {
    fn mul_assign(&mut self, rhs: f64) {
        self.scale_totals(rhs);
    }
}

impl std::ops::DivAssign<f64> for DailyProjectUsageReport {
    fn div_assign(&mut self, rhs: f64) {
        self.divide_totals(rhs);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectUsageReport {
    #[ts(as = "String")]
    project: ProjectIdentifier,
    #[ts(as = "HashMap<String, DailyProjectUsageReport>")]
    reports: HashMap<Date, DailyProjectUsageReport>,
    #[ts(as = "HashMap<String, String>")]
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
            let report = self.reports.get(date).cloned().unwrap_or_default();

            if report.total_usage() == Usage::default() {
                // skip days with no usage
                continue;
            }

            writeln!(f, "{}", date)?;

            for user in report.local_users() {
                let jobs = report.num_jobs_for_user(&user);
                let usage_str = report.usage(&user).to_string();
                let label = match users.get(&user) {
                    Some(userid) => format!("  {}", userid),
                    None => format!("  {} - unknown", user),
                };
                if jobs > 0 {
                    writeln!(
                        f,
                        "{}: {} | {} {} | Average wait: {}",
                        label,
                        usage_str,
                        jobs,
                        if jobs == 1 { "job" } else { "jobs" },
                        Usage::new(report.average_wait_seconds_for_user(&user))
                    )?;
                } else {
                    writeln!(f, "{}: {}", label, usage_str)?;
                }
            }

            match report.num_jobs() {
                0 => (),
                n => {
                    if report.total_wait_seconds() > 0 {
                        writeln!(
                            f,
                            "Number of jobs: {} | Average wait: {}",
                            n,
                            Usage::new(report.total_wait_seconds() / n)
                        )?;
                    } else {
                        writeln!(f, "Number of jobs: {}", n)?;
                    }
                }
            }
            writeln!(f, "Daily total: {}", report.total_usage())?;
            writeln!(f, "----------------------------------------")?;
        }

        writeln!(f, "========================================")?;
        match self.num_jobs() {
            0 => (),
            n => {
                if self.total_wait_seconds() > 0 {
                    writeln!(
                        f,
                        "Number of jobs: {} | Average wait: {}",
                        n,
                        Usage::new(self.total_wait_seconds() / n)
                    )?;
                } else {
                    writeln!(f, "Number of jobs: {}", n)?;
                }
            }
        }
        if self.num_requeue_events() > 0 || !self.total_requeue_usage().is_zero() {
            writeln!(
                f,
                "Requeued: {} {} | {} | Average requeue wait: {}",
                self.num_requeue_events(),
                if self.num_requeue_events() == 1 {
                    "event"
                } else {
                    "events"
                },
                self.total_requeue_usage(),
                Usage::new(self.average_requeue_wait_seconds())
            )?;
        }

        writeln!(f, "Total: {}", self.total_usage())
    }
}

/// Display adapter that formats all [`Usage`] values in a
/// [`ProjectUsageReport`] in hours. Obtained via
/// [`ProjectUsageReport::in_hours`].
pub struct ProjectUsageReportHoursDisplay<'a>(&'a ProjectUsageReport);

impl std::fmt::Display for ProjectUsageReportHoursDisplay<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let report = self.0;
        writeln!(f, "{}", report.project())?;

        let mut dates = report.reports.keys().collect::<Vec<_>>();
        dates.sort();

        let mut users = HashMap::new();
        for (user, local_user) in &report.users {
            users.insert(local_user, user);
        }

        for date in dates {
            let daily = report.reports.get(date).cloned().unwrap_or_default();

            if daily.total_usage() == Usage::default() {
                continue;
            }

            writeln!(f, "{}", date)?;

            for user in daily.local_users() {
                let jobs = daily.num_jobs_for_user(&user);
                let usage_str = daily.usage(&user).in_hours().to_string();
                let label = match users.get(&user) {
                    Some(userid) => format!("  {}", userid),
                    None => format!("  {} - unknown", user),
                };
                if jobs > 0 {
                    writeln!(
                        f,
                        "{}: {} | {} {} | Average wait: {}",
                        label,
                        usage_str,
                        jobs,
                        if jobs == 1 { "job" } else { "jobs" },
                        Usage::new(daily.average_wait_seconds_for_user(&user)).in_hours()
                    )?;
                } else {
                    writeln!(f, "{}: {}", label, usage_str)?;
                }
            }

            match daily.num_jobs() {
                0 => (),
                n => {
                    if daily.total_wait_seconds() > 0 {
                        writeln!(
                            f,
                            "Number of jobs: {} | Average wait: {}",
                            n,
                            Usage::new(daily.total_wait_seconds() / n).in_hours()
                        )?;
                    } else {
                        writeln!(f, "Number of jobs: {}", n)?;
                    }
                }
            }
            writeln!(f, "Daily total: {}", daily.total_usage().in_hours())?;
            writeln!(f, "----------------------------------------")?;
        }

        writeln!(f, "========================================")?;
        match report.num_jobs() {
            0 => (),
            n => {
                if report.total_wait_seconds() > 0 {
                    writeln!(
                        f,
                        "Number of jobs: {} | Average wait: {}",
                        n,
                        Usage::new(report.total_wait_seconds() / n).in_hours()
                    )?;
                } else {
                    writeln!(f, "Number of jobs: {}", n)?;
                }
            }
        }
        if report.num_requeue_events() > 0 || !report.total_requeue_usage().is_zero() {
            writeln!(
                f,
                "Requeued: {} {} | {} | Average requeue wait: {}",
                report.num_requeue_events(),
                if report.num_requeue_events() == 1 {
                    "event"
                } else {
                    "events"
                },
                report.total_requeue_usage().in_hours(),
                Usage::new(report.average_requeue_wait_seconds()).in_hours()
            )?;
        }

        writeln!(f, "Total: {}", report.total_usage().in_hours())
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
                Some(existing_report) => *existing_report += report,
                None => {
                    new_report.reports.insert(date, report);
                }
            }
        }

        for (user, local_user) in other.users {
            new_report.users.entry(user).or_insert(local_user);
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
                Some(existing_report) => *existing_report += report,
                None => {
                    self.reports.insert(date, report);
                }
            }
        }

        for (user, local_user) in other.users {
            self.users.entry(user).or_insert(local_user);
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
            for component_reports in report.components.values_mut() {
                for usage in component_reports.values_mut() {
                    *usage *= rhs;
                }
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
            for component_reports in report.components.values_mut() {
                for usage in component_reports.values_mut() {
                    *usage /= rhs;
                }
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
            for component_reports in report.components.values_mut() {
                for usage in component_reports.values_mut() {
                    *usage *= rhs;
                }
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
            for component_reports in report.components.values_mut() {
                for usage in component_reports.values_mut() {
                    *usage /= rhs;
                }
            }
        }
    }
}

impl ProjectUsageReport {
    /// Replace the project identifier on this report.
    /// Use this when re-labelling a report for a different portal's identifier
    /// (e.g. converting from a local project identifier to a remote one before
    /// merging with a report built for the remote identifier).
    pub fn set_project(&mut self, project: &ProjectIdentifier) {
        self.project = project.clone();
    }

    /// Scale only the main usage totals, leaving component breakdowns unchanged.
    /// Use this when the scale factor converts credit units but components are
    /// in physical units (GPU-hours, CPU-hours etc.) that should not be scaled.
    ///
    /// The requeue totals are scaled with the base totals, never separately: a
    /// caller converts to credits and then subtracts one from the other, so the
    /// two must stay in the same units. The requeue *component* breakdowns are
    /// left alone for the same reason the base ones are.
    pub fn scale_total(&mut self, factor: f64) {
        for report in self.reports.values_mut() {
            for usage in report.reports.values_mut() {
                *usage *= factor;
            }
            // unattributed usage is also in the reports map under a special key
            for usage in report.requeue_reports.values_mut() {
                *usage *= factor;
            }
            for usage in report.requeue_state_usage.values_mut() {
                *usage *= factor;
            }
        }
    }

    pub fn new(project: &ProjectIdentifier) -> Self {
        Self {
            project: project.clone(),
            reports: HashMap::new(),
            users: HashMap::new(),
        }
    }

    pub fn to_json(&self) -> Result<String, Error> {
        serde_json::to_string(self)
            .with_context(|| "Failed to serialize ProjectUsageReport to JSON".to_string())
            .map_err(Error::from)
    }

    pub fn from_json(json: &str) -> Result<Self, Error> {
        serde_json::from_str(json)
            .with_context(|| "Failed to deserialize ProjectUsageReport from JSON".to_string())
            .map_err(Error::from)
    }

    pub fn dates(&self) -> Vec<Date> {
        let mut dates: Vec<Date> = self.reports.keys().cloned().collect();

        dates.sort();

        dates
    }

    /// Return a copy of this report containing only days that fall within
    /// `range` (inclusive on both ends).
    pub fn filter(&self, range: &DateRange) -> Self {
        let reports = self
            .reports
            .iter()
            .filter(|(date, _)| *date >= range.start_date() && *date <= range.end_date())
            .map(|(date, report)| (date.clone(), report.clone()))
            .collect();

        Self {
            project: self.project.clone(),
            reports,
            users: self.users.clone(),
        }
    }

    pub fn components(&self) -> Vec<String> {
        let mut components: std::collections::HashSet<String> = std::collections::HashSet::new();

        for report in self.reports.values() {
            for component in report.components() {
                components.insert(component);
            }
        }

        let mut components: Vec<String> = components.into_iter().collect();

        components.sort();

        components
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

    /// Return the full portal-user → local-username map.
    pub fn user_mapping(&self) -> HashMap<UserIdentifier, String> {
        self.users.clone()
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
        self.reports.values().map(|r| r.total_usage()).sum()
    }

    pub fn num_jobs(&self) -> u64 {
        self.reports.values().map(|r| r.num_jobs()).sum()
    }

    pub fn total_wait_seconds(&self) -> u64 {
        self.reports.values().map(|r| r.total_wait_seconds()).sum()
    }

    pub fn average_wait_seconds(&self) -> u64 {
        let num_jobs = self.num_jobs();
        match num_jobs {
            0 => 0,
            n => self.total_wait_seconds() / n,
        }
    }

    /// Usage consumed on attempts superseded by a requeue. This is the usage
    /// that was invisible before requeue accounting: `total_usage()` counts
    /// only each job's last attempt.
    pub fn total_requeue_usage(&self) -> Usage {
        self.reports.values().map(|r| r.total_requeue_usage()).sum()
    }

    /// This project's true consumption - `total_usage()` plus every superseded
    /// attempt.
    pub fn total_usage_including_requeues(&self) -> Usage {
        self.total_usage() + self.total_requeue_usage()
    }

    /// The number of requeue events, not the number of jobs requeued.
    pub fn num_requeue_events(&self) -> u64 {
        self.reports.values().fold(0u64, |total, r| {
            total.saturating_add(r.num_requeue_events())
        })
    }

    pub fn requeue_wait_seconds(&self) -> u64 {
        self.reports.values().fold(0u64, |total, r| {
            total.saturating_add(r.requeue_wait_seconds())
        })
    }

    /// Mean wait per requeue - not per job.
    pub fn average_requeue_wait_seconds(&self) -> u64 {
        match self.num_requeue_events() {
            0 => 0,
            n => self.requeue_wait_seconds() / n,
        }
    }

    /// Mean total queue wait per job, counting the waits of every attempt.
    pub fn average_wait_seconds_including_requeues(&self) -> u64 {
        match self.num_jobs() {
            0 => 0,
            n => {
                self.total_wait_seconds()
                    .saturating_add(self.requeue_wait_seconds())
                    / n
            }
        }
    }

    /// Requeue events by the terminal state of the superseded attempt, summed
    /// over every day in this report and sorted by state.
    pub fn requeue_states(&self) -> Vec<(String, u64)> {
        let mut totals: HashMap<String, u64> = HashMap::new();

        for report in self.reports.values() {
            for (state, count) in report.requeue_states() {
                *totals.entry(state).or_default() += count;
            }
        }

        let mut states: Vec<(String, u64)> = totals.into_iter().collect();
        states.sort();
        states
    }

    /// Requeue usage by the terminal state of the superseded attempt.
    pub fn requeue_usage_in_state(&self, state: &str) -> Usage {
        self.reports
            .values()
            .map(|r| r.requeue_usage_in_state(state))
            .sum()
    }

    /// Returns a display adapter that formats all usage values in hours only.
    pub fn in_hours(&self) -> ProjectUsageReportHoursDisplay<'_> {
        ProjectUsageReportHoursDisplay(self)
    }

    pub fn daily_reports(&self, with_usage_only: bool) -> Vec<DailyProjectUsageReport> {
        let mut dates: Vec<&Date> = self.reports.keys().collect();
        dates.sort();

        dates
            .into_iter()
            .filter_map(|date| {
                let report = self.reports.get(date)?;
                if with_usage_only && report.total_usage() == Usage::default() {
                    return None;
                }
                Some(report.clone())
            })
            .collect()
    }

    pub fn unmapped_usage(&self) -> Usage {
        let unmapped_users = self.unmapped_users();

        if unmapped_users.is_empty() {
            return Usage::default();
        }

        self.reports
            .values()
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

    pub fn get_component(&self, component: &str) -> ProjectUsageReport {
        let mut reports = HashMap::new();

        for (date, daily_report) in &self.reports {
            let component_report = daily_report.get_component(component);
            reports.insert(date.clone(), component_report);
        }

        ProjectUsageReport {
            project: self.project.clone(),
            reports,
            users: self.users.clone(),
        }
    }

    pub fn combine(reports: &[ProjectUsageReport]) -> Result<Self, Error> {
        let Some(first) = reports.first() else {
            return Err(Error::InvalidState("No reports to combine".to_string()));
        };

        let mut combined = ProjectUsageReport::new(&first.project);

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

    /// Remap this report to a new project identifier.
    ///
    /// Updates the top-level `project` field and rebuilds the `users` map so
    /// that every `UserIdentifier` key reflects the new project and portal
    /// (i.e. `username.old_project.old_portal` becomes
    /// `username.new_project.new_portal`).
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
        Ok(())
    }

    /// Remap this report to a new portal, keeping the project name unchanged.
    ///
    /// Convenience wrapper around [`ProjectUsageReport::remap_project`] that
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
    /// `new_usermapping` maps each `UserIdentifier` (as it currently appears in
    /// this report's `users` map) to a new local-username string.  Only users
    /// present in both `self.users` and `new_usermapping` are updated; others
    /// are left unchanged.
    ///
    /// Returns an error if the remapping would cause two distinct users to
    /// share the same local-username string.
    pub fn remap_users(
        &mut self,
        new_usermapping: &HashMap<UserIdentifier, String>,
    ) -> Result<(), Error> {
        // Check that the remapping is injective (no two users collapse to the
        // same local string).
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

        // Build old-local → new-local map for updating the daily reports.
        let mut string_map: HashMap<String, String> = HashMap::new();
        for (uid, old_local) in &self.users {
            if let Some(new_local) = new_usermapping.get(uid) {
                string_map.insert(old_local.clone(), new_local.clone());
            }
        }

        // Update the users map values.
        for (uid, local) in self.users.iter_mut() {
            if let Some(new_local) = new_usermapping.get(uid) {
                *local = new_local.clone();
            }
        }

        // Propagate to each daily report.
        for daily in self.reports.values_mut() {
            daily.remap_local_users(&string_map);
        }

        Ok(())
    }

    pub fn to_usage_report(&self) -> UsageReport {
        let mut r = UsageReport::new(&self.project.portal_identifier());
        r.reports.insert(self.project.clone(), self.clone());
        r
    }
}

/// A portal-level usage report.
///
/// Deserialised via `try_from` so a wire-supplied report cannot carry map keys that
/// disagree with its own `portal` field - `set_report` enforces that on the
/// programmatic path, but the derive inserted whatever it was given. It only matters
/// for a receiver that trusts the keys rather than re-inserting, but a type whose
/// invariant holds only on one of two construction paths is a trap. See
/// `docs/specifications/security-review-2.md` (finding R33).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct UsageReport {
    #[ts(as = "String")]
    portal: PortalIdentifier,
    #[ts(as = "HashMap<String, ProjectUsageReport>")]
    reports: HashMap<ProjectIdentifier, ProjectUsageReport>,
}

/// The wire shape of a [`UsageReport`] - identical to what the derive produced, so the
/// format is unchanged. Deserialising goes through [`UsageReport::set_report`] so a
/// report whose keys disagree with its `portal` is rejected rather than accepted.
#[derive(Deserialize)]
struct UsageReportRepr {
    portal: PortalIdentifier,
    reports: HashMap<ProjectIdentifier, ProjectUsageReport>,
}

impl TryFrom<UsageReportRepr> for UsageReport {
    type Error = Error;

    fn try_from(repr: UsageReportRepr) -> Result<Self, Self::Error> {
        let mut report = UsageReport::new(&repr.portal);

        for (project, project_report) in repr.reports {
            if project != project_report.project() {
                return Err(Error::InvalidState(format!(
                    "Usage report is keyed on project {} but the report it holds is for \
                     {}",
                    project,
                    project_report.project()
                )));
            }

            report.set_report(project_report)?;
        }

        Ok(report)
    }
}

impl<'de> Deserialize<'de> for UsageReport {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        UsageReportRepr::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
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

    pub fn to_json(&self) -> Result<String, Error> {
        serde_json::to_string(self)
            .with_context(|| "Failed to serialize UsageReport to JSON".to_string())
            .map_err(Error::from)
    }

    pub fn from_json(json: &str) -> Result<Self, Error> {
        serde_json::from_str(json)
            .with_context(|| "Failed to deserialize UsageReport from JSON".to_string())
            .map_err(Error::from)
    }

    pub fn portal(&self) -> &PortalIdentifier {
        &self.portal
    }

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

    pub fn components(&self) -> Vec<String> {
        let mut components: std::collections::HashSet<String> = std::collections::HashSet::new();

        for report in self.reports.values() {
            for component in report.components() {
                components.insert(component);
            }
        }

        let mut components: Vec<String> = components.into_iter().collect();

        components.sort();

        components
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

    /// Return a copy of this report with every contained `ProjectUsageReport`
    /// filtered to only the days that fall within `range` (inclusive).
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

    pub fn get_component(&self, component: &str) -> UsageReport {
        let mut reports = HashMap::new();

        for (project, project_report) in &self.reports {
            let component_report = project_report.get_component(component);
            reports.insert(project.clone(), component_report);
        }

        UsageReport {
            portal: self.portal.clone(),
            reports,
        }
    }

    pub fn total_usage(&self) -> Usage {
        self.reports.values().map(|r| r.total_usage()).sum()
    }

    /// Usage consumed on attempts superseded by a requeue - see
    /// `ProjectUsageReport::total_requeue_usage`.
    pub fn total_requeue_usage(&self) -> Usage {
        self.reports.values().map(|r| r.total_requeue_usage()).sum()
    }

    /// True consumption across every project in this report.
    pub fn total_usage_including_requeues(&self) -> Usage {
        self.total_usage() + self.total_requeue_usage()
    }

    /// The number of requeue events, not the number of jobs requeued.
    pub fn num_requeue_events(&self) -> u64 {
        self.reports.values().fold(0u64, |total, r| {
            total.saturating_add(r.num_requeue_events())
        })
    }

    /// Remap all projects in this report to a new portal.
    ///
    /// Updates `self.portal` and remaps every contained `ProjectUsageReport`
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
    /// Finds the contained `ProjectUsageReport` keyed by `old_project`,
    /// delegates to [`ProjectUsageReport::remap_project`] with `new_project`,
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
    /// Delegates to [`ProjectUsageReport::remap_users`] for each project.
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

    pub fn combine(reports: &[UsageReport]) -> Result<Self, Error> {
        let Some(first) = reports.first() else {
            return Err(Error::InvalidState("No reports to combine".to_string()));
        };

        let mut combined = UsageReport::new(&first.portal);

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
            } else if self.is_billing_hours() {
                if node.billing() == 0 {
                    return Err(Error::InvalidState(
                        "Node has no billing factor, cannot convert billing hours to node hours"
                            .to_string(),
                    ));
                }

                return Ok(Usage::from_hours(size / (node.billing()) as f64));
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

    pub fn to_billing_hours(&self, node: &Node) -> Result<Usage, Error> {
        Ok(self.to_node_hours(node)? * node.billing() as f64)
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

    pub fn from_billing_hours(usage: &Usage, node: &Node) -> Result<Self, Error> {
        Allocation::from_size_and_units(usage.hours() / node.billing() as f64, "BHR")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A daily report with both a base and a requeue figure, as `op-slurm`
    /// builds one: two jobs' worth of base usage and three superseded attempts.
    fn report_with_requeues() -> DailyProjectUsageReport {
        let mut report = DailyProjectUsageReport::default();

        report.add_usage("alice", Usage::new(1800));
        report.add_component_usage("cpu", "alice", Usage::new(3600));
        report.add_jobs("alice", 1);
        report.add_wait_seconds("alice", 60);

        report.add_usage("bob", Usage::new(600));
        report.add_jobs("bob", 1);
        report.add_wait_seconds("bob", 30);

        report.add_requeue_usage("alice", Usage::new(7200));
        report.add_requeue_state_usage("NODE_FAIL", Usage::new(7200));
        report.add_requeue_component_usage("cpu", "alice", Usage::new(14400));
        report.add_requeue_events("alice", "NODE_FAIL", 2);
        report.add_requeue_wait_seconds("alice", 300);

        report.add_requeue_usage("bob", Usage::new(900));
        report.add_requeue_state_usage("PREEMPTED", Usage::new(900));
        report.add_requeue_events("bob", "PREEMPTED", 1);
        report.add_requeue_wait_seconds("bob", 120);

        report
    }

    #[test]
    fn test_requeue_usage_is_reported_separately_from_the_usage_we_always_reported() {
        // The contract: `total_usage` is unchanged by requeue accounting, the
        // requeue figure is carried alongside it, and the sum is a project's
        // true consumption. Which of the two to charge for is a policy
        // decision, so both have to survive to the client.
        let report = report_with_requeues();

        assert_eq!(report.total_usage(), Usage::new(2400));
        assert_eq!(report.total_requeue_usage(), Usage::new(8100));
        assert_eq!(report.total_usage_including_requeues(), Usage::new(10500));

        // events are counted, not jobs: alice was requeued twice
        assert_eq!(report.num_jobs(), 2);
        assert_eq!(report.num_requeue_events(), 3);
        assert_eq!(report.requeue_events_for_user("alice"), 2);

        // and the three wait figures a client can now derive
        assert_eq!(report.average_wait_seconds(), 45);
        assert_eq!(report.average_requeue_wait_seconds(), 140);
        assert_eq!(report.average_wait_seconds_including_requeues(), 255);
    }

    #[test]
    fn test_requeue_events_are_bucketed_by_terminal_state() {
        // A node failure is the site's problem and a preemption is site policy
        // the project opted into - different arguments about who pays, which the
        // flat requeue total cannot distinguish.
        let report = report_with_requeues();

        assert_eq!(
            report.requeue_states(),
            vec![("NODE_FAIL".to_string(), 2), ("PREEMPTED".to_string(), 1)]
        );
        assert_eq!(report.requeue_usage_in_state("NODE_FAIL"), Usage::new(7200));
        assert_eq!(report.requeue_usage_in_state("PREEMPTED"), Usage::new(900));

        // the per-state maps must account for every event and every second
        assert_eq!(
            report
                .requeue_states()
                .iter()
                .map(|(_, count)| count)
                .sum::<u64>(),
            report.num_requeue_events()
        );
        assert!(report.is_consistent());
    }

    #[test]
    fn test_scaling_keeps_base_and_requeue_usage_in_the_same_units() {
        // A client converts to credits and then subtracts one figure from the
        // other. If a scale factor reached only one of them the subtraction
        // would be between two different units - which is why the requeue
        // figures are first-class fields rather than another entry in the
        // `components` map, whose units deliberately differ from the total's.
        let report = report_with_requeues();

        let doubled = report.clone() * 2.0;
        assert_eq!(doubled.total_usage(), Usage::new(4800));
        assert_eq!(doubled.total_requeue_usage(), Usage::new(16200));
        assert_eq!(
            doubled.requeue_usage_in_state("NODE_FAIL"),
            Usage::new(14400)
        );

        let halved = report.clone() / 2.0;
        assert_eq!(halved.total_usage(), Usage::new(1200));
        assert_eq!(halved.total_requeue_usage(), Usage::new(4050));

        // `*=` scales the totals but not the component breakdowns, as it always
        // has - the base and requeue totals still move together
        let mut in_place = report.clone();
        in_place *= 3.0;
        assert_eq!(in_place.total_usage(), Usage::new(7200));
        assert_eq!(in_place.total_requeue_usage(), Usage::new(24300));

        // and `scale_total` at the project level, which is what the Python
        // bindings expose for a credit conversion
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let mut project_report = ProjectUsageReport::new(&project);
        project_report.set_report(&Date::parse("2026-03-01").unwrap(), &report);
        project_report.scale_total(0.5);

        assert_eq!(project_report.total_usage(), Usage::new(1200));
        assert_eq!(project_report.total_requeue_usage(), Usage::new(4050));
    }

    #[test]
    fn test_merging_reports_adds_the_requeue_figures_too() {
        let mut merged = report_with_requeues();
        merged += report_with_requeues();

        assert_eq!(merged.total_usage(), Usage::new(4800));
        assert_eq!(merged.total_requeue_usage(), Usage::new(16200));
        assert_eq!(merged.num_requeue_events(), 6);
        assert_eq!(merged.requeue_wait_seconds(), 840);
        assert_eq!(merged.requeue_events_in_state("NODE_FAIL"), 4);
        assert!(merged.is_consistent());

        let summed = report_with_requeues() + report_with_requeues();
        assert_eq!(summed.total_requeue_usage(), merged.total_requeue_usage());
        assert_eq!(summed.num_requeue_events(), merged.num_requeue_events());
        assert!(summed.is_consistent());
    }

    #[test]
    fn test_a_report_from_an_instance_without_requeue_accounting_still_loads() {
        // Every requeue field is `serde(default)`, so a report from a peer that
        // predates them deserialises as "no requeues seen" rather than failing.
        // Nothing on the wire had to change to deploy this.
        let legacy = serde_json::json!({
            "reports": { "alice": { "seconds": 1800 } },
            "num_jobs": 1,
            "total_wait_seconds": 60,
            "is_complete": true
        });

        let report: DailyProjectUsageReport = serde_json::from_value(legacy).unwrap();

        assert_eq!(report.total_usage(), Usage::new(1800));
        assert_eq!(report.num_jobs(), 1);
        assert_eq!(report.total_requeue_usage(), Usage::default());
        assert_eq!(report.num_requeue_events(), 0);
        assert!(report.requeue_states().is_empty());
        assert!(report.is_consistent());

        // and a round trip of a report that does carry them keeps them
        let round_tripped: DailyProjectUsageReport =
            serde_json::from_str(&serde_json::to_string(&report_with_requeues()).unwrap()).unwrap();
        assert_eq!(round_tripped.total_requeue_usage(), Usage::new(8100));
        assert_eq!(round_tripped.num_requeue_events(), 3);
    }

    #[test]
    fn test_a_component_report_carries_the_requeue_usage_for_that_component() {
        // Asking for "cpu" gives both what the final attempts spent on CPU and
        // what the superseded ones did - the requeued GPU-seconds of a
        // preemption-heavy project being the more interesting figure in
        // practice.
        let report = report_with_requeues();
        let cpu = report.get_component("cpu");

        assert_eq!(cpu.total_usage(), Usage::new(3600));
        assert_eq!(cpu.total_requeue_usage(), Usage::new(14400));
        assert_eq!(cpu.num_requeue_events(), 3);
        assert_eq!(report.requeue_components(), vec!["cpu".to_string()]);
        assert!(cpu.is_consistent());

        // the per-state breakdown describes the whole report, so it is not
        // carried onto a single component - there is no way to apportion it
        assert!(cpu.requeue_states().is_empty());
    }

    #[test]
    fn test_renaming_local_users_moves_their_requeue_figures_with_them() {
        let mut report = report_with_requeues();
        let mut renames = HashMap::new();
        renames.insert("alice".to_string(), "alice2".to_string());
        report.remap_local_users(&renames);

        assert_eq!(report.requeue_usage("alice2"), Usage::new(7200));
        assert_eq!(report.requeue_usage("alice"), Usage::default());
        assert_eq!(report.requeue_events_for_user("alice2"), 2);
        assert_eq!(report.requeue_wait_seconds_for_user("alice2"), 300);
        assert_eq!(
            report.requeue_component_usage("cpu", "alice2"),
            Usage::new(14400)
        );
        assert!(report.is_consistent());
    }

    #[test]
    fn test_usage_arithmetic_saturates_rather_than_wrapping() {
        // Release builds now set `overflow-checks = true`, so an unchecked
        // overflow would abort - and with `panic = "abort"` that is a remote
        // process kill, since these values come from peer-supplied reports.
        // Every operator must saturate. See
        // docs/specifications/security-review-2.md (finding R33).
        let max = Usage::new(u64::MAX);
        let one = Usage::new(1);

        assert_eq!((max + one).seconds(), u64::MAX);
        assert_eq!([max, one].into_iter().sum::<Usage>().seconds(), u64::MAX);

        let mut acc = max;
        acc += one;
        assert_eq!(acc.seconds(), u64::MAX);

        // Underflow clamps at zero rather than wrapping to near u64::MAX. The
        // `-=` form used to wrap while `-` already clamped, so the two
        // disagreed.
        assert_eq!((one - max).seconds(), 0);

        let mut acc = one;
        acc -= max;
        assert_eq!(acc.seconds(), 0);

        // Ordinary values are unaffected.
        assert_eq!((Usage::new(60) + Usage::new(30)).seconds(), 90);
        assert_eq!((Usage::new(60) - Usage::new(30)).seconds(), 30);
    }

    #[test]
    fn test_usage_parse_saturates_on_a_huge_duration() {
        // Multiplying this many days out by 86400 overflows u64 - and the
        // string is short enough to arrive in any instruction.
        let huge = match Usage::parse("184467440737095516 days") {
            Ok(u) => u,
            Err(e) => unreachable!("parse failed: {:?}", e),
        };
        assert_eq!(huge.seconds(), u64::MAX);

        // ...while a normal duration still converts exactly.
        let day = match Usage::parse("1 days") {
            Ok(u) => u,
            Err(e) => unreachable!("parse failed: {:?}", e),
        };
        assert_eq!(day.seconds(), 86400);
    }
}
