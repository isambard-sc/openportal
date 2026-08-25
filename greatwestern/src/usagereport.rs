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

/// Whether a counter is zero, for `skip_serializing_if`.
///
/// A report only states what it has to say: an empty map or a zero counter is
/// left out of the JSON entirely rather than written as `{}` or `0`, and read
/// back by the `serde(default)` on every one of these fields. It keeps a report
/// carrying one day of one project's work legible, and it is what lets a reader
/// that predates a statistic behave identically to one that has simply not seen
/// it - see `docs/plans/slurm-requeue-accounting-design.md`.
fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// Expansion factors are accumulated as thousandths, so that summing them is
/// exact and order-independent - see the field comments in
/// [`DailyProjectUsageReport`].
const EXPANSION_SCALE: u64 = 1000;

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DailyProjectUsageReport {
    // Still written even when empty, unlike every field below: release 0.92.0
    // has no `serde(default)` on `reports` or `is_complete`, so omitting them
    // would make a peer of that version fail outright rather than read a
    // default. The `default` here lets a *later* release stop writing them.
    #[serde(default)]
    reports: HashMap<String, Usage>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    components: HashMap<String, HashMap<String, Usage>>,
    /// Per-user job counts. Empty when reading data from older instances.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    user_job_counts: HashMap<String, u64>,
    /// Per-user wait seconds. Empty when reading data from older instances.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    user_wait_seconds: HashMap<String, u64>,
    /// Scalar total — equals sum of user_job_counts when populated, otherwise
    /// carries the value from older instances that lack per-user maps.
    #[serde(default, skip_serializing_if = "is_zero")]
    num_jobs: u64,
    /// Scalar total — equals sum of user_wait_seconds when populated.
    #[serde(default, skip_serializing_if = "is_zero")]
    total_wait_seconds: u64,

    // ---- Expansion factor ---------------------------------------------------
    //
    // Turnaround over runtime, per job - `(wait + run) / run`, the classical
    // definition, which is 1.0 for a job that started the instant it was
    // eligible and rises with every second spent queueing. It says how much
    // waiting a project endured for the work it got. A rising figure is worth
    // looking at: a job that queues for hours and then exits in seconds, over
    // and over, is what a user struggling to debug something looks like from the
    // outside.
    //
    // The sum of the per-job *ratios* is kept, not a ratio of sums, because the
    // two answer different questions and fail in opposite directions. The mean
    // of ratios is dominated by a short job that waited a long time, which is
    // exactly the case worth catching; a ratio of sums is dominated by whichever
    // job ran longest, which hides it. Total runtime is kept as well so both are
    // available - see `average_expansion_factor` and
    // `aggregate_expansion_factor`.
    //
    // The ratios are accumulated as thousandths rather than as floating point.
    // Float addition is not associative, so summing the same reports in a
    // different order would give a different total, and these reports are
    // merged out of `HashMap`s whose order is arbitrary; the shadow-counter
    // checks would then fail for no reason. Thousandths of an expansion factor
    // is far finer than anyone reads.
    /// Per-user sum of per-job expansion factors, in thousandths.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    user_expansion_milli: HashMap<String, u64>,
    /// Scalar total — equals sum of user_expansion_milli when populated.
    #[serde(default, skip_serializing_if = "is_zero")]
    total_expansion_milli: u64,
    /// Per-user total wall-clock runtime, in seconds. Not the same as usage,
    /// which is weighted by the fraction of a node a job held.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    user_runtime_seconds: HashMap<String, u64>,
    /// Scalar total — equals sum of user_runtime_seconds when populated.
    #[serde(default, skip_serializing_if = "is_zero")]
    total_runtime_seconds: u64,

    // ---- Job size -----------------------------------------------------------
    //
    // The cores and GPUs each job was allocated, summed, so that dividing by the
    // job count gives the mean size of a job - which says whether a project is
    // running many small jobs or a few large ones. This cannot be recovered from
    // usage: usage is core-seconds, and the same core-seconds come from one job
    // on many cores or many jobs on one core.
    //
    // Deliberately *unweighted* - each job counts once regardless of how long it
    // ran, because the question is about the shape of the jobs rather than about
    // what the machine was occupied by. The time-weighted answer to the other
    // question is roughly the `cpu` component's usage over the runtime below.
    /// Per-user sum of the cores each job was allocated.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    user_allocated_cpus: HashMap<String, u64>,
    /// Scalar total — equals sum of user_allocated_cpus when populated.
    #[serde(default, skip_serializing_if = "is_zero")]
    total_allocated_cpus: u64,
    /// Per-user sum of the GPUs each job was allocated.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    user_allocated_gpus: HashMap<String, u64>,
    /// Scalar total — equals sum of user_allocated_gpus when populated.
    #[serde(default, skip_serializing_if = "is_zero")]
    total_allocated_gpus: u64,

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
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    requeue_reports: HashMap<String, Usage>,
    /// The same, broken down by resource component.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    requeue_components: HashMap<String, HashMap<String, Usage>>,
    /// Per-user count of requeue *events* (superseded attempts, not jobs).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    user_requeue_events: HashMap<String, u64>,
    /// Scalar total — equals sum of user_requeue_events when populated.
    #[serde(default, skip_serializing_if = "is_zero")]
    num_requeue_events: u64,
    /// Per-user queue wait accumulated by superseded attempts.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    user_requeue_wait_seconds: HashMap<String, u64>,
    /// Scalar total — equals sum of user_requeue_wait_seconds when populated.
    #[serde(default, skip_serializing_if = "is_zero")]
    requeue_wait_seconds: u64,
    /// Requeue events by the terminal state of the superseded attempt. Sums to
    /// `num_requeue_events`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    requeue_states: HashMap<String, u64>,
    /// Requeue usage by the terminal state of the superseded attempt. Sums to
    /// `total_requeue_usage()`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    requeue_state_usage: HashMap<String, Usage>,

    // ---- Reservations ------------------------------------------------------
    //
    // Which reservation a job ran under, so that a reservation's occupancy can
    // be seen at all. Jobs outside a reservation - almost all of them - are not
    // recorded here, so `total_usage_including_requeues()` minus the reservation
    // total is the unreserved usage.
    //
    // These figures deliberately count *every* attempt, superseded ones
    // included: a requeued attempt held the reservation's nodes exactly as its
    // replacement did, and for occupancy that is what matters. The superseded
    // share is carried separately so the two can still be told apart.
    /// Reservation name → local user → usage consumed inside it.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    reservation_reports: HashMap<String, HashMap<String, Usage>>,
    /// Reservation name → the part of the above from superseded attempts.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    reservation_requeue_usage: HashMap<String, Usage>,
    /// Reservation name → jobs that started inside it, counted as `num_jobs` is.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    reservation_jobs: HashMap<String, u64>,

    /// See the note on `reports` above - written even when false, for now.
    #[serde(default)]
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

                if self.total_runtime_seconds() > 0 {
                    writeln!(
                        f,
                        "Expansion factor: {:.2} mean per job, {:.2} overall",
                        self.average_expansion_factor(),
                        self.aggregate_expansion_factor()
                    )?;
                }

                // A real job always holds at least one core, so no cores across
                // some jobs means the figure was never recorded - a report from
                // before job sizes were. Saying "0.0 cores" would state a
                // falsehood rather than admit a gap.
                if self.average_cpus_per_job() > 0.0 {
                    writeln!(
                        f,
                        "Mean job size: {:.1} cores, {:.1} gpus",
                        self.average_cpus_per_job(),
                        self.average_gpus_per_job()
                    )?;
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

                if report.total_runtime_seconds() > 0 {
                    writeln!(
                        f,
                        "Expansion factor: {:.2} mean per job, {:.2} overall",
                        report.average_expansion_factor(),
                        report.aggregate_expansion_factor()
                    )?;
                }

                // A real job always holds at least one core, so no cores across
                // some jobs means the figure was never recorded - a report from
                // before job sizes were. Saying "0.0 cores" would state a
                // falsehood rather than admit a gap.
                if report.average_cpus_per_job() > 0.0 {
                    writeln!(
                        f,
                        "Mean job size: {:.1} cores, {:.1} gpus",
                        report.average_cpus_per_job(),
                        report.average_gpus_per_job()
                    )?;
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

    ///
    /// Record one job's queue time and runtime, for the expansion factor.
    ///
    /// Called for the same population as `add_jobs` - one job, once, in the
    /// window it started in - so that the mean has a well-defined denominator.
    /// Both figures are properties of the job rather than of the window, so they
    /// are the job's whole wait and whole runtime even where that runs past the
    /// window's end.
    ///
    /// A job with no runtime is ignored rather than counted as an infinite
    /// expansion: `op-slurm` does not report jobs that consumed nothing, so this
    /// should not arise, but a division by zero here would be a process kill.
    ///
    pub fn add_expansion(&mut self, user: &str, wait_seconds: u64, runtime_seconds: u64) {
        if runtime_seconds == 0 {
            return;
        }

        // `(wait + run) / run` - the classical expansion factor, so a job that
        // never waited contributes exactly 1.0
        let expansion_milli = wait_seconds
            .saturating_add(runtime_seconds)
            .saturating_mul(EXPANSION_SCALE)
            .saturating_div(runtime_seconds);

        *self
            .user_expansion_milli
            .entry(user.to_string())
            .or_default() += expansion_milli;
        self.total_expansion_milli = self.total_expansion_milli.saturating_add(expansion_milli);

        *self
            .user_runtime_seconds
            .entry(user.to_string())
            .or_default() += runtime_seconds;
        self.total_runtime_seconds = self.total_runtime_seconds.saturating_add(runtime_seconds);
    }

    ///
    /// Record the size of one job: the cores and GPUs it was allocated.
    ///
    /// Called for the same population as `add_jobs`, so that dividing by the job
    /// count gives a mean over exactly the jobs counted. Each job contributes
    /// once however long it ran - the question is what shape the jobs were, not
    /// what the machine was busy with.
    ///
    pub fn add_job_size(&mut self, user: &str, cpus: u64, gpus: u64) {
        *self
            .user_allocated_cpus
            .entry(user.to_string())
            .or_default() += cpus;
        self.total_allocated_cpus = self.total_allocated_cpus.saturating_add(cpus);

        *self
            .user_allocated_gpus
            .entry(user.to_string())
            .or_default() += gpus;
        self.total_allocated_gpus = self.total_allocated_gpus.saturating_add(gpus);
    }

    /// The cores allocated to the jobs counted in this report, summed over jobs.
    /// Useful mainly as the numerator of `average_cpus_per_job`.
    pub fn total_allocated_cpus(&self) -> u64 {
        self.total_allocated_cpus
    }

    /// The GPUs allocated to the jobs counted in this report, summed over jobs.
    pub fn total_allocated_gpus(&self) -> u64 {
        self.total_allocated_gpus
    }

    ///
    /// The mean number of cores a job was allocated - many small jobs against a
    /// few large ones.
    ///
    /// Each job counts once regardless of how long it ran. This cannot be got
    /// from usage: the same core-seconds come from one job on many cores or many
    /// jobs on one core, which is the distinction being drawn here.
    ///
    pub fn average_cpus_per_job(&self) -> f64 {
        match self.num_jobs {
            0 => 0.0,
            n => self.total_allocated_cpus as f64 / n as f64,
        }
    }

    /// The mean number of GPUs a job was allocated. Zero for a project that ran
    /// no GPU work, which is itself worth knowing on a GPU machine.
    pub fn average_gpus_per_job(&self) -> f64 {
        match self.num_jobs {
            0 => 0.0,
            n => self.total_allocated_gpus as f64 / n as f64,
        }
    }

    pub fn average_cpus_per_job_for_user(&self, user: &str) -> f64 {
        match self.num_jobs_for_user(user) {
            0 => 0.0,
            n => {
                let cpus = self.user_allocated_cpus.get(user).copied().unwrap_or(0);
                cpus as f64 / n as f64
            }
        }
    }

    pub fn average_gpus_per_job_for_user(&self, user: &str) -> f64 {
        match self.num_jobs_for_user(user) {
            0 => 0.0,
            n => {
                let gpus = self.user_allocated_gpus.get(user).copied().unwrap_or(0);
                gpus as f64 / n as f64
            }
        }
    }

    pub fn allocated_cpus_for_user(&self, user: &str) -> u64 {
        self.user_allocated_cpus.get(user).copied().unwrap_or(0)
    }

    pub fn allocated_gpus_for_user(&self, user: &str) -> u64 {
        self.user_allocated_gpus.get(user).copied().unwrap_or(0)
    }

    /// Total wall-clock runtime of the jobs counted in this report. This is not
    /// usage: usage weights each second by the fraction of a node the job held,
    /// while this counts the seconds themselves.
    pub fn total_runtime_seconds(&self) -> u64 {
        self.total_runtime_seconds
    }

    /// The summed per-job expansion factors, in thousandths. Exposed so that a
    /// report spanning several days can compute one mean over every job rather
    /// than averaging each day's average.
    pub fn total_expansion_milli(&self) -> u64 {
        self.total_expansion_milli
    }

    pub fn expansion_milli_for_user(&self, user: &str) -> u64 {
        self.user_expansion_milli.get(user).copied().unwrap_or(0)
    }

    pub fn runtime_seconds_for_user(&self, user: &str) -> u64 {
        self.user_runtime_seconds.get(user).copied().unwrap_or(0)
    }

    ///
    /// The mean expansion factor of the jobs counted in this report: turnaround
    /// over runtime, `(wait + run) / run`, averaged per job.
    ///
    /// **1.0 is the ideal** - a job that ran the instant it became eligible -
    /// and the figure rises with every second spent queueing. A value of 2.0
    /// means jobs spent as long waiting as running. This is the classical
    /// definition, as used by `sreport` and the literature.
    ///
    /// **0.0 means no jobs**, not a perfect score: it is the empty-report
    /// sentinel, and no real job can score below 1.0.
    ///
    /// Being a mean of ratios, one job that queued for hours and then exited in
    /// seconds moves this a long way. That is deliberate - it is the signature
    /// of a user fighting a job that will not run - but it means a single figure
    /// should be read alongside `aggregate_expansion_factor`, which cannot be
    /// moved by one short job.
    ///
    pub fn average_expansion_factor(&self) -> f64 {
        match self.num_jobs {
            0 => 0.0,
            n => self.total_expansion_milli as f64 / (EXPANSION_SCALE as f64 * n as f64),
        }
    }

    /// The mean expansion factor for one user, on the same 1.0-is-ideal scale -
    /// which is where a struggling user shows up, the project-wide mean having
    /// averaged them away.
    pub fn expansion_factor_for_user(&self, user: &str) -> f64 {
        match self.num_jobs_for_user(user) {
            0 => 0.0,
            n => {
                let milli = self.user_expansion_milli.get(user).copied().unwrap_or(0);
                milli as f64 / (EXPANSION_SCALE as f64 * n as f64)
            }
        }
    }

    ///
    /// Total turnaround over total runtime - the whole project treated as one
    /// job. Reads on the same scale as `average_expansion_factor`: 1.0 is
    /// ideal, 0.0 means no jobs.
    ///
    /// The robust companion to `average_expansion_factor`: no single job can
    /// move it much, which also means it will not show a handful of short jobs
    /// that waited a long time. Read the two together - a mean far above the
    /// aggregate says a few short jobs waited a long time, which is usually the
    /// case worth chasing.
    ///
    pub fn aggregate_expansion_factor(&self) -> f64 {
        match self.total_runtime_seconds {
            0 => 0.0,
            runtime => self.total_wait_seconds.saturating_add(runtime) as f64 / runtime as f64,
        }
    }

    /// The local users who ran jobs counted in this report. Taken from the job
    /// counts rather than the usage map, since it is the job count that every
    /// per-job average divides by.
    pub fn job_users(&self) -> Vec<String> {
        let mut users: Vec<String> = self.user_job_counts.keys().cloned().collect();
        users.sort();
        users
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

        if jobs_sum != self.num_jobs || wait_sum != self.total_wait_seconds {
            return false;
        }

        // The expansion sums are exact integers, so these are equalities rather
        // than tolerances - which is the point of accumulating thousandths
        // instead of floats.
        if !self.user_expansion_milli.is_empty() {
            let expansion_sum: u64 = self.user_expansion_milli.values().sum();
            if expansion_sum != self.total_expansion_milli {
                return false;
            }
        }

        if !self.user_runtime_seconds.is_empty() {
            let runtime_sum: u64 = self.user_runtime_seconds.values().sum();
            if runtime_sum != self.total_runtime_seconds {
                return false;
            }
        }

        if !self.user_allocated_cpus.is_empty() {
            let cpu_sum: u64 = self.user_allocated_cpus.values().sum();
            if cpu_sum != self.total_allocated_cpus {
                return false;
            }
        }

        if !self.user_allocated_gpus.is_empty() {
            let gpu_sum: u64 = self.user_allocated_gpus.values().sum();
            if gpu_sum != self.total_allocated_gpus {
                return false;
            }
        }

        true
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

        // Reservations account for a subset of the day's consumption, not all of
        // it, so this is a bound rather than an equality - but usage inside
        // reservations exceeding everything consumed would mean a record had
        // been counted twice.
        if self.total_reservation_usage().seconds()
            > self.total_usage_including_requeues().seconds()
        {
            return false;
        }

        for (reservation, requeued) in &self.reservation_requeue_usage {
            if requeued.seconds() > self.reservation_usage(reservation).seconds() {
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

    /// True if anything about a requeue was recorded for this day.
    pub fn has_requeues(&self) -> bool {
        self.num_requeue_events > 0 || !self.total_requeue_usage().is_zero()
    }

    /// The local users who lost work to a requeue, or whose jobs were requeued.
    ///
    /// The two are not the same set: a superseded attempt's usage is recorded in
    /// every window it overlaps, while the requeue itself is recorded in the one
    /// window where it happened, so a user can appear in one map and not the
    /// other. Both are included.
    pub fn requeue_users(&self) -> Vec<String> {
        let mut users: Vec<String> = self
            .requeue_reports
            .keys()
            .chain(self.user_requeue_events.keys())
            .cloned()
            .collect();

        users.sort();
        users.dedup();
        users
    }

    ///
    /// Requeue events and usage per interrupting state, worst first.
    ///
    /// The counts and the usage come from different rules - see
    /// `add_requeue_state_usage` - so a state can appear with usage but no
    /// events, or the other way round. Every state named by either is listed.
    ///
    pub fn requeue_state_summary(&self) -> Vec<(String, u64, Usage)> {
        let mut states: Vec<String> = self
            .requeue_states
            .keys()
            .chain(self.requeue_state_usage.keys())
            .cloned()
            .collect();

        states.sort();
        states.dedup();

        let mut summary: Vec<(String, u64, Usage)> = states
            .into_iter()
            .map(|state| {
                let events = self.requeue_events_in_state(&state);
                let usage = self.requeue_usage_in_state(&state);
                (state, events, usage)
            })
            .collect();

        // worst first, by usage, with the state name breaking ties so the
        // ordering is stable
        summary.sort_by(|a, b| b.2.seconds().cmp(&a.2.seconds()).then(a.0.cmp(&b.0)));
        summary
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

    // ---- Reservations ------------------------------------------------------

    /// Record usage consumed inside a reservation. Called for every attempt,
    /// superseded ones included - see the field comments.
    pub fn add_reservation_usage(&mut self, reservation: &str, local_user: &str, usage: Usage) {
        if reservation.is_empty() || usage.is_zero() {
            return;
        }

        let reports = self
            .reservation_reports
            .entry(reservation.to_string())
            .or_default();

        *reports.entry(local_user.to_string()).or_default() += usage;
    }

    /// Record the part of a reservation's usage that came from an attempt later
    /// superseded by a requeue. This is a subset of `add_reservation_usage`, not
    /// an addition to it, so both are called for the same record.
    pub fn add_reservation_requeue_usage(&mut self, reservation: &str, usage: Usage) {
        if reservation.is_empty() || usage.is_zero() {
            return;
        }

        *self
            .reservation_requeue_usage
            .entry(reservation.to_string())
            .or_default() += usage;
    }

    pub fn add_reservation_jobs(&mut self, reservation: &str, count: u64) {
        if reservation.is_empty() {
            return;
        }

        *self
            .reservation_jobs
            .entry(reservation.to_string())
            .or_default() += count;
    }

    /// The reservations any of this day's jobs ran under, sorted by name.
    pub fn reservations(&self) -> Vec<String> {
        let mut reservations: Vec<String> = self
            .reservation_reports
            .keys()
            .chain(self.reservation_jobs.keys())
            .cloned()
            .collect();

        reservations.sort();
        reservations.dedup();
        reservations
    }

    pub fn has_reservations(&self) -> bool {
        !self.reservation_reports.is_empty() || !self.reservation_jobs.is_empty()
    }

    /// Usage consumed inside `reservation`, counting every attempt.
    pub fn reservation_usage(&self, reservation: &str) -> Usage {
        match self.reservation_reports.get(reservation) {
            Some(reports) => reports.values().cloned().sum(),
            None => Usage::default(),
        }
    }

    pub fn reservation_usage_for_user(&self, reservation: &str, local_user: &str) -> Usage {
        self.reservation_reports
            .get(reservation)
            .and_then(|reports| reports.get(local_user))
            .cloned()
            .unwrap_or_default()
    }

    /// The part of `reservation_usage` that was discarded by a requeue.
    pub fn reservation_requeue_usage(&self, reservation: &str) -> Usage {
        self.reservation_requeue_usage
            .get(reservation)
            .cloned()
            .unwrap_or_default()
    }

    pub fn reservation_jobs(&self, reservation: &str) -> u64 {
        self.reservation_jobs.get(reservation).copied().unwrap_or(0)
    }

    pub fn reservation_users(&self, reservation: &str) -> Vec<String> {
        let mut users: Vec<String> = match self.reservation_reports.get(reservation) {
            Some(reports) => reports.keys().cloned().collect(),
            None => Vec::new(),
        };

        users.sort();
        users
    }

    /// Usage consumed inside any reservation, counting every attempt.
    pub fn total_reservation_usage(&self) -> Usage {
        self.reservation_reports
            .values()
            .map(|reports| reports.values().cloned().sum::<Usage>())
            .sum()
    }

    /// Usage consumed outside any reservation. Counts every attempt, so it is
    /// the complement of `total_reservation_usage` within
    /// `total_usage_including_requeues`.
    pub fn usage_outside_reservations(&self) -> Usage {
        self.total_usage_including_requeues() - self.total_reservation_usage()
    }

    /// Jobs, usage and discarded share per reservation, busiest first.
    pub fn reservation_summary(&self) -> Vec<(String, u64, Usage, Usage)> {
        let mut summary: Vec<(String, u64, Usage, Usage)> = self
            .reservations()
            .into_iter()
            .map(|reservation| {
                let jobs = self.reservation_jobs(&reservation);
                let usage = self.reservation_usage(&reservation);
                let requeued = self.reservation_requeue_usage(&reservation);
                (reservation, jobs, usage, requeued)
            })
            .collect();

        summary.sort_by(|a, b| b.2.seconds().cmp(&a.2.seconds()).then(a.0.cmp(&b.0)));
        summary
    }

    /// Scale the usage totals - base and requeue together. The two must always
    /// be scaled by the same factor, or `total_usage()` and
    /// `total_requeue_usage()` end up in different units and the sum a client
    /// makes of them is meaningless.
    ///
    /// Job counts, wait times, runtimes, expansion factors and job sizes are
    /// deliberately untouched by every scaling operation on this type. A credit
    /// conversion rescales usage; it does not change how many jobs ran, how long
    /// they queued, how many cores they held, or a dimensionless ratio.
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
        for reports in self.reservation_reports.values_mut() {
            for usage in reports.values_mut() {
                *usage *= factor;
            }
        }
        for usage in self.reservation_requeue_usage.values_mut() {
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
        for reports in self.reservation_reports.values_mut() {
            for usage in reports.values_mut() {
                *usage /= divisor;
            }
        }
        for usage in self.reservation_requeue_usage.values_mut() {
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

        let old_expansion = std::mem::take(&mut self.user_expansion_milli);
        self.user_expansion_milli = old_expansion
            .into_iter()
            .map(|(user, milli)| {
                let new_user = string_map.get(&user).cloned().unwrap_or(user);
                (new_user, milli)
            })
            .collect();

        let old_cpus = std::mem::take(&mut self.user_allocated_cpus);
        self.user_allocated_cpus = old_cpus
            .into_iter()
            .map(|(user, cpus)| {
                let new_user = string_map.get(&user).cloned().unwrap_or(user);
                (new_user, cpus)
            })
            .collect();

        let old_gpus = std::mem::take(&mut self.user_allocated_gpus);
        self.user_allocated_gpus = old_gpus
            .into_iter()
            .map(|(user, gpus)| {
                let new_user = string_map.get(&user).cloned().unwrap_or(user);
                (new_user, gpus)
            })
            .collect();

        let old_runtimes = std::mem::take(&mut self.user_runtime_seconds);
        self.user_runtime_seconds = old_runtimes
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

        let old_reservations = std::mem::take(&mut self.reservation_reports);
        self.reservation_reports = old_reservations
            .into_iter()
            .map(|(reservation, user_map)| {
                let new_user_map = user_map
                    .into_iter()
                    .map(|(user, usage)| {
                        let new_user = string_map.get(&user).cloned().unwrap_or(user);
                        (new_user, usage)
                    })
                    .collect();
                (reservation, new_user_map)
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

        for (user, milli) in &other.user_expansion_milli {
            *new_report
                .user_expansion_milli
                .entry(user.clone())
                .or_default() += milli;
        }
        for (user, secs) in &other.user_runtime_seconds {
            *new_report
                .user_runtime_seconds
                .entry(user.clone())
                .or_default() += secs;
        }
        new_report.total_expansion_milli = self
            .total_expansion_milli
            .saturating_add(other.total_expansion_milli);
        new_report.total_runtime_seconds = self
            .total_runtime_seconds
            .saturating_add(other.total_runtime_seconds);

        for (user, cpus) in &other.user_allocated_cpus {
            *new_report
                .user_allocated_cpus
                .entry(user.clone())
                .or_default() += cpus;
        }
        for (user, gpus) in &other.user_allocated_gpus {
            *new_report
                .user_allocated_gpus
                .entry(user.clone())
                .or_default() += gpus;
        }
        new_report.total_allocated_cpus = self
            .total_allocated_cpus
            .saturating_add(other.total_allocated_cpus);
        new_report.total_allocated_gpus = self
            .total_allocated_gpus
            .saturating_add(other.total_allocated_gpus);

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

        for (reservation, reports) in other.reservation_reports {
            for (user, usage) in reports {
                new_report.add_reservation_usage(&reservation, &user, usage);
            }
        }
        for (reservation, usage) in other.reservation_requeue_usage {
            new_report.add_reservation_requeue_usage(&reservation, usage);
        }
        for (reservation, count) in &other.reservation_jobs {
            new_report.add_reservation_jobs(reservation, *count);
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

        for (user, milli) in &other.user_expansion_milli {
            *self.user_expansion_milli.entry(user.clone()).or_default() += milli;
        }
        for (user, secs) in &other.user_runtime_seconds {
            *self.user_runtime_seconds.entry(user.clone()).or_default() += secs;
        }
        self.total_expansion_milli = self
            .total_expansion_milli
            .saturating_add(other.total_expansion_milli);
        self.total_runtime_seconds = self
            .total_runtime_seconds
            .saturating_add(other.total_runtime_seconds);

        for (user, cpus) in &other.user_allocated_cpus {
            *self.user_allocated_cpus.entry(user.clone()).or_default() += cpus;
        }
        for (user, gpus) in &other.user_allocated_gpus {
            *self.user_allocated_gpus.entry(user.clone()).or_default() += gpus;
        }
        self.total_allocated_cpus = self
            .total_allocated_cpus
            .saturating_add(other.total_allocated_cpus);
        self.total_allocated_gpus = self
            .total_allocated_gpus
            .saturating_add(other.total_allocated_gpus);

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

        for (reservation, reports) in other.reservation_reports {
            for (user, usage) in reports {
                self.add_reservation_usage(&reservation, &user, usage);
            }
        }
        for (reservation, usage) in other.reservation_requeue_usage {
            self.add_reservation_requeue_usage(&reservation, usage);
        }
        for (reservation, count) in &other.reservation_jobs {
            self.add_reservation_jobs(reservation, *count);
        }

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
    /// See the note on `DailyProjectUsageReport::reports` - written even when
    /// empty, because release 0.92.0 cannot read a report without it.
    #[serde(default)]
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

            if report.total_usage() == Usage::default() && !report.has_requeues() {
                // skip days with no usage - but a day whose whole consumption
                // was discarded by requeues has plenty to say
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

                    if report.total_runtime_seconds() > 0 {
                        writeln!(
                            f,
                            "Expansion factor: {:.2} mean per job, {:.2} overall",
                            report.average_expansion_factor(),
                            report.aggregate_expansion_factor()
                        )?;
                    }

                    // A real job always holds at least one core, so no cores across
                    // some jobs means the figure was never recorded - a report from
                    // before job sizes were. Saying "0.0 cores" would state a
                    // falsehood rather than admit a gap.
                    if report.average_cpus_per_job() > 0.0 {
                        writeln!(
                            f,
                            "Mean job size: {:.1} cores, {:.1} gpus",
                            report.average_cpus_per_job(),
                            report.average_gpus_per_job()
                        )?;
                    }
                }
            }
            if report.has_requeues() {
                writeln!(
                    f,
                    "Requeued: {} {} | {} | Average requeue wait: {}",
                    report.num_requeue_events(),
                    if report.num_requeue_events() == 1 {
                        "event"
                    } else {
                        "events"
                    },
                    report.total_requeue_usage(),
                    Usage::new(report.average_requeue_wait_seconds())
                )?;
            }
            if report.has_reservations() {
                for (reservation, jobs, usage, _) in report.reservation_summary() {
                    writeln!(
                        f,
                        "Reservation {}: {} | {} {}",
                        reservation,
                        usage,
                        jobs,
                        if jobs == 1 { "job" } else { "jobs" }
                    )?;
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

                if self.total_runtime_seconds() > 0 {
                    writeln!(
                        f,
                        "Expansion factor: {:.2} mean per job, {:.2} overall",
                        self.average_expansion_factor(),
                        self.aggregate_expansion_factor()
                    )?;
                }

                // A real job always holds at least one core, so no cores across
                // some jobs means the figure was never recorded - a report from
                // before job sizes were. Saying "0.0 cores" would state a
                // falsehood rather than admit a gap.
                if self.average_cpus_per_job() > 0.0 {
                    writeln!(
                        f,
                        "Mean job size: {:.1} cores, {:.1} gpus",
                        self.average_cpus_per_job(),
                        self.average_gpus_per_job()
                    )?;
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
        if self.has_reservations() {
            writeln!(
                f,
                "In reservations: {} across {}",
                self.total_reservation_usage(),
                self.reservations().join(", ")
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

            if daily.total_usage() == Usage::default() && !daily.has_requeues() {
                // see the note in the `Display` impl above
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

                    if daily.total_runtime_seconds() > 0 {
                        writeln!(
                            f,
                            "Expansion factor: {:.2} mean per job, {:.2} overall",
                            daily.average_expansion_factor(),
                            daily.aggregate_expansion_factor()
                        )?;
                    }

                    // A real job always holds at least one core, so no cores across
                    // some jobs means the figure was never recorded - a report from
                    // before job sizes were. Saying "0.0 cores" would state a
                    // falsehood rather than admit a gap.
                    if daily.average_cpus_per_job() > 0.0 {
                        writeln!(
                            f,
                            "Mean job size: {:.1} cores, {:.1} gpus",
                            daily.average_cpus_per_job(),
                            daily.average_gpus_per_job()
                        )?;
                    }
                }
            }
            if daily.has_requeues() {
                writeln!(
                    f,
                    "Requeued: {} {} | {} | Average requeue wait: {}",
                    daily.num_requeue_events(),
                    if daily.num_requeue_events() == 1 {
                        "event"
                    } else {
                        "events"
                    },
                    daily.total_requeue_usage().in_hours(),
                    Usage::new(daily.average_requeue_wait_seconds()).in_hours()
                )?;
            }
            if daily.has_reservations() {
                for (reservation, jobs, usage, _) in daily.reservation_summary() {
                    writeln!(
                        f,
                        "Reservation {}: {} | {} {}",
                        reservation,
                        usage.in_hours(),
                        jobs,
                        if jobs == 1 { "job" } else { "jobs" }
                    )?;
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

                if report.total_runtime_seconds() > 0 {
                    writeln!(
                        f,
                        "Expansion factor: {:.2} mean per job, {:.2} overall",
                        report.average_expansion_factor(),
                        report.aggregate_expansion_factor()
                    )?;
                }

                // A real job always holds at least one core, so no cores across
                // some jobs means the figure was never recorded - a report from
                // before job sizes were. Saying "0.0 cores" would state a
                // falsehood rather than admit a gap.
                if report.average_cpus_per_job() > 0.0 {
                    writeln!(
                        f,
                        "Mean job size: {:.1} cores, {:.1} gpus",
                        report.average_cpus_per_job(),
                        report.average_gpus_per_job()
                    )?;
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
        if report.has_reservations() {
            writeln!(
                f,
                "In reservations: {} across {}",
                report.total_reservation_usage().in_hours(),
                report.reservations().join(", ")
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
            for reports in report.reservation_reports.values_mut() {
                for usage in reports.values_mut() {
                    *usage *= factor;
                }
            }
            for usage in report.reservation_requeue_usage.values_mut() {
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

    /// The mean number of cores a job was allocated, over every job in this
    /// report - many small jobs against a few large ones. Computed over all
    /// jobs, not by averaging each day's average.
    pub fn average_cpus_per_job(&self) -> f64 {
        match self.num_jobs() {
            0 => 0.0,
            n => {
                let cpus = self.reports.values().fold(0u64, |total, report| {
                    total.saturating_add(report.total_allocated_cpus())
                });

                cpus as f64 / n as f64
            }
        }
    }

    /// The mean number of GPUs a job was allocated, over every job in this
    /// report.
    pub fn average_gpus_per_job(&self) -> f64 {
        match self.num_jobs() {
            0 => 0.0,
            n => {
                let gpus = self.reports.values().fold(0u64, |total, report| {
                    total.saturating_add(report.total_allocated_gpus())
                });

                gpus as f64 / n as f64
            }
        }
    }

    /// The mean job size for one local user - which is where an outlier shows
    /// up, the project-wide mean having averaged them away.
    pub fn average_cpus_per_job_for_user(&self, user: &str) -> f64 {
        let jobs = self.reports.values().fold(0u64, |total, report| {
            total.saturating_add(report.num_jobs_for_user(user))
        });

        match jobs {
            0 => 0.0,
            n => {
                let cpus = self.reports.values().fold(0u64, |total, report| {
                    total.saturating_add(report.allocated_cpus_for_user(user))
                });

                cpus as f64 / n as f64
            }
        }
    }

    pub fn average_gpus_per_job_for_user(&self, user: &str) -> f64 {
        let jobs = self.reports.values().fold(0u64, |total, report| {
            total.saturating_add(report.num_jobs_for_user(user))
        });

        match jobs {
            0 => 0.0,
            n => {
                let gpus = self.reports.values().fold(0u64, |total, report| {
                    total.saturating_add(report.allocated_gpus_for_user(user))
                });

                gpus as f64 / n as f64
            }
        }
    }

    /// The local users who ran jobs on any day in this report.
    pub fn job_users(&self) -> Vec<String> {
        let mut users: Vec<String> = self
            .reports
            .values()
            .flat_map(|report| report.job_users())
            .collect();

        users.sort();
        users.dedup();
        users
    }

    pub fn num_jobs_for_user(&self, user: &str) -> u64 {
        self.reports.values().fold(0u64, |total, report| {
            total.saturating_add(report.num_jobs_for_user(user))
        })
    }

    pub fn wait_seconds_for_user(&self, user: &str) -> u64 {
        self.reports.values().fold(0u64, |total, report| {
            total.saturating_add(report.wait_seconds_for_user(user))
        })
    }

    /// The mean queue wait per job for one local user, over every day.
    pub fn average_wait_seconds_for_user(&self, user: &str) -> u64 {
        match self.num_jobs_for_user(user) {
            0 => 0,
            n => self.wait_seconds_for_user(user) / n,
        }
    }

    /// Total wall-clock runtime of every job in this report. Not usage - usage
    /// weights each second by the fraction of a node held.
    pub fn total_runtime_seconds(&self) -> u64 {
        self.reports.values().fold(0u64, |total, report| {
            total.saturating_add(report.total_runtime_seconds())
        })
    }

    ///
    /// The mean expansion factor across every job in this report - turnaround
    /// over runtime, `(wait + run) / run`, averaged per job. 1.0 is ideal and
    /// 0.0 means no jobs.
    ///
    /// Computed from the summed thousandths and the total job count, not by
    /// averaging each day's average: a day with four jobs would otherwise weigh
    /// as heavily as a day with four hundred.
    ///
    /// See `DailyProjectUsageReport::average_expansion_factor` for what the
    /// figure means, and for its relationship to the classical form.
    ///
    pub fn average_expansion_factor(&self) -> f64 {
        let jobs = self.num_jobs();

        match jobs {
            0 => 0.0,
            n => {
                let milli = self.reports.values().fold(0u64, |total, report| {
                    total.saturating_add(report.total_expansion_milli())
                });

                milli as f64 / (EXPANSION_SCALE as f64 * n as f64)
            }
        }
    }

    /// The mean expansion factor for one local user, across every day.
    pub fn expansion_factor_for_user(&self, user: &str) -> f64 {
        let jobs = self.reports.values().fold(0u64, |total, report| {
            total.saturating_add(report.num_jobs_for_user(user))
        });

        match jobs {
            0 => 0.0,
            n => {
                let milli = self.reports.values().fold(0u64, |total, report| {
                    total.saturating_add(report.expansion_milli_for_user(user))
                });

                milli as f64 / (EXPANSION_SCALE as f64 * n as f64)
            }
        }
    }

    /// Total turnaround over total runtime - the robust companion to
    /// `average_expansion_factor`, which no single job can move much. 1.0 is
    /// ideal, 0.0 means no jobs.
    pub fn aggregate_expansion_factor(&self) -> f64 {
        match self.total_runtime_seconds() {
            0 => 0.0,
            runtime => self.total_wait_seconds().saturating_add(runtime) as f64 / runtime as f64,
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

    /// True if anything about a requeue was recorded for any day.
    pub fn has_requeues(&self) -> bool {
        self.reports.values().any(|report| report.has_requeues())
    }

    // ---- Reservations ------------------------------------------------------

    /// True if any of this project's jobs ran inside a reservation.
    pub fn has_reservations(&self) -> bool {
        self.reports
            .values()
            .any(|report| report.has_reservations())
    }

    /// The reservations this project's jobs ran under, sorted by name.
    pub fn reservations(&self) -> Vec<String> {
        let mut reservations: Vec<String> = self
            .reports
            .values()
            .flat_map(|report| report.reservations())
            .collect();

        reservations.sort();
        reservations.dedup();
        reservations
    }

    /// Usage this project consumed inside `reservation`, counting every attempt.
    pub fn reservation_usage(&self, reservation: &str) -> Usage {
        self.reports
            .values()
            .map(|report| report.reservation_usage(reservation))
            .sum()
    }

    /// The part of `reservation_usage` that was discarded by a requeue.
    pub fn reservation_requeue_usage(&self, reservation: &str) -> Usage {
        self.reports
            .values()
            .map(|report| report.reservation_requeue_usage(reservation))
            .sum()
    }

    pub fn reservation_jobs(&self, reservation: &str) -> u64 {
        self.reports.values().fold(0u64, |total, report| {
            total.saturating_add(report.reservation_jobs(reservation))
        })
    }

    /// Usage consumed inside any reservation, counting every attempt.
    pub fn total_reservation_usage(&self) -> Usage {
        self.reports
            .values()
            .map(|report| report.total_reservation_usage())
            .sum()
    }

    /// Usage consumed outside any reservation.
    pub fn usage_outside_reservations(&self) -> Usage {
        self.total_usage_including_requeues() - self.total_reservation_usage()
    }

    /// Jobs, usage and discarded share per reservation, busiest first.
    pub fn reservation_summary(&self) -> Vec<(String, u64, Usage, Usage)> {
        let mut summary: Vec<(String, u64, Usage, Usage)> = self
            .reservations()
            .into_iter()
            .map(|reservation| {
                let jobs = self.reservation_jobs(&reservation);
                let usage = self.reservation_usage(&reservation);
                let requeued = self.reservation_requeue_usage(&reservation);
                (reservation, jobs, usage, requeued)
            })
            .collect();

        summary.sort_by(|a, b| b.2.seconds().cmp(&a.2.seconds()).then(a.0.cmp(&b.0)));
        summary
    }

    ///
    /// A readable summary of how well this project's jobs were served, and what
    /// shape they were.
    ///
    /// Both of these are distribution questions being asked of a single number,
    /// so the per-user table is the point of the report rather than a refinement
    /// of it: a project-wide mean job size of twenty cores can be four
    /// 512-core jobs beside a hundred 2-core ones, describing neither.
    ///
    pub fn expansion_factor_report(&self) -> String {
        use std::fmt::Write;

        let mut out = String::new();
        let rule = "=".repeat(72);

        // `write!` to a String cannot fail, so the results are deliberately
        // discarded rather than unwrapped - `unwrap` is denied in this crate.
        let _ = writeln!(out, "Expansion factor and job size for {}", self.project());
        let _ = writeln!(out, "{}", rule);

        if self.num_jobs() == 0 {
            let _ = writeln!(out, "No jobs recorded.");
            let _ = writeln!(out, "{}", rule);
            return out;
        }

        // A report that predates these statistics has jobs but no runtime, so
        // every figure below would come out as 0.00 - which on this scale reads
        // as "turned around instantly", the opposite of the truth. Say what is
        // actually the case instead.
        if self.total_runtime_seconds() == 0 {
            let _ = writeln!(
                out,
                "{} jobs | mean wait {}",
                self.num_jobs(),
                Usage::new(self.average_wait_seconds()).in_hours()
            );
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "No expansion factor or job size recorded - this report was produced before"
            );
            let _ = writeln!(out, "those statistics were collected.");
            let _ = writeln!(out, "{}", rule);
            return out;
        }

        let mean = self.average_expansion_factor();
        let aggregate = self.aggregate_expansion_factor();

        let _ = writeln!(
            out,
            "{} jobs | mean expansion factor {:.2} | overall {:.2}",
            self.num_jobs(),
            mean,
            aggregate
        );
        let _ = writeln!(
            out,
            "Mean wait {} | mean job size {:.1} cores, {:.1} gpus",
            Usage::new(self.average_wait_seconds()).in_hours(),
            self.average_cpus_per_job(),
            self.average_gpus_per_job()
        );
        // The mean is the sum of per-job ratios over the job count, so it is
        // exactly "how many times its own runtime the average job took to turn
        // around" - worth spelling out, because a bare ratio invites being read
        // as a percentage.
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "The average job took {:.2} times its own runtime to turn around; 1.00 would",
            mean
        );
        let _ = writeln!(out, "mean it ran the moment it became eligible.");

        // The gap between the two forms is the most useful thing in the report:
        // they are moved by opposite ends of the job-size distribution.
        //
        // Compared as excesses over 1.0, not as raw values. On this scale 1.0 is
        // "waited not at all", so all the signal is in the part above it -
        // comparing 1.02 against 1.97 as a ratio says they are similar when one
        // project waited fifty times as much as the other. Nothing is said at
        // all when both excesses are small, because then there is nothing to
        // explain.
        let mean_excess = (mean - 1.0).max(0.0);
        let aggregate_excess = (aggregate - 1.0).max(0.0);
        let worth_explaining = mean_excess.max(aggregate_excess) > 0.25;

        if worth_explaining && mean_excess > aggregate_excess * 2.0 {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "The mean is well above the overall figure, so some *short* jobs waited a"
            );
            let _ = writeln!(
                out,
                "long time - a pattern worth chasing, and often a user fighting a job that"
            );
            let _ = writeln!(out, "will not run. The per-user table below says who.");
        } else if worth_explaining && aggregate_excess > mean_excess * 2.0 {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "The overall figure is well above the mean, so the waiting fell on the"
            );
            let _ = writeln!(
                out,
                "*long* jobs - which is usually queue contention rather than anything wrong."
            );
        }

        // ---- per user, worst-served first
        let mut local_to_portal = HashMap::new();

        for (user, local_user) in &self.users {
            local_to_portal.insert(local_user.clone(), user.clone());
        }

        let mut users: Vec<(String, u64, f64, f64, f64, u64)> = self
            .job_users()
            .into_iter()
            .map(|user| {
                let jobs = self.num_jobs_for_user(&user);
                let expansion = self.expansion_factor_for_user(&user);
                let cpus = self.average_cpus_per_job_for_user(&user);
                let gpus = self.average_gpus_per_job_for_user(&user);
                let wait = self.average_wait_seconds_for_user(&user);
                (user, jobs, expansion, cpus, gpus, wait)
            })
            .collect();

        users.sort_by(|a, b| {
            b.2.partial_cmp(&a.2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });

        // Merging a legacy day with a recent one can leave a row with no
        // expansion or size data. Zero is this scale's "not recorded" sentinel,
        // so show it as one rather than as a score of nought. A GPU count of
        // zero is a real answer and is printed as it is.
        let or_dash = |value: f64, places: usize| match value > 0.0 {
            true => format!("{:.*}", places, value),
            false => "-".to_string(),
        };

        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  {:<28} {:>6} {:>10} {:>8} {:>7} {:>14}",
            "user", "jobs", "expansion", "cores", "gpus", "mean wait"
        );

        for (user, jobs, expansion, cpus, gpus, wait) in users {
            let label = match local_to_portal.get(&user) {
                Some(portal_user) => portal_user.to_string(),
                None => format!("{} - unknown", user),
            };

            let _ = writeln!(
                out,
                "  {:<28} {:>6} {:>10} {:>8} {:>7} {:>14}",
                label,
                jobs,
                or_dash(expansion, 2),
                or_dash(cpus, 1),
                format!("{:.1}", gpus),
                Usage::new(wait).in_hours().to_string()
            );
        }

        // ---- per day, so a change over time is visible
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  {:<28} {:>6} {:>10} {:>8} {:>7}",
            "day", "jobs", "expansion", "cores", "gpus"
        );

        for date in self.dates() {
            let Some(report) = self.reports.get(&date) else {
                continue;
            };

            if report.num_jobs() == 0 {
                continue;
            }

            let _ = writeln!(
                out,
                "  {:<28} {:>6} {:>10} {:>8} {:>7}",
                date.to_string(),
                report.num_jobs(),
                or_dash(report.average_expansion_factor(), 2),
                or_dash(report.average_cpus_per_job(), 1),
                format!("{:.1}", report.average_gpus_per_job())
            );
        }

        let _ = writeln!(out, "{}", rule);
        let _ = writeln!(
            out,
            "Expansion factor is turnaround over runtime, so 1.00 is ideal and higher is"
        );
        let _ = writeln!(
            out,
            "worse. Job sizes count each job once however long it ran, so they describe"
        );
        let _ = writeln!(
            out,
            "the shape of the jobs, not what the machine was busy with."
        );

        out
    }

    ///
    /// A readable summary of what this project ran inside reservations.
    ///
    /// This answers "what did this project put into each reservation", which is
    /// the half of reservation utilisation a usage report can answer. The other
    /// half - what the reservation *held* - is a property of the reservation
    /// rather than of any project, and no per-project report can supply it: a
    /// reservation may be shared by several projects, and its capacity comes
    /// from its node count and duration, which the job records do not carry. So
    /// the shares below are shares of this project's own consumption, and are
    /// deliberately not called utilisation.
    ///
    pub fn reservation_report(&self) -> String {
        use std::fmt::Write;

        let mut out = String::new();
        let rule = "=".repeat(64);

        // `write!` to a String cannot fail, so the results are deliberately
        // discarded rather than unwrapped - `unwrap` is denied in this crate.
        let _ = writeln!(out, "Reservation summary for {}", self.project());
        let _ = writeln!(out, "{}", rule);

        if !self.has_reservations() {
            let _ = writeln!(out, "No jobs ran inside a reservation.");
            let _ = writeln!(out, "{}", rule);
            return out;
        }

        let truth = self.total_usage_including_requeues();
        let reserved = self.total_reservation_usage();

        let percent = |part: &Usage| match truth.seconds() {
            0 => 0.0,
            total => 100.0 * part.seconds() as f64 / total as f64,
        };

        let _ = writeln!(
            out,
            "Consumed inside reservations  : {:>14}  ({:.1}% of this project)",
            reserved.in_hours().to_string(),
            percent(&reserved)
        );
        let _ = writeln!(
            out,
            "Consumed outside reservations : {:>14}",
            self.usage_outside_reservations().in_hours().to_string()
        );
        let _ = writeln!(
            out,
            "True consumption              : {:>14}",
            truth.in_hours().to_string()
        );

        let _ = writeln!(out);
        let _ = writeln!(out, "By reservation:");

        for (reservation, jobs, usage, requeued) in self.reservation_summary() {
            let _ = writeln!(
                out,
                "  {:<24} {:>5} {:<5} {:>14}  ({:.1}%)",
                reservation,
                jobs,
                if jobs == 1 { "job" } else { "jobs" },
                usage.in_hours().to_string(),
                percent(&usage)
            );

            if !requeued.is_zero() {
                let _ = writeln!(
                    out,
                    "  {:<24} {:>5} {:<5} {:>14}   of which discarded by requeues",
                    "",
                    "",
                    "",
                    requeued.in_hours().to_string()
                );
            }
        }

        // ---- per day, then per user within each reservation
        let _ = writeln!(out);
        let _ = writeln!(out, "By day:");

        for date in self.dates() {
            let Some(report) = self.reports.get(&date) else {
                continue;
            };

            if !report.has_reservations() {
                continue;
            }

            for (reservation, jobs, usage, _) in report.reservation_summary() {
                let _ = writeln!(
                    out,
                    "  {:<12} {:<24} {:>5} {:<5} {:>14}",
                    date.to_string(),
                    reservation,
                    jobs,
                    if jobs == 1 { "job" } else { "jobs" },
                    usage.in_hours().to_string()
                );
            }
        }

        let mut local_to_portal = HashMap::new();

        for (user, local_user) in &self.users {
            local_to_portal.insert(local_user.clone(), user.clone());
        }

        let _ = writeln!(out);
        let _ = writeln!(out, "By user:");

        for reservation in self.reservations() {
            let mut users: Vec<String> = self
                .reports
                .values()
                .flat_map(|report| report.reservation_users(&reservation))
                .collect();

            users.sort();
            users.dedup();

            for user in users {
                let usage: Usage = self
                    .reports
                    .values()
                    .map(|report| report.reservation_usage_for_user(&reservation, &user))
                    .sum();

                let label = match local_to_portal.get(&user) {
                    Some(portal_user) => portal_user.to_string(),
                    None => format!("{} - unknown", user),
                };

                let _ = writeln!(
                    out,
                    "  {:<24} {:<24} {:>14}",
                    reservation,
                    label,
                    usage.in_hours().to_string()
                );
            }
        }

        let _ = writeln!(out, "{}", rule);
        let _ = writeln!(
            out,
            "Shares are of this project's own consumption. What each reservation held -"
        );
        let _ = writeln!(
            out,
            "and so how fully it was used - is a property of the reservation, not of any"
        );
        let _ = writeln!(out, "one project, and is not available from these records.");

        out
    }

    /// Requeue events and usage per interrupting state, worst first - see
    /// `DailyProjectUsageReport::requeue_state_summary`.
    pub fn requeue_state_summary(&self) -> Vec<(String, u64, Usage)> {
        let mut events: HashMap<String, u64> = HashMap::new();
        let mut usage: HashMap<String, Usage> = HashMap::new();

        for report in self.reports.values() {
            for (state, state_events, state_usage) in report.requeue_state_summary() {
                *events.entry(state.clone()).or_default() += state_events;
                *usage.entry(state).or_default() += state_usage;
            }
        }

        let mut summary: Vec<(String, u64, Usage)> = events
            .keys()
            .chain(usage.keys())
            .cloned()
            .collect::<std::collections::BTreeSet<String>>()
            .into_iter()
            .map(|state| {
                let state_events = events.get(&state).copied().unwrap_or(0);
                let state_usage = usage.get(&state).cloned().unwrap_or_default();
                (state, state_events, state_usage)
            })
            .collect();

        summary.sort_by(|a, b| b.2.seconds().cmp(&a.2.seconds()).then(a.0.cmp(&b.0)));
        summary
    }

    ///
    /// A readable summary of everything this report knows about requeues.
    ///
    /// The figures a charging decision needs, in one place: what was reported,
    /// what was discarded, what Slurm thinks the true total is, and - because
    /// this is usually the question that matters - which states did the
    /// interrupting, since work lost to a node failure is the site's doing and
    /// work lost to preemption is the site's policy.
    ///
    /// Everything is in hours, so the columns are comparable at a glance.
    /// `Usage`'s own formatting rescales itself per value, which is right for a
    /// single figure and unreadable in a table.
    ///
    pub fn requeue_report(&self) -> String {
        use std::fmt::Write;

        let mut out = String::new();
        let rule = "=".repeat(64);

        // `write!` to a String cannot fail, so the results are deliberately
        // discarded rather than unwrapped - `unwrap` is denied in this crate.
        let _ = writeln!(out, "Requeue summary for {}", self.project());
        let _ = writeln!(out, "{}", rule);

        if !self.has_requeues() {
            let _ = writeln!(out, "No requeued jobs recorded.");
            let _ = writeln!(out, "{}", rule);
            return out;
        }

        let reported = self.total_usage();
        let discarded = self.total_requeue_usage();
        let truth = self.total_usage_including_requeues();

        let percent = |part: &Usage| match truth.seconds() {
            0 => 0.0,
            total => 100.0 * part.seconds() as f64 / total as f64,
        };

        let _ = writeln!(
            out,
            "Reported usage (final attempt of each job) : {:>14}",
            reported.in_hours().to_string()
        );
        let _ = writeln!(
            out,
            "Discarded by requeues                      : {:>14}  ({:.1}%)",
            discarded.in_hours().to_string(),
            percent(&discarded)
        );
        let _ = writeln!(
            out,
            "True consumption (Slurm's view)            : {:>14}",
            truth.in_hours().to_string()
        );
        let _ = writeln!(out);

        let events = self.num_requeue_events();
        let _ = writeln!(
            out,
            "{} requeue {} | queue wait discarded: {} in total, {} per requeue",
            events,
            if events == 1 { "event" } else { "events" },
            Usage::new(self.requeue_wait_seconds()).in_hours(),
            Usage::new(self.average_requeue_wait_seconds()).in_hours()
        );

        // ---- by interrupting state
        let _ = writeln!(out);
        let _ = writeln!(out, "Work was interrupted by:");

        for (state, state_events, state_usage) in self.requeue_state_summary() {
            let _ = writeln!(
                out,
                "  {:<16} {:>4} {:<7} {:>14}  ({:.1}%)",
                state,
                state_events,
                if state_events == 1 { "event" } else { "events" },
                state_usage.in_hours().to_string(),
                percent(&state_usage)
            );
        }

        // ---- by day
        let _ = writeln!(out);
        let _ = writeln!(out, "By day:");

        for date in self.dates() {
            let Some(report) = self.reports.get(&date) else {
                continue;
            };

            if !report.has_requeues() {
                continue;
            }

            let day_events = report.num_requeue_events();
            let _ = writeln!(
                out,
                "  {:<16} {:>4} {:<7} {:>14}  (reported {})",
                date.to_string(),
                day_events,
                if day_events == 1 { "event" } else { "events" },
                report.total_requeue_usage().in_hours().to_string(),
                report.total_usage().in_hours()
            );
        }

        // ---- by user, labelled with the portal identifier where we have one
        let mut local_to_portal = HashMap::new();

        for (user, local_user) in &self.users {
            local_to_portal.insert(local_user.clone(), user.clone());
        }

        let mut users: Vec<String> = Vec::new();

        for report in self.reports.values() {
            users.extend(report.requeue_users());
        }

        users.sort();
        users.dedup();

        let _ = writeln!(out);
        let _ = writeln!(out, "By user:");

        for user in users {
            let mut user_events = 0u64;
            let mut user_usage = Usage::default();

            for report in self.reports.values() {
                user_events = user_events.saturating_add(report.requeue_events_for_user(&user));
                user_usage += report.requeue_usage(&user);
            }

            let label = match local_to_portal.get(&user) {
                Some(portal_user) => portal_user.to_string(),
                None => format!("{} - unknown", user),
            };

            let _ = writeln!(
                out,
                "  {:<24} {:>4} {:<7} {:>14}",
                label,
                user_events,
                if user_events == 1 { "event" } else { "events" },
                user_usage.in_hours().to_string()
            );
        }

        let _ = writeln!(out, "{}", rule);

        out
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
                if with_usage_only
                    && report.total_usage() == Usage::default()
                    && !report.has_requeues()
                {
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

    /// True if anything about a requeue was recorded for any project.
    pub fn has_requeues(&self) -> bool {
        self.reports.values().any(|report| report.has_requeues())
    }

    /// True if any project's jobs ran inside a reservation.
    pub fn has_reservations(&self) -> bool {
        self.reports
            .values()
            .any(|report| report.has_reservations())
    }

    /// Usage consumed inside any reservation, across every project.
    pub fn total_reservation_usage(&self) -> Usage {
        self.reports
            .values()
            .map(|report| report.total_reservation_usage())
            .sum()
    }

    ///
    ///
    /// A readable expansion-factor and job-size summary for every project that
    /// ran any jobs - see `ProjectUsageReport::expansion_factor_report`.
    ///
    pub fn expansion_factor_report(&self) -> String {
        use std::fmt::Write;

        let mut projects: Vec<&ProjectIdentifier> = self.reports.keys().collect();
        projects.sort_by_cached_key(|project| project.to_string());

        let mut out = String::new();

        for project in projects {
            let Some(report) = self.reports.get(project) else {
                continue;
            };

            if report.num_jobs() == 0 {
                continue;
            }

            // `write!` to a String cannot fail
            let _ = write!(out, "{}", report.expansion_factor_report());
        }

        if out.is_empty() {
            let _ = writeln!(out, "No jobs recorded for any project.");
        }

        out
    }

    /// A readable reservation summary for every project that ran inside one.
    ///
    /// Note what this is not: a reservation's own utilisation. A reservation is
    /// usually shared between projects, so even summed over a portal these are
    /// the shares each project contributed, not how full the reservation was -
    /// see `ProjectUsageReport::reservation_report`.
    ///
    pub fn reservation_report(&self) -> String {
        use std::fmt::Write;

        let mut projects: Vec<&ProjectIdentifier> = self.reports.keys().collect();
        projects.sort_by_cached_key(|project| project.to_string());

        let mut out = String::new();

        for project in projects {
            let Some(report) = self.reports.get(project) else {
                continue;
            };

            if !report.has_reservations() {
                continue;
            }

            // `write!` to a String cannot fail
            let _ = write!(out, "{}", report.reservation_report());
        }

        if out.is_empty() {
            let _ = writeln!(out, "No jobs ran inside a reservation for any project.");
        }

        out
    }

    /// A readable requeue summary for every project that has one - see
    /// `ProjectUsageReport::requeue_report`. Projects with no requeues are left
    /// out rather than listed as empty, since on a real portal they are the
    /// overwhelming majority.
    pub fn requeue_report(&self) -> String {
        use std::fmt::Write;

        let mut projects: Vec<&ProjectIdentifier> = self.reports.keys().collect();
        projects.sort_by_cached_key(|project| project.to_string());

        let mut out = String::new();

        for project in projects {
            let Some(report) = self.reports.get(project) else {
                continue;
            };

            if !report.has_requeues() {
                continue;
            }

            // `write!` to a String cannot fail
            let _ = write!(out, "{}", report.requeue_report());
        }

        if out.is_empty() {
            let _ = writeln!(out, "No requeued jobs recorded for any project.");
        }

        out
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

    ///
    /// A real month of one project's usage, as `op-slurm` produced it, with the
    /// project and usernames anonymised and nothing else touched.
    ///
    /// Synthetic reports can only check that the arithmetic agrees with itself.
    /// This one checks it against numbers a cluster actually generated - 24
    /// days, 357 jobs, eight users of very different habits, requeues in both
    /// states, an `interactive` reservation appearing on some days and not
    /// others, and one day whose reports are still incomplete.
    ///
    fn real_month() -> ProjectUsageReport {
        ProjectUsageReport::from_json(include_str!("../tests/data/project-usage-report.json"))
            .unwrap()
    }

    #[test]
    fn test_a_real_month_of_data_is_internally_consistent() {
        let report = real_month();

        assert_eq!(report.dates().len(), 24);

        // every day's per-user maps sum to its own scalars, its per-state maps
        // account for every requeue event and second, and no reservation claims
        // more than the day consumed
        for date in report.dates() {
            let daily = report.get_report(&date);
            assert!(
                daily.daily_reports(false).iter().all(|d| d.is_consistent()),
                "inconsistent report for {}",
                date
            );
        }

        // and the month's totals are what the days add up to
        assert_eq!(report.total_usage(), Usage::new(13_390_528));
        assert_eq!(report.total_requeue_usage(), Usage::new(5_547_234));
        assert_eq!(
            report.total_usage_including_requeues(),
            Usage::new(13_390_528 + 5_547_234)
        );
        assert_eq!(report.num_jobs(), 357);
        assert_eq!(report.total_wait_seconds(), 5_287_100);
        assert_eq!(report.num_requeue_events(), 12);
        assert_eq!(report.requeue_wait_seconds(), 932_011);
        assert_eq!(report.total_runtime_seconds(), 2_066_571);
        assert_eq!(report.total_reservation_usage(), Usage::new(254_220));
        assert_eq!(report.reservation_jobs("interactive"), 82);

        // requeues in both states, worst first
        assert_eq!(
            report.requeue_state_summary(),
            vec![
                ("REQUEUED".to_string(), 9, Usage::new(4_366_354)),
                ("NODE_FAIL".to_string(), 3, Usage::new(1_180_880)),
            ]
        );
    }

    #[test]
    fn test_splitting_a_month_into_days_and_recombining_gives_the_same_totals() {
        // Every figure in a report has to be additive over days, or a monthly
        // report and the same month summed from its days would disagree - and
        // `op-slurm` builds a month exactly by summing days.
        let report = real_month();

        let mut recombined = DailyProjectUsageReport::default();

        for date in report.dates() {
            let daily = report.get_report(&date);
            for day in daily.daily_reports(false) {
                recombined += day;
            }
        }

        assert_eq!(recombined.total_usage(), report.total_usage());
        assert_eq!(
            recombined.total_requeue_usage(),
            report.total_requeue_usage()
        );
        assert_eq!(recombined.num_jobs(), report.num_jobs());
        assert_eq!(recombined.total_wait_seconds(), report.total_wait_seconds());
        assert_eq!(recombined.num_requeue_events(), report.num_requeue_events());
        assert_eq!(
            recombined.requeue_wait_seconds(),
            report.requeue_wait_seconds()
        );
        assert_eq!(
            recombined.total_runtime_seconds(),
            report.total_runtime_seconds()
        );
        assert_eq!(
            recombined.total_reservation_usage(),
            report.total_reservation_usage()
        );
        assert_eq!(recombined.reservation_jobs("interactive"), 82);
        assert_eq!(
            recombined.requeue_state_summary(),
            report.requeue_state_summary()
        );

        // the component breakdowns too
        for component in ["cpu", "gpu", "memory", "billing"] {
            assert_eq!(
                recombined.total_requeue_component_usage(component),
                report.get_component(component).total_requeue_usage(),
                "component {} disagrees",
                component
            );
        }

        // and the recombined day is still internally consistent
        assert!(recombined.is_consistent());
    }

    #[test]
    fn test_the_derived_ratios_survive_recombination() {
        // The ratios are the part that could plausibly not survive, since each
        // is a quotient of two sums - so they have to be computed from the sums
        // rather than from other ratios.
        let report = real_month();

        let mut recombined = DailyProjectUsageReport::default();
        for date in report.dates() {
            for day in report.get_report(&date).daily_reports(false) {
                recombined += day;
            }
        }

        assert_eq!(
            recombined.average_expansion_factor(),
            report.average_expansion_factor()
        );
        assert_eq!(
            recombined.aggregate_expansion_factor(),
            report.aggregate_expansion_factor()
        );
        assert_eq!(
            recombined.average_cpus_per_job(),
            report.average_cpus_per_job()
        );
        assert_eq!(
            recombined.average_gpus_per_job(),
            report.average_gpus_per_job()
        );
        assert_eq!(
            recombined.average_requeue_wait_seconds(),
            report.average_requeue_wait_seconds()
        );

        // the real figures, for the record: jobs on this cluster waited a great
        // deal longer than they ran
        assert!((report.average_expansion_factor() - 2_150.719_031).abs() < 1e-5);
        assert!((report.aggregate_expansion_factor() - 3.558_393).abs() < 1e-5);
        assert!((report.average_cpus_per_job() - 1_062.655_462).abs() < 1e-5);

        // and per user, which is where the answer actually is
        assert!((report.expansion_factor_for_user("user1.project") - 7_290.137).abs() < 0.01);
        assert!((report.expansion_factor_for_user("user8.project") - 1.002).abs() < 0.01);
        assert_eq!(report.num_jobs_for_user("user2.project"), 214);
    }

    #[test]
    fn test_a_project_mean_is_not_the_mean_of_daily_means() {
        // Real data proving why the project figure is computed over every job
        // rather than by averaging each day's average: on this month the two
        // differ by more than two hundred.
        let report = real_month();

        let daily_means: Vec<f64> = report
            .dates()
            .iter()
            .flat_map(|date| report.get_report(date).daily_reports(false))
            .filter(|day| day.num_jobs() > 0)
            .map(|day| day.average_expansion_factor())
            .collect();

        let mean_of_means = daily_means.iter().sum::<f64>() / daily_means.len() as f64;

        assert!((mean_of_means - 1_945.591_234).abs() < 1e-5);
        assert!(
            (report.average_expansion_factor() - mean_of_means).abs() > 200.0,
            "the two must differ, or the test proves nothing"
        );
    }

    #[test]
    fn test_filtering_a_month_into_halves_partitions_every_total() {
        // The same additivity from the other direction - a date range carved out
        // of a report and its complement have to add back up to it.
        let report = real_month();

        let first = report.filter(&DateRange::parse("2026-08-01:2026-08-12").unwrap());
        let second = report.filter(&DateRange::parse("2026-08-13:2026-08-24").unwrap());

        assert_eq!(first.dates().len(), 12);
        assert_eq!(second.dates().len(), 12);

        assert_eq!(
            first.total_usage() + second.total_usage(),
            report.total_usage()
        );
        assert_eq!(
            first.total_requeue_usage() + second.total_requeue_usage(),
            report.total_requeue_usage()
        );
        assert_eq!(first.num_jobs() + second.num_jobs(), report.num_jobs());
        assert_eq!(
            first.num_requeue_events() + second.num_requeue_events(),
            report.num_requeue_events()
        );
        assert_eq!(
            first.total_runtime_seconds() + second.total_runtime_seconds(),
            report.total_runtime_seconds()
        );
        assert_eq!(
            first.total_reservation_usage() + second.total_reservation_usage(),
            report.total_reservation_usage()
        );

        // every requeue in this month happened in the first half, and every
        // reservation figure is split across both - so neither half is trivial
        assert_eq!(first.num_requeue_events(), 12);
        assert!(first.has_reservations() && second.has_reservations());
    }

    #[test]
    fn test_the_real_month_round_trips_through_the_minimal_json() {
        // A report only writes what it has to say, so most days omit most
        // fields. Reading one back has to give exactly what was written.
        let report = real_month();
        let json = report.to_json().unwrap();

        // the empty maps this month is full of are not written at all
        assert!(
            !json.contains("\"reservation_jobs\":{}"),
            "empty maps remain"
        );
        assert!(!json.contains("\"requeue_states\":{}"), "empty maps remain");
        assert!(
            !json.contains("\"requeue_wait_seconds\":0"),
            "zero counters remain"
        );

        let reparsed = ProjectUsageReport::from_json(&json).unwrap();

        assert_eq!(reparsed.total_usage(), report.total_usage());
        assert_eq!(reparsed.total_requeue_usage(), report.total_requeue_usage());
        assert_eq!(reparsed.num_jobs(), report.num_jobs());
        assert_eq!(reparsed.total_wait_seconds(), report.total_wait_seconds());
        assert_eq!(reparsed.num_requeue_events(), report.num_requeue_events());
        assert_eq!(
            reparsed.total_runtime_seconds(),
            report.total_runtime_seconds()
        );
        assert_eq!(
            reparsed.total_reservation_usage(),
            report.total_reservation_usage()
        );
        assert_eq!(
            reparsed.average_expansion_factor(),
            report.average_expansion_factor()
        );
        assert_eq!(
            reparsed.average_cpus_per_job(),
            report.average_cpus_per_job()
        );
        assert_eq!(
            reparsed.requeue_state_summary(),
            report.requeue_state_summary()
        );
        assert_eq!(reparsed.dates(), report.dates());

        // and writing it again gives the same document - compared as JSON rather
        // than as bytes, because these are `HashMap`s and serde emits their keys
        // in whatever order the map iterates
        let first: serde_json::Value = serde_json::from_str(&json).unwrap();
        let again: serde_json::Value = serde_json::from_str(&reparsed.to_json().unwrap()).unwrap();

        assert_eq!(first, again);
    }

    #[test]
    fn test_scaling_a_month_agrees_with_scaling_its_days() {
        // `Usage` truncates to whole seconds, so scaling and summing are not in
        // general interchangeable - but they are here, and for a reason worth
        // recording: usage is only ever *stored* per user per day, and both
        // paths scale those same stored values. Nothing is scaled after being
        // summed, so there is no coarser value to lose a fraction of.
        //
        // A caller converting a month to credits and a caller converting each
        // day therefore agree to the second, which is what makes a monthly
        // invoice reconcilable against a daily breakdown.
        let report = real_month();

        let mut scaled_whole = report.clone();
        scaled_whole.scale_total(0.5);

        let mut scaled_days = DailyProjectUsageReport::default();
        for date in report.dates() {
            for day in report.get_report(&date).daily_reports(false) {
                scaled_days += day / 2.0;
            }
        }

        assert_eq!(
            scaled_whole.total_usage().seconds(),
            scaled_days.total_usage().seconds()
        );

        // Halving really did something, so the equality above is not vacuous.
        // Doubling does not recover the original: each odd per-user-day value
        // lost half a second, thirty-two of them across this month. That is the
        // truncation - it just falls in the same place either way round.
        let halved = scaled_whole.total_usage().seconds();
        assert!(halved * 2 <= report.total_usage().seconds());
        assert!(
            report.total_usage().seconds() - halved * 2 < 100,
            "the loss is bounded by one second per stored value"
        );
    }

    #[test]
    fn test_the_quick_reports_read_sensibly_over_real_data() {
        let report = real_month();

        let requeues = report.requeue_report();
        assert!(requeues.contains("NODE_FAIL"), "{}", requeues);
        assert!(requeues.contains("REQUEUED"), "{}", requeues);
        assert!(requeues.contains("12 requeue events"), "{}", requeues);

        let reservations = report.reservation_report();
        assert!(reservations.contains("interactive"), "{}", reservations);
        assert!(reservations.contains("82 jobs"), "{}", reservations);

        let expansion = report.expansion_factor_report();
        assert!(expansion.contains("357 jobs"), "{}", expansion);
        // the mean is far above the overall figure on this month, so the report
        // should say which end of the distribution waited
        assert!(expansion.contains("*short* jobs"), "{}", expansion);
        // and the worst-served user is named first
        // "mean wait" in lower case appears only in the column header, so the
        // row after it is the worst-served user
        let first_row = expansion
            .lines()
            .skip_while(|line| !line.contains("mean wait"))
            .nth(1);
        let Some(first_row) = first_row else {
            unreachable!("no user rows in:\n{}", expansion);
        };
        assert!(first_row.contains("user1.project"), "{}", first_row);
    }

    #[test]
    fn test_an_absent_is_complete_reads_as_incomplete() {
        // `is_complete` is one of the three fields still written even when it
        // has nothing to say, because release 0.92.0 cannot read a report
        // without it. It now carries a `serde(default)` so that a later release
        // can stop writing it - and the default has to be the safe direction.
        //
        // `false` is that direction: a report that does not say it is finished
        // is treated as still being filled in. `op-slurm` refuses to cache an
        // incomplete day and will fetch it again, and the printout marks it as
        // incomplete, so the cost of guessing wrong is a repeated query. The
        // other way round, a partial day would be cached as final and a
        // project's usage silently understated for ever.
        let without: DailyProjectUsageReport = serde_json::from_value(serde_json::json!({
            "reports": { "alice": { "seconds": 3600 } }
        }))
        .unwrap();

        assert!(!without.is_complete());
        assert_eq!(without.total_usage(), Usage::new(3600));
        assert!(without.to_string().contains("incomplete"));

        // and a report that does say so is believed
        let with: DailyProjectUsageReport = serde_json::from_value(serde_json::json!({
            "reports": { "alice": { "seconds": 3600 } },
            "is_complete": true
        }))
        .unwrap();

        assert!(with.is_complete());
        assert!(!with.to_string().contains("incomplete"));

        // the real month has one day still being filled in, and it reads as such
        let month = real_month();
        let incomplete: Vec<Date> = month
            .dates()
            .into_iter()
            .filter(|date| {
                !month
                    .get_report(date)
                    .daily_reports(false)
                    .iter()
                    .all(|day| day.is_complete())
            })
            .collect();

        assert_eq!(incomplete.len(), 1);
        assert_eq!(
            incomplete.first().map(|d| d.to_string()),
            Some("2026-08-24".to_string())
        );
    }

    #[test]
    fn test_a_report_from_the_previous_release_still_reads_correctly() {
        // Exactly what release 0.92.0 serialised - the fields it had, and none
        // of the ones added since. Every new figure has to read as "not
        // recorded" rather than as a value, and nothing it used to say may have
        // changed.
        let legacy = serde_json::json!({
            "project": "proj.portal",
            "users": { "alice.proj.portal": "alice" },
            "reports": {
                "2026-03-01": {
                    "reports": { "alice": { "seconds": 7200 } },
                    "components": {
                        "cpu": { "alice": { "seconds": 921600 } },
                        "gpu": { "alice": { "seconds": 28800 } }
                    },
                    "user_job_counts": { "alice": 4 },
                    "user_wait_seconds": { "alice": 3600 },
                    "num_jobs": 4,
                    "total_wait_seconds": 3600,
                    "is_complete": true
                }
            }
        });

        let report: ProjectUsageReport = serde_json::from_value(legacy).unwrap();

        // everything it used to say, unchanged
        assert_eq!(report.total_usage(), Usage::new(7200));
        assert_eq!(report.num_jobs(), 4);
        assert_eq!(report.total_wait_seconds(), 3600);
        assert_eq!(report.average_wait_seconds(), 900);
        assert_eq!(
            report.components(),
            vec!["cpu".to_string(), "gpu".to_string()]
        );
        assert_eq!(
            report.get_component("cpu").total_usage(),
            Usage::new(921600)
        );

        // every new figure reads as "nothing recorded"
        assert_eq!(report.total_requeue_usage(), Usage::default());
        assert_eq!(report.total_usage_including_requeues(), Usage::new(7200));
        assert_eq!(report.num_requeue_events(), 0);
        assert!(!report.has_requeues());
        assert!(!report.has_reservations());
        assert_eq!(report.usage_outside_reservations(), Usage::new(7200));
        assert_eq!(report.total_runtime_seconds(), 0);
        assert_eq!(report.average_expansion_factor(), 0.0);
        assert_eq!(report.average_cpus_per_job(), 0.0);

        // and the printed output must not turn those absences into claims. A
        // legacy report's jobs did not run on zero cores, and they did not turn
        // around instantly - on the classical scale 0.00 would read as better
        // than perfect.
        let printed = report.to_string();
        assert!(!printed.contains("Mean job size"), "{}", printed);
        assert!(!printed.contains("Expansion factor"), "{}", printed);

        let dump = report.expansion_factor_report();
        assert!(
            dump.contains("No expansion factor or job size recorded"),
            "{}",
            dump
        );
        assert!(!dump.contains("0.00"), "{}", dump);

        // the other quick reports say so in words rather than printing zeroes
        assert!(report.requeue_report().contains("No requeued jobs"));
        assert!(report.reservation_report().contains("No jobs ran inside"));
    }

    #[test]
    fn test_what_we_emit_now_still_loads_into_a_reader_that_knows_only_the_old_fields() {
        // The other direction: an older peer deserialising one of our reports
        // ignores what it does not know, so nothing on the wire had to change.
        #[derive(serde::Deserialize)]
        struct OldDaily {
            reports: HashMap<String, Usage>,
            #[serde(default)]
            num_jobs: u64,
            #[serde(default)]
            total_wait_seconds: u64,
            is_complete: bool,
        }

        let mut modern = DailyProjectUsageReport::default();
        modern.add_usage("alice", Usage::new(3600));
        modern.add_jobs("alice", 1);
        modern.add_wait_seconds("alice", 60);
        modern.add_expansion("alice", 60, 3600);
        modern.add_job_size("alice", 128, 4);
        modern.add_requeue_usage("alice", Usage::new(600));
        modern.add_requeue_events("alice", "NODE_FAIL", 1);
        modern.add_reservation_usage("bench", "alice", Usage::new(1200));
        modern.set_complete();

        let old: OldDaily = serde_json::from_str(&serde_json::to_string(&modern).unwrap()).unwrap();

        assert_eq!(old.num_jobs, 1);
        assert_eq!(old.total_wait_seconds, 60);
        assert_eq!(
            old.reports.get("alice").cloned().unwrap_or_default(),
            Usage::new(3600)
        );
        assert!(old.is_complete);
    }

    #[test]
    fn test_a_day_with_no_size_data_shows_a_dash_not_a_zero() {
        // Merging a legacy day with a recent one leaves rows with no expansion
        // or size data, and zero is the sentinel for that - not a score.
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let mut report = ProjectUsageReport::new(&project);

        let mut legacy_day = DailyProjectUsageReport::default();
        legacy_day.add_usage("alice", Usage::new(3600));
        legacy_day.add_jobs("alice", 1);
        legacy_day.add_wait_seconds("alice", 60);
        report.set_report(&Date::parse("2026-03-01").unwrap(), &legacy_day);

        let mut modern_day = DailyProjectUsageReport::default();
        modern_day.add_usage("bob", Usage::new(3600));
        modern_day.add_jobs("bob", 1);
        modern_day.add_wait_seconds("bob", 60);
        modern_day.add_expansion("bob", 60, 3600);
        modern_day.add_job_size("bob", 128, 0);
        report.set_report(&Date::parse("2026-03-02").unwrap(), &modern_day);

        let dump = report.expansion_factor_report();

        let legacy_row = dump
            .lines()
            .find(|line| line.trim_start().starts_with("2026-03-01"));
        let Some(legacy_row) = legacy_row else {
            unreachable!("no row for the legacy day in:\n{}", dump);
        };

        assert!(
            legacy_row.contains('-'),
            "the legacy day should show a dash: {}",
            legacy_row
        );
        assert!(
            !legacy_row.contains("0.00"),
            "and not a zero: {}",
            legacy_row
        );

        // while the modern day shows its real figures
        let modern_row = dump
            .lines()
            .find(|line| line.trim_start().starts_with("2026-03-02"));
        let Some(modern_row) = modern_row else {
            unreachable!("no row for the modern day in:\n{}", dump);
        };
        assert!(modern_row.contains("1.02"), "{}", modern_row);
        assert!(modern_row.contains("128.0"), "{}", modern_row);
    }

    #[test]
    fn test_the_expansion_factor_is_the_mean_of_per_job_ratios() {
        // Turnaround over runtime, per job, averaged - the classical form, so a
        // job that never waited scores exactly 1.0. A mean of ratios rather than
        // a ratio of sums, so that a job which queued for a long time and then
        // exited quickly shows up instead of being swallowed.
        let mut report = DailyProjectUsageReport::default();

        // an hour's wait for an hour's work
        report.add_jobs("alice", 1);
        report.add_wait_seconds("alice", 3600);
        report.add_expansion("alice", 3600, 3600);

        // no wait at all
        report.add_jobs("alice", 1);
        report.add_wait_seconds("alice", 0);
        report.add_expansion("alice", 0, 7200);

        assert_eq!(report.num_jobs(), 2);
        assert_eq!(report.total_runtime_seconds(), 10800);

        // the first job doubled its own runtime waiting, the second waited not
        // at all: (2.0 + 1.0) / 2
        assert!((report.average_expansion_factor() - 1.5).abs() < 1e-9);

        // 3600 waited plus 10800 run, over 10800 run - the same jobs, weighted
        // by size
        assert!((report.aggregate_expansion_factor() - (14400.0 / 10800.0)).abs() < 1e-9);

        assert!(report.is_consistent());
    }

    #[test]
    fn test_a_long_wait_for_a_job_that_dies_immediately_is_visible() {
        // The case the statistic exists for: a user fighting a job that will not
        // run. Two hours queued, four seconds of work, over and over.
        let mut struggling = DailyProjectUsageReport::default();

        for _ in 0..5 {
            struggling.add_jobs("alice", 1);
            struggling.add_wait_seconds("alice", 7200);
            struggling.add_expansion("alice", 7200, 4);
        }

        // 7204 seconds of turnaround for 4 of work: 1801, and loud
        assert!((struggling.average_expansion_factor() - 1801.0).abs() < 0.01);

        // and it survives being averaged with a well-behaved day, which is the
        // reason for preferring the mean of ratios: the aggregate form would
        // bury these five jobs under one long-running one
        let mut healthy = DailyProjectUsageReport::default();
        healthy.add_jobs("bob", 1);
        healthy.add_wait_seconds("bob", 60);
        healthy.add_expansion("bob", 60, 86400);

        let combined = struggling + healthy;

        assert!(
            combined.average_expansion_factor() > 1000.0,
            "the mean of ratios must still show it: {}",
            combined.average_expansion_factor()
        );
        assert!(
            combined.aggregate_expansion_factor() < 1.5,
            "while the aggregate form buries it, close to the ideal 1.0: {}",
            combined.aggregate_expansion_factor()
        );

        // per user is where you look to find who it was
        assert!(combined.expansion_factor_for_user("alice") > 1000.0);
        assert!(combined.expansion_factor_for_user("bob") < 1.01);
        assert!(combined.is_consistent());
    }

    /// A project with three users of deliberately different habits: one running
    /// a few large jobs and well served, one running many small ones, and one
    /// fighting a job that will not run.
    fn project_report_with_mixed_habits() -> ProjectUsageReport {
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let mut report = ProjectUsageReport::new(&project);

        let mut day = DailyProjectUsageReport::default();

        for _ in 0..4 {
            day.add_usage("alice", Usage::new(3600 * 128));
            day.add_jobs("alice", 1);
            day.add_wait_seconds("alice", 900);
            day.add_expansion("alice", 900, 7200);
            day.add_job_size("alice", 512, 16);
        }

        for _ in 0..120 {
            day.add_usage("carol", Usage::new(600));
            day.add_jobs("carol", 1);
            day.add_wait_seconds("carol", 300);
            day.add_expansion("carol", 300, 600);
            day.add_job_size("carol", 2, 0);
        }

        for _ in 0..6 {
            day.add_usage("bob", Usage::new(30));
            day.add_jobs("bob", 1);
            day.add_wait_seconds("bob", 9000);
            day.add_expansion("bob", 9000, 30);
            day.add_job_size("bob", 64, 4);
        }

        report.set_report(&Date::parse("2026-03-01").unwrap(), &day);
        report
    }

    #[test]
    fn test_the_expansion_report_names_the_user_who_is_struggling() {
        // The per-user table is the point of the report: the project-wide mean
        // job size here is about 20 cores, which describes none of these three.
        let report = project_report_with_mixed_habits();
        let dump = report.expansion_factor_report();

        // worst-served first, so the user in trouble is the first row
        let user_rows: Vec<&str> = dump
            .lines()
            .skip_while(|line| !line.contains("expansion"))
            .filter(|line| line.starts_with("  ") && !line.contains("expansion"))
            .collect();

        let Some(worst) = user_rows.first() else {
            unreachable!("no per-user rows in:\n{}", dump);
        };

        assert!(worst.contains("bob"), "bob should be first: {}", worst);
        assert!(worst.contains("301.00"), "{}", worst);

        // and every user appears with their own job shape
        assert!(dump.contains("512.0"), "alice's job size: {}", dump);
        assert!(
            dump.contains("180") || dump.contains("120"),
            "carol's jobs: {}",
            dump
        );
    }

    #[test]
    fn test_the_expansion_report_explains_which_end_of_the_distribution_waited() {
        // The gap between the two forms is the most useful thing in the report,
        // so it is spelled out rather than left to be spotted.
        let struggling = project_report_with_mixed_habits();
        let dump = struggling.expansion_factor_report();

        assert!(
            dump.contains("some *short* jobs waited a"),
            "a mean far above the aggregate should be called out: {}",
            dump
        );

        // the opposite case: one long job that waited, and nothing else
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let mut contended = ProjectUsageReport::new(&project);
        let mut day = DailyProjectUsageReport::default();

        day.add_jobs("alice", 1);
        day.add_wait_seconds("alice", 86400);
        day.add_expansion("alice", 86400, 86400);
        day.add_job_size("alice", 128, 0);

        for _ in 0..50 {
            day.add_jobs("carol", 1);
            day.add_wait_seconds("carol", 0);
            day.add_expansion("carol", 0, 60);
            day.add_job_size("carol", 1, 0);
        }

        contended.set_report(&Date::parse("2026-03-01").unwrap(), &day);
        let dump = contended.expansion_factor_report();

        assert!(
            dump.contains("*long* jobs"),
            "an aggregate far above the mean should be called out: {}",
            dump
        );
    }

    #[test]
    fn test_the_expansion_report_says_nothing_when_there_is_nothing_to_explain() {
        // A well-served project gets the figures and no commentary. The
        // comparison is between the excesses over 1.0, not the raw values: on
        // this scale 1.0 means "waited not at all", so 1.02 against 1.04 is a
        // doubling of nothing and must not be announced as a pattern.
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let mut report = ProjectUsageReport::new(&project);
        let mut day = DailyProjectUsageReport::default();

        for _ in 0..20 {
            day.add_jobs("alice", 1);
            day.add_wait_seconds("alice", 30);
            day.add_expansion("alice", 30, 3600);
            day.add_job_size("alice", 128, 0);
        }

        report.set_report(&Date::parse("2026-03-01").unwrap(), &day);
        let dump = report.expansion_factor_report();

        assert!(!dump.contains("*short* jobs"), "{}", dump);
        assert!(!dump.contains("*long* jobs"), "{}", dump);

        // but the figures themselves are still there
        assert!(dump.contains("20 jobs"), "{}", dump);
        assert!(dump.contains("128.0"), "{}", dump);
    }

    #[test]
    fn test_the_expansion_report_is_one_line_when_no_jobs_ran() {
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let report = ProjectUsageReport::new(&project);

        assert!(report
            .expansion_factor_report()
            .contains("No jobs recorded"));
        assert!(!report
            .expansion_factor_report()
            .contains("expansion factor 0"));
    }

    #[test]
    fn test_the_expansion_report_shows_each_day_so_a_change_is_visible() {
        // When the trouble started is as useful as who caused it.
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let mut report = ProjectUsageReport::new(&project);

        let mut quiet = DailyProjectUsageReport::default();
        quiet.add_jobs("alice", 1);
        quiet.add_expansion("alice", 0, 3600);
        quiet.add_job_size("alice", 128, 0);
        report.set_report(&Date::parse("2026-03-01").unwrap(), &quiet);

        let mut bad = DailyProjectUsageReport::default();
        bad.add_jobs("alice", 1);
        bad.add_wait_seconds("alice", 36000);
        bad.add_expansion("alice", 36000, 60);
        bad.add_job_size("alice", 128, 0);
        report.set_report(&Date::parse("2026-03-02").unwrap(), &bad);

        let dump = report.expansion_factor_report();

        assert!(dump.contains("2026-03-01"), "{}", dump);
        assert!(dump.contains("2026-03-02"), "{}", dump);

        // the quiet day is 1.00 and the bad one is not
        let day_rows: Vec<&str> = dump
            .lines()
            .filter(|line| line.trim_start().starts_with("2026-03-"))
            .collect();

        assert_eq!(day_rows.len(), 2);
        assert!(day_rows[0].contains("1.00"), "{}", day_rows[0]);
        assert!(day_rows[1].contains("601.00"), "{}", day_rows[1]);
    }

    #[test]
    fn test_mean_job_size_distinguishes_many_small_jobs_from_a_few_large_ones() {
        // The distinction usage cannot draw. Both of these projects consumed the
        // same core-seconds; one ran a single wide job and the other ran a
        // hundred narrow ones.
        let mut wide = DailyProjectUsageReport::default();
        wide.add_usage("alice", Usage::new(3600 * 128));
        wide.add_jobs("alice", 1);
        wide.add_expansion("alice", 0, 3600);
        wide.add_job_size("alice", 128, 4);

        let mut narrow = DailyProjectUsageReport::default();
        for _ in 0..128 {
            narrow.add_usage("bob", Usage::new(3600));
            narrow.add_jobs("bob", 1);
            narrow.add_expansion("bob", 0, 3600);
            narrow.add_job_size("bob", 1, 0);
        }

        // indistinguishable by usage...
        assert_eq!(wide.total_usage(), narrow.total_usage());

        // ...and plainly different by job size
        assert_eq!(wide.average_cpus_per_job(), 128.0);
        assert_eq!(wide.average_gpus_per_job(), 4.0);
        assert_eq!(narrow.average_cpus_per_job(), 1.0);
        assert_eq!(narrow.average_gpus_per_job(), 0.0);

        assert!(wide.is_consistent());
        assert!(narrow.is_consistent());
    }

    #[test]
    fn test_job_size_is_unweighted_by_runtime() {
        // Each job counts once however long it ran, because the question is what
        // shape the jobs were - not what the machine was occupied by, which is
        // what the usage components already answer.
        let mut report = DailyProjectUsageReport::default();

        // one enormous job that ran for a minute
        report.add_jobs("alice", 1);
        report.add_expansion("alice", 0, 60);
        report.add_job_size("alice", 512, 16);

        // and one small job that ran for a day
        report.add_jobs("alice", 1);
        report.add_expansion("alice", 0, 86400);
        report.add_job_size("alice", 2, 0);

        // (512 + 2) / 2 - the day-long job does not dominate
        assert_eq!(report.average_cpus_per_job(), 257.0);
        assert_eq!(report.average_gpus_per_job(), 8.0);
    }

    #[test]
    fn test_mean_job_size_is_per_user_and_per_project_over_every_job() {
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let mut report = ProjectUsageReport::new(&project);

        let mut busy = DailyProjectUsageReport::default();
        for _ in 0..9 {
            busy.add_jobs("alice", 1);
            busy.add_job_size("alice", 4, 0);
        }
        busy.add_jobs("bob", 1);
        busy.add_job_size("bob", 256, 8);
        report.set_report(&Date::parse("2026-03-01").unwrap(), &busy);

        let mut quiet = DailyProjectUsageReport::default();
        quiet.add_jobs("alice", 1);
        quiet.add_job_size("alice", 4, 0);
        report.set_report(&Date::parse("2026-03-02").unwrap(), &quiet);

        // ten four-core jobs and one 256-core job, over eleven jobs
        assert_eq!(report.num_jobs(), 11);
        assert!((report.average_cpus_per_job() - (296.0 / 11.0)).abs() < 1e-9);

        // and per user, which is where the outlier is
        assert_eq!(report.average_cpus_per_job_for_user("alice"), 4.0);
        assert_eq!(report.average_cpus_per_job_for_user("bob"), 256.0);
        assert_eq!(report.average_gpus_per_job_for_user("bob"), 8.0);
        assert_eq!(report.average_cpus_per_job_for_user("nobody"), 0.0);
    }

    #[test]
    fn test_job_sizes_survive_merging_renaming_and_scaling() {
        let mut day = DailyProjectUsageReport::default();
        day.add_usage("alice", Usage::new(3600));
        day.add_jobs("alice", 2);
        day.add_job_size("alice", 64, 4);

        let mut merged = day.clone();
        merged += day.clone();

        assert_eq!(merged.total_allocated_cpus(), 128);
        assert_eq!(merged.total_allocated_gpus(), 8);
        assert_eq!(merged.num_jobs(), 4);
        assert_eq!(merged.average_cpus_per_job(), 32.0);
        assert!(merged.is_consistent());

        // a credit conversion does not change how many cores a job held
        let scaled = day.clone() * 100.0;
        assert_eq!(scaled.total_allocated_cpus(), 64);
        assert_eq!(scaled.average_cpus_per_job(), day.average_cpus_per_job());

        let mut renamed = day.clone();
        let mut renames = HashMap::new();
        renames.insert("alice".to_string(), "alice2".to_string());
        renamed.remap_local_users(&renames);

        assert_eq!(renamed.allocated_cpus_for_user("alice2"), 64);
        assert_eq!(renamed.allocated_cpus_for_user("alice"), 0);
        assert_eq!(renamed.average_cpus_per_job_for_user("alice2"), 32.0);
        assert!(renamed.is_consistent());
    }

    #[test]
    fn test_a_report_from_an_instance_without_job_sizes_still_loads() {
        let legacy = serde_json::json!({
            "reports": { "alice": { "seconds": 1800 } },
            "num_jobs": 1,
            "is_complete": true
        });

        let report: DailyProjectUsageReport = serde_json::from_value(legacy).unwrap();

        assert_eq!(report.total_allocated_cpus(), 0);
        assert_eq!(report.average_cpus_per_job(), 0.0);
        assert_eq!(report.average_gpus_per_job(), 0.0);
        assert!(report.is_consistent());
    }

    #[test]
    fn test_a_job_that_never_waited_scores_exactly_one() {
        // The classical convention: 1.0 is the ideal, not 0.0. Reading it the
        // other way round would make a perfectly served project look like a
        // badly served one.
        let mut report = DailyProjectUsageReport::default();
        report.add_jobs("alice", 1);
        report.add_wait_seconds("alice", 0);
        report.add_expansion("alice", 0, 3600);

        assert_eq!(report.average_expansion_factor(), 1.0);
        assert_eq!(report.aggregate_expansion_factor(), 1.0);

        // and a job that waited as long as it ran scores 2.0
        let mut report = DailyProjectUsageReport::default();
        report.add_jobs("alice", 1);
        report.add_wait_seconds("alice", 3600);
        report.add_expansion("alice", 3600, 3600);

        assert_eq!(report.average_expansion_factor(), 2.0);
        assert_eq!(report.aggregate_expansion_factor(), 2.0);
    }

    #[test]
    fn test_a_job_with_no_runtime_cannot_divide_by_zero() {
        // `op-slurm` does not report jobs that consumed nothing, so this should
        // never arise - but a division by zero here would abort the process, and
        // the values come from a peer.
        let mut report = DailyProjectUsageReport::default();
        report.add_jobs("alice", 1);
        report.add_expansion("alice", 3600, 0);

        assert_eq!(report.total_runtime_seconds(), 0);
        assert_eq!(report.average_expansion_factor(), 0.0);
        assert_eq!(report.aggregate_expansion_factor(), 0.0);

        // An empty report is 0.0 rather than a NaN. Note that 0.0 is the
        // no-jobs sentinel and not a score - on the classical scale no real job
        // can be below 1.0, so the two cannot be confused.
        let empty = DailyProjectUsageReport::default();
        assert_eq!(empty.average_expansion_factor(), 0.0);
        assert_eq!(empty.aggregate_expansion_factor(), 0.0);
    }

    #[test]
    fn test_expansion_sums_are_order_independent() {
        // Accumulated as thousandths precisely so that merging the same reports
        // in a different order gives the same answer - these reports are merged
        // out of HashMaps whose iteration order is arbitrary, and float addition
        // is not associative.
        let day = |wait: u64, run: u64| {
            let mut report = DailyProjectUsageReport::default();
            report.add_jobs("alice", 1);
            report.add_wait_seconds("alice", wait);
            report.add_expansion("alice", wait, run);
            report
        };

        let forwards = day(1, 3) + day(2, 7) + day(5, 11);
        let backwards = day(5, 11) + day(2, 7) + day(1, 3);

        assert_eq!(
            forwards.total_expansion_milli(),
            backwards.total_expansion_milli()
        );
        assert_eq!(
            forwards.average_expansion_factor(),
            backwards.average_expansion_factor()
        );
    }

    #[test]
    fn test_a_projects_expansion_factor_weighs_every_job_not_every_day() {
        // Averaging each day's average would let a day with one job weigh as
        // heavily as a day with a hundred.
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let mut report = ProjectUsageReport::new(&project);

        let mut busy = DailyProjectUsageReport::default();
        for _ in 0..99 {
            busy.add_jobs("alice", 1);
            busy.add_wait_seconds("alice", 0);
            busy.add_expansion("alice", 0, 3600);
        }
        report.set_report(&Date::parse("2026-03-01").unwrap(), &busy);

        let mut quiet = DailyProjectUsageReport::default();
        quiet.add_jobs("alice", 1);
        quiet.add_wait_seconds("alice", 3600);
        quiet.add_expansion("alice", 3600, 3600);
        report.set_report(&Date::parse("2026-03-02").unwrap(), &quiet);

        // ninety-nine jobs scored 1.0 and one scored 2.0, so the mean is 1.01
        assert_eq!(report.num_jobs(), 100);
        assert!((report.average_expansion_factor() - 1.01).abs() < 1e-9);

        // not 1.5, which is what averaging the two daily means would give
        assert!(report.average_expansion_factor() < 1.1);
    }

    #[test]
    fn test_scaling_usage_leaves_the_expansion_factor_alone() {
        // A credit conversion rescales usage. It does not change how many jobs
        // ran, how long they queued, or a dimensionless ratio of the two.
        let mut report = DailyProjectUsageReport::default();
        report.add_usage("alice", Usage::new(3600));
        report.add_jobs("alice", 1);
        report.add_wait_seconds("alice", 1800);
        report.add_expansion("alice", 1800, 3600);

        let scaled = report.clone() * 10.0;

        assert_eq!(scaled.total_usage(), Usage::new(36000));
        assert_eq!(scaled.total_runtime_seconds(), 3600);
        assert_eq!(
            scaled.average_expansion_factor(),
            report.average_expansion_factor()
        );

        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let mut project_report = ProjectUsageReport::new(&project);
        project_report.set_report(&Date::parse("2026-03-01").unwrap(), &report);
        project_report.scale_total(0.5);

        assert!((project_report.average_expansion_factor() - 1.5).abs() < 1e-9);
        assert_eq!(project_report.total_runtime_seconds(), 3600);
    }

    #[test]
    fn test_a_report_from_an_instance_without_expansion_factors_still_loads() {
        let legacy = serde_json::json!({
            "reports": { "alice": { "seconds": 1800 } },
            "num_jobs": 1,
            "total_wait_seconds": 60,
            "is_complete": true
        });

        let report: DailyProjectUsageReport = serde_json::from_value(legacy).unwrap();

        assert_eq!(report.total_runtime_seconds(), 0);
        assert_eq!(report.average_expansion_factor(), 0.0);
        assert!(report.is_consistent());
    }

    /// A two-day project report: one day with requeues, one without.
    fn project_report_with_requeues() -> ProjectUsageReport {
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let mut report = ProjectUsageReport::new(&project);

        report.set_report(&Date::parse("2026-03-01").unwrap(), &report_with_requeues());

        let mut quiet_day = DailyProjectUsageReport::default();
        quiet_day.add_usage("alice", Usage::new(3600));
        quiet_day.add_jobs("alice", 1);
        report.set_report(&Date::parse("2026-03-02").unwrap(), &quiet_day);

        report
    }

    /// A day with usage inside two reservations and some outside them.
    fn report_with_reservations() -> DailyProjectUsageReport {
        let mut report = report_with_requeues();

        // of alice's 1800 reported and 7200 discarded seconds, some ran inside
        // `bench`; bob's 600 ran inside `maint`
        report.add_reservation_usage("bench", "alice", Usage::new(1200));
        report.add_reservation_usage("bench", "alice", Usage::new(4800));
        report.add_reservation_requeue_usage("bench", Usage::new(4800));
        report.add_reservation_jobs("bench", 1);

        report.add_reservation_usage("maint", "bob", Usage::new(600));
        report.add_reservation_jobs("maint", 1);

        report
    }

    #[test]
    fn test_reservation_usage_is_a_subset_of_consumption_not_a_partition_of_it() {
        // Most jobs run outside a reservation, so these figures account for part
        // of a day rather than all of it - and they count superseded attempts,
        // so the part they account for is of the true total, not the reported
        // one.
        let report = report_with_reservations();

        assert_eq!(report.reservation_usage("bench"), Usage::new(6000));
        assert_eq!(report.reservation_requeue_usage("bench"), Usage::new(4800));
        assert_eq!(report.reservation_usage("maint"), Usage::new(600));
        assert_eq!(report.total_reservation_usage(), Usage::new(6600));

        assert_eq!(
            report.total_reservation_usage() + report.usage_outside_reservations(),
            report.total_usage_including_requeues()
        );

        assert_eq!(
            report.reservation_usage_for_user("bench", "alice"),
            Usage::new(6000)
        );
        assert_eq!(report.reservation_users("bench"), vec!["alice".to_string()]);
        assert!(report.is_consistent());
    }

    #[test]
    fn test_a_report_claiming_more_reservation_usage_than_it_consumed_is_inconsistent() {
        // The one invariant available: reservations account for a subset, so
        // usage inside them exceeding everything consumed means a record was
        // counted twice.
        let mut report = DailyProjectUsageReport::default();
        report.add_usage("alice", Usage::new(600));
        report.add_reservation_usage("bench", "alice", Usage::new(6000));

        assert!(!report.is_consistent());

        // and a discarded share larger than the reservation's own usage
        let mut report = DailyProjectUsageReport::default();
        report.add_usage("alice", Usage::new(6000));
        report.add_reservation_usage("bench", "alice", Usage::new(600));
        report.add_reservation_requeue_usage("bench", Usage::new(6000));

        assert!(!report.is_consistent());
    }

    #[test]
    fn test_reservation_figures_survive_merging_scaling_and_renaming() {
        let mut merged = report_with_reservations();
        merged += report_with_reservations();

        assert_eq!(merged.reservation_usage("bench"), Usage::new(12000));
        assert_eq!(merged.reservation_requeue_usage("bench"), Usage::new(9600));
        assert_eq!(merged.reservation_jobs("bench"), 2);
        assert!(merged.is_consistent());

        // scaled with the totals, never apart from them - a client converts to
        // credits and then compares the two
        let doubled = report_with_reservations() * 2.0;
        assert_eq!(doubled.reservation_usage("bench"), Usage::new(12000));
        assert_eq!(doubled.total_usage(), Usage::new(4800));

        let mut renamed = report_with_reservations();
        let mut renames = HashMap::new();
        renames.insert("alice".to_string(), "alice2".to_string());
        renamed.remap_local_users(&renames);

        assert_eq!(
            renamed.reservation_usage_for_user("bench", "alice2"),
            Usage::new(6000)
        );
        assert_eq!(
            renamed.reservation_usage_for_user("bench", "alice"),
            Usage::default()
        );
        // the reservation itself is not a user and keeps its name
        assert_eq!(renamed.reservation_usage("bench"), Usage::new(6000));
    }

    #[test]
    fn test_a_report_from_an_instance_without_reservations_still_loads() {
        let legacy = serde_json::json!({
            "reports": { "alice": { "seconds": 1800 } },
            "num_jobs": 1,
            "is_complete": true
        });

        let report: DailyProjectUsageReport = serde_json::from_value(legacy).unwrap();

        assert!(!report.has_reservations());
        assert_eq!(report.total_reservation_usage(), Usage::default());
        assert_eq!(report.usage_outside_reservations(), Usage::new(1800));
        assert!(report.is_consistent());
    }

    #[test]
    fn test_the_reservation_report_says_what_went_in_and_not_how_full_it_was() {
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let mut report = ProjectUsageReport::new(&project);
        report.set_report(
            &Date::parse("2026-03-01").unwrap(),
            &report_with_reservations(),
        );

        let dump = report.reservation_report();

        assert!(dump.contains("Consumed inside reservations"));
        assert!(dump.contains("Consumed outside reservations"));

        // busiest first
        assert!(dump.find("bench") < dump.find("maint"), "{}", dump);

        // the discarded share is called out, since it went into the reservation
        // but is not in the usage we report
        assert!(dump.contains("discarded by requeues"), "{}", dump);

        // and the report is explicit about the question it cannot answer, so
        // nobody reads these shares as utilisation
        assert!(
            dump.contains("is a property of the reservation"),
            "the report must not be mistaken for utilisation: {}",
            dump
        );
    }

    #[test]
    fn test_the_reservation_report_is_one_line_when_nothing_was_reserved() {
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let mut report = ProjectUsageReport::new(&project);
        report.set_report(&Date::parse("2026-03-01").unwrap(), &report_with_requeues());

        assert!(!report.has_reservations());
        assert!(report
            .reservation_report()
            .contains("No jobs ran inside a reservation"));
    }

    #[test]
    fn test_the_requeue_report_says_what_was_lost_and_what_did_the_interrupting() {
        let report = project_report_with_requeues();
        let dump = report.requeue_report();

        // the three figures a charging decision turns on
        assert!(dump.contains("Reported usage (final attempt of each job)"));
        assert!(dump.contains("Discarded by requeues"));
        assert!(dump.contains("True consumption (Slurm's view)"));

        // 8100 of 6000 + 8100 seconds discarded
        assert!(
            dump.contains("57.4%"),
            "expected the discarded share: {}",
            dump
        );

        // the breakdown that separates the site's fault from the project's
        let by_state = dump
            .lines()
            .skip_while(|line| !line.starts_with("Work was interrupted by:"))
            .take(3)
            .collect::<Vec<&str>>()
            .join("\n");

        assert!(by_state.contains("NODE_FAIL"), "{}", by_state);
        assert!(by_state.contains("PREEMPTED"), "{}", by_state);

        // worst first, so NODE_FAIL's 7200 seconds outrank PREEMPTED's 900
        let node_fail = dump.find("NODE_FAIL");
        let preempted = dump.find("PREEMPTED");
        assert!(
            node_fail < preempted,
            "worst state should come first: {}",
            dump
        );

        // per day, and only the days that had any
        assert!(dump.contains("2026-03-01"), "{}", dump);
        assert!(
            !dump.contains("2026-03-02"),
            "quiet days are not listed: {}",
            dump
        );

        // per user, with the events attributed to each
        assert!(dump.contains("alice"), "{}", dump);
        assert!(dump.contains("bob"), "{}", dump);
    }

    #[test]
    fn test_the_requeue_report_is_a_single_line_when_there_is_nothing_to_say() {
        // The overwhelmingly common case, and it should not print a page of
        // zeroes to say so.
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let mut report = ProjectUsageReport::new(&project);

        let mut day = DailyProjectUsageReport::default();
        day.add_usage("alice", Usage::new(3600));
        day.add_jobs("alice", 1);
        report.set_report(&Date::parse("2026-03-01").unwrap(), &day);

        assert!(!report.has_requeues());
        assert!(report
            .requeue_report()
            .contains("No requeued jobs recorded"));
        assert!(!report.requeue_report().contains("By day"));
    }

    #[test]
    fn test_a_day_whose_whole_consumption_was_requeued_is_still_shown() {
        // A day can consist entirely of attempts that were later requeued - the
        // usage is real and the day has plenty to say, but it has no *reported*
        // usage, and the daily listing used to drop anything with a zero total.
        let project = ProjectIdentifier::parse("proj.portal").unwrap();
        let mut report = ProjectUsageReport::new(&project);

        let mut day = DailyProjectUsageReport::default();
        day.add_requeue_usage("alice", Usage::new(7200));
        day.add_requeue_state_usage("NODE_FAIL", Usage::new(7200));
        day.add_requeue_events("alice", "NODE_FAIL", 1);
        report.set_report(&Date::parse("2026-03-01").unwrap(), &day);

        assert_eq!(report.total_usage(), Usage::default());
        assert_eq!(report.total_requeue_usage(), Usage::new(7200));

        // it survives the daily listing, the printout and the requeue report
        assert_eq!(report.daily_reports(true).len(), 1);
        assert!(report.to_string().contains("2026-03-01"));
        assert!(report.requeue_report().contains("2026-03-01"));
    }

    #[test]
    fn test_the_daily_printout_reports_requeues_per_day() {
        // The per-day requeue line existed on a daily report's own `Display`,
        // but a project report renders its days itself, so it never appeared
        // where anyone was reading it.
        let printed = project_report_with_requeues().to_string();

        let day_line = printed
            .lines()
            .skip_while(|line| !line.starts_with("2026-03-01"))
            .take_while(|line| !line.starts_with("----"))
            .find(|line| line.starts_with("Requeued:"));

        let Some(day_line) = day_line else {
            unreachable!("no per-day requeue line in:\n{}", printed);
        };

        assert!(day_line.contains("3 events"), "{}", day_line);

        // and the quiet day says nothing about requeues
        let quiet: Vec<&str> = printed
            .lines()
            .skip_while(|line| !line.starts_with("2026-03-02"))
            .take_while(|line| !line.starts_with("----"))
            .collect();

        assert!(!quiet.iter().any(|line| line.starts_with("Requeued:")));
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
