// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::storage::{QuotaLimit, Volume};
use crate::usagereport::Usage;
use templemeads::destination::{Destination, Destinations};
use templemeads::named::NamedType;
use templemeads::portal_identifier::PortalIdentifier;
use templemeads::validate::{validate_identifier_component, validate_mapping_target, LocalUser};
use templemeads::Error;

use anyhow::Context;
use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::{hash::Hash, sync::Arc};
use ts_rs::TS;
use url::Url;
use wildmatch::WildMatch;

///
/// A project identifier - this is a double of project.portal
///
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectIdentifier {
    project: String,
    portal: String,
}

impl NamedType for ProjectIdentifier {
    fn type_name() -> String {
        "ProjectIdentifier".to_string()
    }
}

impl ProjectIdentifier {
    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = identifier.split('.').collect();

        // Destructured rather than indexed, so a wrong component count is a
        // parse error by construction and cannot panic - see
        // docs/specifications/security-review-2.md (finding R1).
        let [project, portal] = parts.as_slice() else {
            return Err(Error::Parse(format!(
                "Invalid ProjectIdentifier: {}",
                identifier
            )));
        };

        let project = project.trim();
        let portal = portal.trim();

        validate_identifier_component(project, "project", identifier)?;
        validate_identifier_component(portal, "portal", identifier)?;

        Ok(Self {
            project: project.to_string(),
            portal: portal.to_string(),
        })
    }

    pub fn project(&self) -> String {
        self.project.clone()
    }

    pub fn portal(&self) -> String {
        self.portal.clone()
    }

    pub fn portal_identifier(&self) -> PortalIdentifier {
        PortalIdentifier::from_validated(self.portal.clone())
    }
}

impl std::fmt::Display for ProjectIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.project, self.portal)
    }
}

impl From<UserIdentifier> for ProjectIdentifier {
    fn from(user: UserIdentifier) -> Self {
        Self {
            project: user.project().to_string(),
            portal: user.portal().to_string(),
        }
    }
}

impl Serialize for ProjectIdentifier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProjectIdentifier {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

///
/// A user identifier - this is a triple of username.project.portal
///
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserIdentifier {
    username: String,
    project: String,
    portal: String,
}

impl NamedType for UserIdentifier {
    fn type_name() -> String {
        "UserIdentifier".to_string()
    }
}

impl UserIdentifier {
    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = identifier.split('.').collect();

        let [username, project, portal] = parts.as_slice() else {
            return Err(Error::Parse(format!(
                "Invalid UserIdentifier: {}",
                identifier
            )));
        };

        let username = username.trim();
        let project = project.trim();
        let portal = portal.trim();

        validate_identifier_component(username, "username", identifier)?;
        validate_identifier_component(project, "project", identifier)?;
        validate_identifier_component(portal, "portal", identifier)?;

        Ok(Self {
            username: username.to_string(),
            project: project.to_string(),
            portal: portal.to_string(),
        })
    }

    pub fn username(&self) -> String {
        self.username.clone()
    }

    pub fn project(&self) -> String {
        self.project.clone()
    }

    pub fn portal(&self) -> String {
        self.portal.clone()
    }

    pub fn project_identifier(&self) -> ProjectIdentifier {
        ProjectIdentifier {
            project: self.project.clone(),
            portal: self.portal.clone(),
        }
    }

    pub fn portal_identifier(&self) -> PortalIdentifier {
        PortalIdentifier::from_validated(self.portal.clone())
    }
}

impl std::fmt::Display for UserIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.username, self.project, self.portal)
    }
}

/// Serialize and Deserialize via the string representation
/// of the UserIdentifier
impl Serialize for UserIdentifier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UserIdentifier {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

///
/// Struct that holds the mapping of a ProjectIdentifier to a local
/// project on a system
///
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectMapping {
    project: ProjectIdentifier,
    local_group: String,
}

impl NamedType for ProjectMapping {
    fn type_name() -> String {
        "ProjectMapping".to_string()
    }
}

impl ProjectMapping {
    pub fn new(project: &ProjectIdentifier, local_group: &str) -> Result<Self, Error> {
        let local_group = local_group.trim();

        // Allow-list, not a deny-list. The previous deny-list rejected only
        // empty, leading/trailing `.`, `/`, a leading `-` and control
        // characters - which still admitted whitespace, `,`, `=`, `%`, `?` and
        // `#`. Those matter because a mapping target is not only a spawned
        // tool's operand: it is interpolated into space-delimited OpenPortal
        // instruction strings (a space shifts every later argument), into
        // `sacctmgr` `key=value` arguments (a comma is a list separator), and
        // into Slurm REST URLs (a `?` starts a query). See
        // `docs/specifications/security-review-2.md` (finding R14).
        validate_mapping_target(local_group, "local_group", local_group)?;

        Ok(Self {
            project: project.clone(),
            local_group: local_group.to_string(),
        })
    }

    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = identifier.split(':').collect();

        let [project, local_group] = parts.as_slice() else {
            return Err(Error::Parse(format!(
                "Invalid ProjectMapping: {}",
                identifier
            )));
        };

        let project = ProjectIdentifier::parse(project)?;
        let local_group = local_group.trim();

        Self::new(&project, local_group)
    }

    pub fn project(&self) -> &ProjectIdentifier {
        &self.project
    }

    pub fn local_group(&self) -> &str {
        &self.local_group
    }
}

impl std::fmt::Display for ProjectMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.project, self.local_group)
    }
}

impl From<UserMapping> for ProjectMapping {
    fn from(mapping: UserMapping) -> Self {
        Self {
            project: mapping.user().project_identifier(),
            local_group: mapping.local_group().to_string(),
        }
    }
}

impl Serialize for ProjectMapping {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProjectMapping {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

///
/// Struct that holds the mapping of a UserIdentifier to a local
/// username on a system
///
/// The `local_user` is a [`LocalUser`] rather than a `String` because the two
/// layers that produce mappings mean different things by it: an account agent
/// reports a Unix account name, while a portal reports the member's email
/// address. Consumers must therefore say which they need -
/// [`LocalUser::unix`] for anything that becomes a Unix name, a path, or a
/// command operand, [`LocalUser::as_str`] for display and reports. See the
/// [`LocalUser`] docs for why the distinction is made at parse time rather
/// than by widening `validate_mapping_target`.
#[derive(Debug, Clone, PartialEq)]
pub struct UserMapping {
    user: UserIdentifier,
    local_user: LocalUser,
    local_group: String,
}

impl NamedType for UserMapping {
    fn type_name() -> String {
        "UserMapping".to_string()
    }
}

impl UserMapping {
    pub fn new(user: &UserIdentifier, local_user: &str, local_group: &str) -> Result<Self, Error> {
        let local_group = local_group.trim();

        // Allow-list rather than deny-list - see `ProjectMapping::new` and
        // `docs/specifications/security-review-2.md` (finding R14).
        //
        // `local_user` may be either a Unix account name or an email address,
        // so it is parsed into a `LocalUser`, which applies whichever of the
        // two grammars fits. `local_group` stays a strict mapping target: it
        // names a Unix group at every layer (a portal reports the project
        // identifier, which already satisfies these rules), so nothing needs
        // the email form there.
        let local_user = LocalUser::parse(local_user)?;
        validate_mapping_target(local_group, "local_group", local_group)?;

        Ok(Self {
            user: user.clone(),
            local_user,
            local_group: local_group.to_string(),
        })
    }

    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = identifier.split(':').collect();

        let [user, local_user, local_group] = parts.as_slice() else {
            return Err(Error::Parse(format!("Invalid UserMapping: {}", identifier)));
        };

        let user = UserIdentifier::parse(user)?;
        let local_user = local_user.trim();
        let local_group = local_group.trim();

        Self::new(&user, local_user, local_group)
    }

    pub fn user(&self) -> &UserIdentifier {
        &self.user
    }

    /// The local user this mapping points at.
    ///
    /// Call [`LocalUser::unix`] on the result to use it as a Unix account name
    /// (which fails if the mapping came from a portal and carries an email
    /// address), or [`LocalUser::as_str`] to display or record it.
    pub fn local_user(&self) -> &LocalUser {
        &self.local_user
    }

    pub fn local_group(&self) -> &str {
        &self.local_group
    }

    pub fn project(&self) -> ProjectMapping {
        ProjectMapping {
            project: self.user.project_identifier(),
            local_group: self.local_group.clone(),
        }
    }
}

impl std::fmt::Display for UserMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.user, self.local_user, self.local_group)
    }
}

/// Serialize and Deserialize via the string representation
/// of the UserMapping
impl Serialize for UserMapping {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UserMapping {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

///
/// Simple enum that can hold either a user or project identifier
///
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UserOrProjectIdentifier {
    User(UserIdentifier),
    Project(ProjectIdentifier),
}

impl From<UserIdentifier> for UserOrProjectIdentifier {
    fn from(user: UserIdentifier) -> Self {
        UserOrProjectIdentifier::User(user)
    }
}

impl From<ProjectIdentifier> for UserOrProjectIdentifier {
    fn from(project: ProjectIdentifier) -> Self {
        UserOrProjectIdentifier::Project(project)
    }
}

///
/// Simple enum that can hold either a user or project mapping
///
#[derive(Debug, Clone, PartialEq)]
pub enum UserOrProjectMapping {
    User(UserMapping),
    Project(ProjectMapping),
}

impl From<UserMapping> for UserOrProjectMapping {
    fn from(user: UserMapping) -> Self {
        UserOrProjectMapping::User(user)
    }
}

impl From<ProjectMapping> for UserOrProjectMapping {
    fn from(project: ProjectMapping) -> Self {
        UserOrProjectMapping::Project(project)
    }
}

///
/// Struct used to represent a single hour
///
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Hour {
    hour: chrono::NaiveDateTime,
}

impl NamedType for Hour {
    fn type_name() -> String {
        "Hour".to_string()
    }
}

impl Hour {
    pub fn to_chrono(&self) -> chrono::NaiveDateTime {
        self.hour
    }

    pub fn from_chrono(hour: &chrono::NaiveDateTime) -> Result<Self, Error> {
        // make sure that this is a valid hour (i.e. minutes and seconds are zero)
        if hour.minute() != 0 || hour.second() != 0 {
            return Err(Error::Parse(format!(
                "Invalid Hour - minutes and seconds must be zero '{}'",
                hour
            )));
        }

        Ok(Self { hour: *hour })
    }

    pub fn from_timestamp(timestamp: i64) -> Result<Self, Error> {
        let hour = chrono::DateTime::from_timestamp(timestamp, 0)
            .with_context(|| {
                format!(
                    "Invalid Hour - cannot convert timestamp '{}' to a valid hour",
                    timestamp
                )
            })?
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .with_context(|| {
                format!(
                    "Invalid Hour - cannot convert timestamp '{}' to a valid hour",
                    timestamp
                )
            })?;

        Self::from_chrono(&hour)
    }

    pub fn parse(hour: &str) -> Result<Self, Error> {
        let hour = hour.trim();

        if hour.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid Hour - cannot be empty '{}'",
                hour
            )));
        };

        let hour = chrono::NaiveDateTime::parse_from_str(hour, "%Y-%m-%d %H")
            .with_context(|| format!("Invalid Hour - hour cannot be parsed from '{}'", hour))?;

        Self::from_chrono(&hour)
    }

    pub fn timestamp(&self) -> i64 {
        self.hour.and_utc().timestamp()
    }

    pub fn now() -> Result<Self, Error> {
        let now = chrono::Local::now().naive_local();
        let hour = chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), now.day())
            .unwrap_or_else(|| {
                tracing::error!("Invalid date '{}' - cannot convert to an hour", now);
                chrono::NaiveDate::default()
            })
            .and_hms_opt(now.hour(), 0, 0)
            .unwrap_or_else(|| {
                tracing::error!("Invalid time '{}' - cannot convert to an hour", now);
                chrono::NaiveDateTime::default()
            });

        Self::from_chrono(&hour)
    }

    pub fn prev(self: &Hour) -> Result<Self, Error> {
        let hour = self.hour - chrono::Duration::hours(1);
        Self::from_chrono(&hour)
    }

    pub fn next(self: &Hour) -> Result<Self, Error> {
        let hour = self.hour + chrono::Duration::hours(1);
        Self::from_chrono(&hour)
    }

    pub fn day(self: &Hour) -> Date {
        Date {
            date: self.hour.date(),
        }
    }

    pub fn hour(&self) -> &chrono::NaiveDateTime {
        &self.hour
    }

    // the start time is inclusive, i.e. [start_time, end_time)
    pub fn start_time(&self) -> chrono::NaiveDateTime {
        self.hour
    }

    // the end time is exclusive, i.e. [start_time, end_time)
    pub fn end_time(&self) -> chrono::NaiveDateTime {
        // Checked, so a date at the very top of the representable range cannot
        // panic here - see finding R25.
        self.hour
            .checked_add_signed(chrono::Duration::hours(1))
            .unwrap_or(self.hour)
    }

    pub fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.hour.partial_cmp(&other.hour)
    }

    pub fn is_in(&self, date: &Date) -> bool {
        self.hour.date() == date.date
    }
}

impl std::fmt::Display for Hour {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.hour.format("%Y-%m-%d %H"))
    }
}

impl Serialize for Hour {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Hour {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// Truncate to the top of the hour, so `Hour`'s invariant - minutes and seconds are
/// zero - holds however it was constructed.
///
/// `from_chrono` *rejects* a non-zero minute or second, but this `From` accepted one
/// silently, so `.into()` could produce an `Hour` that is not on an hour boundary. No
/// exploiting caller was found, and making this fallible would be a public API break
/// for no security benefit, so it truncates instead: the invariant becomes
/// unconditional rather than merely checked on one of the two paths. See
/// `docs/specifications/security-review-2.md` (finding R33).
impl From<chrono::NaiveDateTime> for Hour {
    fn from(hour: chrono::NaiveDateTime) -> Self {
        let truncated = hour.with_minute(0).and_then(|h| h.with_second(0));

        match truncated {
            Some(hour) => Self { hour },
            None => {
                // `with_minute(0)`/`with_second(0)` cannot fail for a valid
                // NaiveDateTime, but never panic on a value that came from the wire.
                tracing::warn!(
                    "Could not truncate '{}' to the top of the hour - using it as-is",
                    hour
                );
                Self { hour }
            }
        }
    }
}

impl From<Hour> for chrono::NaiveDateTime {
    fn from(hour: Hour) -> Self {
        hour.hour
    }
}

///
/// Struct used to represent a single date
///
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Date {
    date: chrono::NaiveDate,
}

impl NamedType for Date {
    fn type_name() -> String {
        "Date".to_string()
    }
}

/// Earliest and latest year a `Date` may name.
///
/// `chrono`'s `%Y` accepts an unbounded digit count when a sign is present, so
/// `Date::parse` used to accept chrono's entire representable range - from
/// `-262143-01-01` to `+262142-12-31`. Nothing rejected a `DateRange` spanning
/// it either, and a span that wide is a resource bomb rather than a query:
/// `DateRange::days()` would try to build a 191,491,529-element `Vec`, and the
/// day-by-day arithmetic in `days`/`weeks`/`end_time` overflowed at the top of
/// the range. Bounding the year at parse time closes that whole class in one
/// check. See `docs/specifications/security-review-2.md` (finding R25).
const MIN_YEAR: i32 = 1970;
const MAX_YEAR: i32 = 2200;

/// Longest span a single `DateRange` may cover, in days (~5 years).
///
/// Usage and storage reports are aggregated per day, per week or per month over
/// a range, so the range's length directly bounds how much a single instruction
/// can ask an agent to allocate and compute. Five years is far beyond any real
/// reporting query while keeping the worst case at a few thousand elements. See
/// `docs/specifications/security-review-2.md` (finding R25).
const MAX_DATE_RANGE_DAYS: i64 = 5 * 366;

impl Date {
    pub fn to_chrono(&self) -> chrono::NaiveDate {
        self.date
    }

    pub fn from_chrono(date: &chrono::NaiveDate) -> Self {
        Self { date: *date }
    }

    pub fn from_timestamp(timestamp: i64) -> Self {
        Self {
            date: chrono::DateTime::from_timestamp(timestamp, 0)
                .unwrap_or_default()
                .date_naive(),
        }
    }

    pub fn parse(date: &str) -> Result<Self, Error> {
        let date = date.trim();

        if date.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid Date - cannot be empty '{}'",
                date
            )));
        };

        let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .with_context(|| format!("Invalid Date - date cannot be parsed from '{}'", date))?;

        // `%Y` accepts a signed, unbounded digit count, so without this the
        // whole chrono range parses - see `MIN_YEAR`/`MAX_YEAR` (finding R25).
        if date.year() < MIN_YEAR || date.year() > MAX_YEAR {
            return Err(Error::Parse(format!(
                "Invalid Date - year {} is outside the supported range {}-{} '{}'",
                date.year(),
                MIN_YEAR,
                MAX_YEAR,
                date
            )));
        }

        Ok(Self { date })
    }

    pub fn timestamp(&self) -> i64 {
        self.date
            .and_hms_opt(0, 0, 0)
            .unwrap_or_else(|| {
                tracing::error!(
                    "Invalid date '{}' - cannot convert to a timestamp",
                    self.date
                );
                chrono::NaiveDateTime::default()
            })
            .and_utc()
            .timestamp()
    }

    pub fn hours(&self) -> Vec<Hour> {
        let mut hours = Vec::new();

        for hour in 0..24 {
            let hour = self.date.and_hms_opt(hour, 0, 0).unwrap_or_else(|| {
                tracing::error!("Invalid date '{}' - cannot convert to an hour", self.date);
                chrono::NaiveDateTime::default()
            });
            if let Ok(hour) = Hour::from_chrono(&hour) {
                hours.push(hour);
            }
        }

        hours
    }

    pub fn yesterday() -> Self {
        Self {
            date: Date::today().date - chrono::Duration::days(1),
        }
    }

    pub fn today() -> Self {
        Self {
            date: chrono::Local::now().naive_local().into(),
        }
    }

    pub fn tomorrow() -> Self {
        Self {
            date: Date::today().next().date,
        }
    }

    pub fn day(self: &Date) -> DateRange {
        DateRange {
            start_date: Date { date: self.date },
            end_date: Date { date: self.date },
        }
    }

    pub fn prev(self: &Date) -> Date {
        Date {
            date: self.date - chrono::Duration::days(1),
        }
    }

    pub fn next(self: &Date) -> Date {
        Date {
            date: self.date + chrono::Duration::days(1),
        }
    }

    pub fn week(self: &Date) -> DateRange {
        let start_date = match self.date.weekday() {
            chrono::Weekday::Mon => self.date,
            chrono::Weekday::Tue => self.date - chrono::Duration::days(1),
            chrono::Weekday::Wed => self.date - chrono::Duration::days(2),
            chrono::Weekday::Thu => self.date - chrono::Duration::days(3),
            chrono::Weekday::Fri => self.date - chrono::Duration::days(4),
            chrono::Weekday::Sat => self.date - chrono::Duration::days(5),
            chrono::Weekday::Sun => self.date - chrono::Duration::days(6),
        };

        let end_date = start_date + chrono::Duration::days(6);

        DateRange {
            start_date: Date { date: start_date },
            end_date: Date { date: end_date },
        }
    }

    pub fn prev_week(self: &Date) -> DateRange {
        let start_date = match self.date.weekday() {
            chrono::Weekday::Mon => self.date - chrono::Duration::days(7),
            chrono::Weekday::Tue => self.date - chrono::Duration::days(8),
            chrono::Weekday::Wed => self.date - chrono::Duration::days(9),
            chrono::Weekday::Thu => self.date - chrono::Duration::days(10),
            chrono::Weekday::Fri => self.date - chrono::Duration::days(11),
            chrono::Weekday::Sat => self.date - chrono::Duration::days(12),
            chrono::Weekday::Sun => self.date - chrono::Duration::days(13),
        };

        let end_date = start_date + chrono::Duration::days(6);

        DateRange {
            start_date: Date { date: start_date },
            end_date: Date { date: end_date },
        }
    }

    pub fn next_week(self: &Date) -> DateRange {
        let start_date = match self.date.weekday() {
            chrono::Weekday::Mon => self.date + chrono::Duration::days(7),
            chrono::Weekday::Tue => self.date + chrono::Duration::days(6),
            chrono::Weekday::Wed => self.date + chrono::Duration::days(5),
            chrono::Weekday::Thu => self.date + chrono::Duration::days(4),
            chrono::Weekday::Fri => self.date + chrono::Duration::days(3),
            chrono::Weekday::Sat => self.date + chrono::Duration::days(2),
            chrono::Weekday::Sun => self.date + chrono::Duration::days(1),
        };

        let end_date = start_date + chrono::Duration::days(6);

        DateRange {
            start_date: Date { date: start_date },
            end_date: Date { date: end_date },
        }
    }

    pub fn this_week() -> DateRange {
        Date::today().week()
    }

    pub fn month(self: &Date) -> DateRange {
        // note that all the unwraps are safe, as we are always working with
        // valid dates.

        let start_date = self.date.with_day(1).unwrap_or(self.date);

        let end_date =
            chrono::NaiveDate::from_ymd_opt(start_date.year(), start_date.month() + 1, 1)
                .unwrap_or(
                    chrono::NaiveDate::from_ymd_opt(start_date.year() + 1, 1, 1)
                        .unwrap_or(start_date),
                )
                .pred_opt()
                .unwrap_or(start_date);

        DateRange {
            start_date: Date { date: start_date },
            end_date: Date { date: end_date },
        }
    }

    pub fn prev_month(self: &Date) -> DateRange {
        // note that all the unwraps are safe, as we are always working with
        // valid dates.

        let end_of_last_month = self
            .date
            .with_day(1)
            .unwrap_or(self.date)
            .pred_opt()
            .unwrap_or(self.date);

        Date::from_chrono(&end_of_last_month).month()
    }

    pub fn next_month(self: &Date) -> DateRange {
        // note that all the unwraps are safe, as we are always working with
        // valid dates.
        let end_of_this_month = self
            .date
            .with_month(self.date.month() + 1)
            .unwrap_or(self.date)
            .with_day(1)
            .unwrap_or(self.date)
            .pred_opt()
            .unwrap_or(self.date);

        Date::from_chrono(&end_of_this_month.succ_opt().unwrap_or(self.date)).month()
    }

    pub fn this_month() -> DateRange {
        Date::today().month()
    }

    pub fn year(self: &Date) -> DateRange {
        // note that all the unwraps are safe, as we are always working with
        // valid dates.

        let start_date = self
            .date
            .with_month(1)
            .unwrap_or(self.date)
            .with_day(1)
            .unwrap_or(self.date);

        let end_date = chrono::NaiveDate::from_ymd_opt(start_date.year() + 1, 1, 1)
            .unwrap_or(start_date)
            .pred_opt()
            .unwrap_or(start_date);

        DateRange {
            start_date: Date { date: start_date },
            end_date: Date { date: end_date },
        }
    }

    pub fn prev_year(self: &Date) -> DateRange {
        // note that all the unwraps are safe, as we are always working with
        // valid dates.

        let end_of_last_year = self
            .date
            .with_month(1)
            .unwrap_or(self.date)
            .with_day(1)
            .unwrap_or(self.date)
            .pred_opt()
            .unwrap_or(self.date);

        Date::from_chrono(&end_of_last_year).year()
    }

    pub fn next_year(self: &Date) -> DateRange {
        // note that all the unwraps are safe, as we are always working with
        // valid dates.

        let end_of_this_year = self
            .date
            .with_year(self.date.year() + 1)
            .unwrap_or(self.date)
            .with_month(1)
            .unwrap_or(self.date)
            .with_day(1)
            .unwrap_or(self.date)
            .pred_opt()
            .unwrap_or(self.date);

        Date::from_chrono(&end_of_this_year.succ_opt().unwrap_or(self.date)).year()
    }

    pub fn this_year() -> DateRange {
        Date::today().year()
    }

    pub fn date(&self) -> &chrono::NaiveDate {
        &self.date
    }

    pub fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.date.partial_cmp(&other.date)
    }
}

impl std::fmt::Display for Date {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.date.format("%Y-%m-%d"))
    }
}

impl Serialize for Date {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Date {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl From<chrono::NaiveDate> for Date {
    fn from(date: chrono::NaiveDate) -> Self {
        Self { date }
    }
}

impl From<Date> for chrono::NaiveDate {
    fn from(date: Date) -> Self {
        date.date
    }
}

///
/// Struct used to parse a date range (from start to end inclusive)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DateRange {
    start_date: Date,
    end_date: Date,
}

impl NamedType for DateRange {
    fn type_name() -> String {
        "DateRange".to_string()
    }
}

impl DateRange {
    pub fn from_chrono(start_date: &chrono::NaiveDate, end_date: &chrono::NaiveDate) -> Self {
        match start_date < end_date {
            true => Self {
                start_date: Date { date: *start_date },
                end_date: Date { date: *end_date },
            },
            false => Self {
                start_date: Date { date: *end_date },
                end_date: Date { date: *start_date },
            },
        }
    }

    pub fn parse(date_range: &str) -> Result<Self, Error> {
        let date_range = date_range.trim().to_lowercase();

        if date_range.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid DateRange - cannot be empty '{}'",
                date_range
            )));
        };

        // some special cases
        match date_range.as_str() {
            "yesterday" => {
                return Ok(Date::yesterday().day());
            }
            "today" => {
                return Ok(Date::today().day());
            }
            "tomorrow" => {
                return Ok(Date::tomorrow().day());
            }
            "this_day" => {
                return Ok(Date::today().day());
            }
            "this_week" => {
                return Ok(Date::this_week());
            }
            "last_week" => {
                return Ok(Date::today().prev_week());
            }
            "this_month" => {
                return Ok(Date::this_month());
            }
            "last_month" => {
                return Ok(Date::today().prev_month());
            }
            "this_year" => {
                return Ok(Date::today().year());
            }
            "last_year" => {
                return Ok(Date::today().prev_year());
            }
            _ => {}
        }

        let parts: Vec<&str> = date_range.split(':').collect();

        let (start, end) = match parts.as_slice() {
            // a single date means the start and end date are the same
            [single] => (*single, *single),
            [start, end] => (*start, *end),
            _ => {
                return Err(Error::Parse(format!(
                    "Invalid DateRange - must contain two dates, separated by a colon '{}'",
                    date_range
                )));
            }
        };

        let start_date = Date::parse(start)?;
        let end_date = Date::parse(end)?;

        // Bound the *span*, not just the endpoints. `days()` builds one element
        // per calendar day and the report types aggregate per day/week/month
        // over it, so the span is what bounds how much work a single
        // instruction can ask an agent to do. See `MAX_DATE_RANGE_DAYS` and
        // `docs/specifications/security-review-2.md` (finding R25).
        let span = end_date
            .to_chrono()
            .signed_duration_since(start_date.to_chrono())
            .num_days();

        if span.abs() > MAX_DATE_RANGE_DAYS {
            return Err(Error::Parse(format!(
                "Invalid DateRange - span of {} days exceeds the maximum of {} '{}'",
                span.abs(),
                MAX_DATE_RANGE_DAYS,
                date_range
            )));
        }

        Ok(Self {
            start_date,
            end_date,
        })
    }

    pub fn start_date(&self) -> &Date {
        &self.start_date
    }

    pub fn end_date(&self) -> &Date {
        &self.end_date
    }

    // the start time is inclusive, i.e. [start_time, end_time)
    pub fn start_time(&self) -> chrono::NaiveDateTime {
        self.start_date
            .date
            .and_hms_opt(0, 0, 0)
            .unwrap_or_else(|| {
                tracing::error!(
                    "Invalid start date '{}' - cannot convert to a start_time",
                    self.start_date
                );
                chrono::NaiveDateTime::default()
            })
    }

    // the end time is exclusive, i.e. [start_time, end_time)
    pub fn end_time(&self) -> chrono::NaiveDateTime {
        // this will finish at midnight on the day after the end date,
        // as we have a half-open interval [start_time, end_time)
        let midnight = self.end_date.date.and_hms_opt(0, 0, 0).unwrap_or_else(|| {
            tracing::error!(
                "Invalid end date '{}' - cannot convert to an end_time",
                self.end_date
            );
            chrono::NaiveDateTime::default()
        });

        // Checked, so an end date at the top of the representable range cannot
        // panic here - see finding R25.
        midnight
            .checked_add_signed(chrono::Duration::days(1))
            .unwrap_or(midnight)
    }

    /// The day after `period_end`, or `None` if that cannot be represented or
    /// would not move the iteration forward from `current`.
    ///
    /// The forward-progress check is what makes `months()`/`years()` terminate.
    /// At the top of the representable range `from_ymd_opt(year + 1, 1, 1)`
    /// returns `None`, the `unwrap_or(start_date)` fallback then produced a
    /// `period_end` *earlier* than `current`, and `current = period_end + 1 day`
    /// therefore never advanced - an infinite loop that also pushed a
    /// `DateRange` into a `Vec` on every iteration. See
    /// `docs/specifications/security-review-2.md` (finding R25).
    fn advance_past(
        current: chrono::NaiveDate,
        period_end: chrono::NaiveDate,
    ) -> Option<chrono::NaiveDate> {
        let next = period_end.checked_add_signed(chrono::Duration::days(1))?;

        match next > current {
            true => Some(next),
            false => None,
        }
    }

    pub fn days(&self) -> Vec<Date> {
        let mut days = Vec::new();

        let mut current = self.start_date.date;
        while current <= self.end_date.date {
            days.push(Date { date: current });

            // Checked: `current + 1 day` overflows at the top of chrono's
            // representable range. `Date::parse` now bounds the year so this is
            // unreachable from a parsed range, but `from_chrono` takes a
            // `NaiveDate` directly. See finding R25.
            match current.checked_add_signed(chrono::Duration::days(1)) {
                Some(next) => current = next,
                None => break,
            }
        }

        days
    }

    pub fn weeks(&self) -> Vec<DateRange> {
        let mut weeks = Vec::new();

        let mut current = self.start_date.date;
        while current <= self.end_date.date {
            // Both the roll-back to Monday and the roll-forward to Sunday are
            // checked: at the very edges of chrono's representable range either
            // can overflow, which would abort the process under
            // `panic = "abort"`. See finding R25.
            let days_since_monday = current.weekday().num_days_from_monday() as i64;

            let Some(start_date) =
                current.checked_sub_signed(chrono::Duration::days(days_since_monday))
            else {
                break;
            };

            let Some(end_date) = start_date.checked_add_signed(chrono::Duration::days(6)) else {
                break;
            };

            weeks.push(DateRange {
                start_date: Date { date: start_date },
                end_date: Date { date: end_date },
            });

            match Self::advance_past(current, end_date) {
                Some(next) => current = next,
                None => break,
            }
        }

        weeks
    }

    pub fn months(&self) -> Vec<DateRange> {
        let mut months = Vec::new();

        let mut current = self.start_date.date;
        while current <= self.end_date.date {
            let start_date = current.with_day(1).unwrap_or(current);

            let end_date =
                chrono::NaiveDate::from_ymd_opt(start_date.year(), start_date.month() + 1, 1)
                    .unwrap_or(
                        chrono::NaiveDate::from_ymd_opt(start_date.year() + 1, 1, 1)
                            .unwrap_or(start_date),
                    )
                    .pred_opt()
                    .unwrap_or(start_date);

            months.push(DateRange {
                start_date: Date { date: start_date },
                end_date: Date { date: end_date },
            });

            match Self::advance_past(current, end_date) {
                Some(next) => current = next,
                None => break,
            }
        }

        months
    }

    pub fn years(&self) -> Vec<DateRange> {
        let mut years = Vec::new();

        let mut current = self.start_date.date;
        while current <= self.end_date.date {
            let start_date = current
                .with_month(1)
                .unwrap_or(current)
                .with_day(1)
                .unwrap_or(current);

            let end_date = chrono::NaiveDate::from_ymd_opt(start_date.year() + 1, 1, 1)
                .unwrap_or(start_date)
                .pred_opt()
                .unwrap_or(start_date);

            years.push(DateRange {
                start_date: Date { date: start_date },
                end_date: Date { date: end_date },
            });

            match Self::advance_past(current, end_date) {
                Some(next) => current = next,
                None => break,
            }
        }

        years
    }
}

impl std::fmt::Display for DateRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.start_date, self.end_date)
    }
}

/// Serialize and Deserialize via the string representation
/// of the Day
impl Serialize for DateRange {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DateRange {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

///
/// The template used by the portal to create the Project. This can be used
/// e.g. to specify that a project is for a particular type of
/// infrastructure (e.g. "cpu-cluster", "gpu-cluster" etc.).
/// The types available on a portal are controlled by the
/// portal administrator, and can be arbitrarily defined. Note
/// however that once a project has been created in a type,
/// it cannot be changed.
///
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectTemplate {
    /// The name of the template - this must not have any spaces
    /// or special characters
    name: String,
}

impl ProjectTemplate {
    pub fn parse(name: &str) -> Result<Self, Error> {
        let name = name.trim();

        if name.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid ProjectTemplate - cannot be empty '{}'",
                name
            )));
        };

        if name.contains(' ') {
            return Err(Error::Parse(format!(
                "Invalid ProjectTemplate - cannot contain spaces '{}'",
                name
            )));
        };

        // name can only be alphanumeric, underscores and dashes
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(Error::Parse(format!(
                "Invalid ProjectTemplate - can only contain alphanumeric characters, underscores and dashes '{}'",
                name
            )));
        };

        Ok(Self {
            name: name.to_string(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl NamedType for ProjectTemplate {
    fn type_name() -> String {
        "ProjectTemplate".to_string()
    }
}

impl std::fmt::Display for ProjectTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Serialize for ProjectTemplate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProjectTemplate {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        ProjectTemplate::parse(&s).map_err(serde::de::Error::custom)
    }
}

///
/// Details about a compute node
///
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// The number of CPUs in the node
    cpus: u32,

    /// The number of cores per cpu
    cores_per_cpu: u32,

    /// The number of GPUs in the node
    gpus: u32,

    /// The amount of memory in the node in MB
    memory_mb: u32,

    /// The total billing value of one node in billing units
    billing: u32,
}

impl std::fmt::Display for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Node(cpus: {}, cores_per_cpu: {}, gpus: {}, memory: {} GB, billing: {})",
            self.cpus,
            self.cores_per_cpu,
            self.gpus,
            self.memory_gb(),
            self.billing
        )
    }
}

impl Node {
    pub fn new() -> Self {
        Self {
            cpus: 0,
            cores_per_cpu: 0,
            gpus: 0,
            memory_mb: 0,
            billing: 0,
        }
    }

    pub fn construct(
        cpus: u32,
        cores_per_cpu: u32,
        gpus: u32,
        memory_mb: u32,
        billing: u32,
    ) -> Self {
        Self {
            cpus,
            cores_per_cpu,
            gpus,
            memory_mb,
            billing,
        }
    }

    pub fn cpus(&self) -> u32 {
        self.cpus
    }

    pub fn cores_per_cpu(&self) -> u32 {
        self.cores_per_cpu
    }

    pub fn cores(&self) -> u32 {
        self.cpus * self.cores_per_cpu
    }

    pub fn gpus(&self) -> u32 {
        self.gpus
    }

    pub fn memory_mb(&self) -> u32 {
        self.memory_mb
    }

    pub fn memory_gb(&self) -> f64 {
        self.memory_mb as f64 / 1024.0
    }

    pub fn billing(&self) -> u32 {
        self.billing
    }

    pub fn set_cpus(&mut self, cpus: u32) {
        self.cpus = cpus;
    }

    pub fn set_cores_per_cpu(&mut self, cores_per_cpu: u32) {
        self.cores_per_cpu = cores_per_cpu;
    }

    pub fn set_gpus(&mut self, gpus: u32) {
        self.gpus = gpus;
    }

    pub fn set_memory_mb(&mut self, memory_mb: u32) {
        self.memory_mb = memory_mb;
    }

    pub fn set_billing(&mut self, billing: u32) {
        self.billing = billing;
    }
}

impl NamedType for Node {
    fn type_name() -> String {
        "Node".to_string()
    }
}

///
/// Details about an allocation to a project. This combines the
/// size of the allocation plus the units of that allocation
///
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Allocation {
    /// The size of the allocation, e.g. "1000"
    size: Option<f64>,

    /// The units of the allocation, e.g. "NHR", "GPUh" etc.
    units: Option<String>,
}

impl Allocation {
    pub fn new() -> Self {
        Self {
            size: None,
            units: None,
        }
    }

    pub fn canonicalize(units: &str) -> String {
        let canonical = units.trim().to_lowercase();

        if canonical == "node hours" || canonical == "node hour" || canonical == "nhr" {
            return "NHR".to_string();
        } else if canonical == "gpu hours" || canonical == "gpu hour" || canonical == "gpuhr" {
            return "GPUHR".to_string();
        } else if canonical == "cpu hours" || canonical == "cpu hour" || canonical == "cpuhr" {
            return "CPUHR".to_string();
        } else if canonical == "core hours" || canonical == "core hour" || canonical == "corehr" {
            return "COREHR".to_string();
        } else if canonical == "gb hours" || canonical == "gb hour" || canonical == "gbhr" {
            return "GBHR".to_string();
        } else if canonical == "billing hours" || canonical == "billing hour" || canonical == "bhr"
        {
            return "BHR".to_string();
        }

        // Add more canonicalizations as needed
        canonical
    }

    pub fn from_size_and_units(size: f64, units: &str) -> Result<Self, Error> {
        // `!is_finite()` rather than only `< 0.0`: the negative test is *false*
        // for NaN, so `"NaN"` and `"inf"` both parsed cleanly and then
        // saturated to `u64::MAX` on the way into a `Usage` or `StorageSize`.
        // See docs/specifications/security-review-2.md (finding R33).
        if !size.is_finite() {
            return Err(Error::Parse(format!(
                "Invalid Allocation - size must be a finite number '{}'",
                size
            )));
        }

        if size < 0.0 {
            return Err(Error::Parse(format!(
                "Invalid Allocation - size cannot be negative '{}'",
                size
            )));
        }

        let units = units.trim();

        if units.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid Allocation - units cannot be empty '{}'",
                units
            )));
        }

        Ok(Self {
            size: Some(size),
            units: Some(Allocation::canonicalize(units)),
        })
    }

    pub fn parse(allocation: &str) -> Result<Self, Error> {
        let allocation = allocation.trim();

        if allocation.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid Allocation - cannot be empty '{}'",
                allocation
            )));
        };

        if allocation.to_lowercase() == "none" || allocation.to_lowercase() == "no allocation" {
            return Ok(Self::default());
        }

        let parts: Vec<&str> = allocation.split_whitespace().collect();

        // Split rather than indexed, so "no size" and "no units" are parse
        // errors by construction - see
        // docs/specifications/security-review-2.md (finding R1).
        let Some((size_part, unit_parts)) = parts.split_first().filter(|(_, u)| !u.is_empty())
        else {
            return Err(Error::Parse(format!(
                "Invalid Allocation - must contain a size and units '{}'",
                allocation
            )));
        };

        let size = size_part.parse::<f64>().map_err(|_| {
            Error::Parse(format!(
                "Invalid Allocation - size must be a number '{}'",
                size_part
            ))
        })?;

        // f64::from_str accepts "NaN", "inf" and "infinity", and the negative
        // test below is *false* for NaN, so both used to parse cleanly and then
        // saturate to u64::MAX downstream. See
        // docs/specifications/security-review-2.md (finding R33).
        if !size.is_finite() {
            return Err(Error::Parse(format!(
                "Invalid Allocation - size must be a finite number '{}'",
                size_part
            )));
        }

        if size < 0.0 {
            return Err(Error::Parse(format!(
                "Invalid Allocation - size cannot be negative '{}'",
                size
            )));
        }

        let units = {
            let u = unit_parts.join(" ");
            let u = u.trim();

            if u.is_empty() {
                return Err(Error::Parse(format!(
                    "Invalid Allocation - units cannot be empty '{}'",
                    allocation
                )));
            }

            u.to_string()
        };

        Ok(Self {
            size: Some(size),
            units: Some(Allocation::canonicalize(&units)),
        })
    }

    pub fn size(&self) -> Option<f64> {
        self.size
    }

    pub fn units(&self) -> Option<String> {
        self.units.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.size.is_none()
    }

    pub fn is_node_hours(&self) -> bool {
        if let Some(units) = &self.units {
            units == "NHR"
        } else {
            false
        }
    }

    pub fn is_gpu_hours(&self) -> bool {
        if let Some(units) = &self.units {
            units == "GPUHR"
        } else {
            false
        }
    }

    pub fn is_cpu_hours(&self) -> bool {
        if let Some(units) = &self.units {
            units == "CPUHR"
        } else {
            false
        }
    }

    pub fn is_core_hours(&self) -> bool {
        if let Some(units) = &self.units {
            units == "COREHR"
        } else {
            false
        }
    }

    pub fn is_gb_hours(&self) -> bool {
        if let Some(units) = &self.units {
            units == "GBHR"
        } else {
            false
        }
    }

    pub fn is_billing_hours(&self) -> bool {
        if let Some(units) = &self.units {
            units == "BHR"
        } else {
            false
        }
    }
}

impl NamedType for Allocation {
    fn type_name() -> String {
        "Allocation".to_string()
    }
}

impl std::fmt::Display for Allocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(size) = self.size {
            if let Some(units) = &self.units {
                write!(f, "{} {}", size, units)
            } else {
                write!(f, "{}", size)
            }
        } else {
            write!(f, "No allocation")
        }
    }
}

impl Serialize for Allocation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Allocation {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Allocation::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// Validates that a string is a well-formed email address (local@domain).
pub(crate) fn validate_email_address(email: &str) -> Result<(), Error> {
    let mut parts = email.splitn(2, '@');
    let local = parts
        .next()
        .ok_or_else(|| Error::Parse("Invalid email address".to_string()))?;
    let domain = parts
        .next()
        .ok_or_else(|| Error::Parse("Email address must contain '@'".to_string()))?;

    if local.is_empty() {
        return Err(Error::Parse("Email local part cannot be empty".to_string()));
    }
    if !local
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-'))
    {
        return Err(Error::Parse(format!(
            "Email local part '{}' contains invalid characters",
            local
        )));
    }
    if domain.contains('@') {
        return Err(Error::Parse(
            "Email address must contain exactly one '@'".to_string(),
        ));
    }
    DomainPattern::validate_domain_name(domain)
}

/// A domain pattern - this can be used to match domains that are allowed / denied
/// Supports exact matches (e.g., "example.com") and wildcard matches (e.g., "*.example.com")
/// Serializes to/from JSON as a plain string (e.g., "*.example.com")
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DomainPattern {
    /// The pattern string to match a domain or a specific email address.
    /// Domain forms: "example.com" (exact) or "*.example.com" (wildcard subdomain).
    /// Email form: "chris@example.com" (exact, case-insensitive).
    pattern: String,
}

impl NamedType for DomainPattern {
    fn type_name() -> String {
        "DomainPattern".to_string()
    }
}

impl DomainPattern {
    pub fn parse(pattern: &str) -> Result<Self, Error> {
        if pattern.is_empty() {
            return Err(Error::Parse("Domain pattern cannot be empty".to_string()));
        }

        if pattern.contains('@') {
            Self::validate_email(pattern)?;
        } else if pattern.starts_with("*.") {
            let domain_part = pattern
                .strip_prefix("*.")
                .ok_or_else(|| Error::Parse("Invalid wildcard pattern".to_string()))?;
            if domain_part.is_empty() {
                return Err(Error::Parse(
                    "Wildcard pattern must have a domain after '*.'".to_string(),
                ));
            }
            if domain_part.contains('*') {
                return Err(Error::Parse(
                    "Wildcard '*' can only appear at the start of the pattern".to_string(),
                ));
            }
            Self::validate_domain_name(domain_part)?;
        } else {
            if pattern.contains('*') {
                return Err(Error::Parse(
                    "Wildcard '*' can only appear at the start as '*.'".to_string(),
                ));
            }
            Self::validate_domain_name(pattern)?;
        }

        Ok(Self {
            pattern: pattern.to_string(),
        })
    }

    fn validate_email(email: &str) -> Result<(), Error> {
        validate_email_address(email)
    }

    /// Returns true if this pattern is a specific email address rather than a domain pattern.
    pub fn is_email_pattern(&self) -> bool {
        self.pattern.contains('@')
    }

    /// Validates that a domain name contains only valid characters
    fn validate_domain_name(domain: &str) -> Result<(), Error> {
        if domain.is_empty() {
            return Err(Error::Parse("Domain name cannot be empty".to_string()));
        }

        // Domain names can contain letters, digits, hyphens, and dots
        // Each label (part between dots) must start and end with alphanumeric
        for label in domain.split('.') {
            if label.is_empty() {
                return Err(Error::Parse(
                    "Domain name cannot have empty labels (e.g., '..', '.com')".to_string(),
                ));
            }

            if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(Error::Parse(
                    format!(
                        "Domain label '{}' contains invalid characters (only letters, digits, and hyphens allowed)",
                        label
                    ),
                ));
            }

            if label.starts_with('-') || label.ends_with('-') {
                return Err(Error::Parse(format!(
                    "Domain label '{}' cannot start or end with a hyphen",
                    label
                )));
            }
        }

        Ok(())
    }

    pub fn pattern(&self) -> String {
        self.pattern.clone()
    }

    /// Tests if a concrete domain matches this pattern. Only valid for domain patterns;
    /// always returns false for email patterns.
    /// - Exact pattern (e.g., "example.com"): only exact match returns true
    /// - Wildcard pattern (e.g., "*.example.com"): matches any subdomain at any depth
    pub fn matches(&self, domain: &str) -> bool {
        if self.is_email_pattern() {
            return false;
        }
        WildMatch::new(&self.pattern.to_lowercase()).matches(&domain.to_lowercase())
    }

    /// Tests if a concrete email address matches this pattern. Only valid for email patterns;
    /// always returns false for domain patterns. Match is case-insensitive.
    pub fn matches_email(&self, email: &str) -> bool {
        if !self.is_email_pattern() {
            return false;
        }
        self.pattern.to_lowercase() == email.to_lowercase()
    }
}

impl Serialize for DomainPattern {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize as a plain string
        serializer.serialize_str(&self.pattern)
    }
}

impl<'de> Deserialize<'de> for DomainPattern {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Deserialize from a string and validate it
        let pattern = String::deserialize(deserializer)?;
        DomainPattern::parse(&pattern).map_err(serde::de::Error::custom)
    }
}

/// A reference to an external resource: an optional human-readable ID
/// and an optional URL. Used for award, call, project, and renewal links
/// inside AwardDetails.
#[derive(Debug, Default, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct Link {
    /// Human-readable identifier, e.g. "EP/X000000/1" or "061-4738952-1"
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    id: Option<String>,

    /// URL pointing to the resource (must be a valid URL if provided)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    url: Option<String>,
}

impl Link {
    pub fn new() -> Self {
        Self {
            id: None,
            url: None,
        }
    }

    pub fn id(&self) -> Option<String> {
        self.id.clone()
    }

    pub fn set_id(&mut self, id: &str) {
        let id = id.trim();
        if id.is_empty() {
            self.id = None;
        } else {
            self.id = Some(id.to_string());
        }
    }

    pub fn clear_id(&mut self) {
        self.id = None;
    }

    pub fn url(&self) -> Option<String> {
        self.url.clone()
    }

    pub fn set_url(&mut self, url: &str) -> Result<(), Error> {
        let url = url.trim();
        if url.is_empty() {
            self.url = None;
            Ok(())
        } else {
            let parsed = Url::parse(url)
                .map_err(|e| Error::Parse(format!("Invalid URL for link: {}", e)))?;

            // `Url::parse` accepts `javascript:`, `data:` and `file:`. These links are
            // documented as being for display in a portal UI, so if any consumer
            // renders one as an anchor those schemes are a stored-XSS or
            // local-file-read primitive. Restrict to the two schemes a link in a web
            // UI can legitimately need. See
            // `docs/specifications/security-review-2.md` (finding R33).
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(Error::Parse(format!(
                    "Invalid URL for link: scheme '{}' is not allowed - only http and \
                     https are, because these links are intended to be rendered in a \
                     portal UI",
                    parsed.scheme()
                )));
            }

            self.url = Some(url.to_string());
            Ok(())
        }
    }

    pub fn clear_url(&mut self) {
        self.url = None;
    }

    pub fn is_empty(&self) -> bool {
        self.id.is_none() && self.url.is_none()
    }
}

impl std::fmt::Display for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", serde_json::to_string(self).unwrap_or_default())
    }
}

impl<'de> Deserialize<'de> for Link {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct LinkHelper {
            id: Option<String>,
            url: Option<String>,
        }

        let helper = LinkHelper::deserialize(deserializer)?;

        // Route through `set_url` rather than re-validating here, so the wire path and
        // the programmatic path cannot drift - this copy of the validation used to be
        // a plain `Url::parse` and so did not gain the scheme allow-list. See
        // `docs/specifications/security-review-2.md` (finding R33).
        let mut link = Link {
            id: helper.id,
            url: None,
        };

        if let Some(url) = &helper.url {
            link.set_url(url).map_err(serde::de::Error::custom)?;
        }

        Ok(link)
    }
}

/// A timestamped note attached to an award. Notes are append-only
/// messages, typically used by the awarding portal to communicate
/// with the project team.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Note {
    /// When the note was created (UTC)
    timestamp: DateTime<Utc>,

    /// Name of the person who created the note
    author: String,

    /// Free-text content of the note
    text: String,
}

impl Note {
    pub fn new(author: &str, text: &str) -> Self {
        Self {
            timestamp: Utc::now(),
            author: author.to_string(),
            text: text.to_string(),
        }
    }

    pub fn with_timestamp(timestamp: DateTime<Utc>, author: &str, text: &str) -> Self {
        Self {
            timestamp,
            author: author.to_string(),
            text: text.to_string(),
        }
    }

    pub fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp
    }

    pub fn author(&self) -> &str {
        &self.author
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

impl std::fmt::Display for Note {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{} — {}] {}",
            self.timestamp.format("%Y-%m-%d %H:%M UTC"),
            self.author,
            self.text
        )
    }
}

/// Controls whether the receiving portal may independently modify the membership
/// or roles of a project.
///
/// When this field is absent (`None` on `AwardDetails`) the behaviour is
/// identical to `Open` — the receiving portal manages membership freely.
/// Explicitly setting a value lets the sending portal declare a policy that
/// the receiving portal is expected to honour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MembershipControl {
    /// Receiving portal may freely add/remove members and change roles (default
    /// when field is absent).
    Open,
    /// Receiving portal may add or remove members, but must not change the
    /// role of any member — roles are authoritative in `AwardDetails`.
    MembersOnly,
    /// Receiving portal may change the role of existing members, but must not
    /// add new members or remove existing ones.
    RolesOnly,
    /// Receiving portal must not change membership or roles; both are
    /// authoritative in `AwardDetails` updates from the sender.
    Locked,
}

impl MembershipControl {
    /// Returns `true` if the receiving portal may add or remove members.
    pub fn can_change_membership(&self) -> bool {
        matches!(self, Self::Open | Self::MembersOnly)
    }

    /// Returns `true` if the receiving portal may change the role of a member.
    pub fn can_change_roles(&self) -> bool {
        matches!(self, Self::Open | Self::RolesOnly)
    }
}

impl std::fmt::Display for MembershipControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::MembersOnly => write!(f, "members_only"),
            Self::RolesOnly => write!(f, "roles_only"),
            Self::Locked => write!(f, "locked"),
        }
    }
}

/// Details about a project that exists in a portal.
/// This holds all data as "option" as not all details
/// will be set by all portals. Also, using "option" allows
/// this struct to be used in "update" requests, as only
/// the fields that are set will be updated.
///
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AwardDetails {
    /// The name of the project
    name: Option<String>,

    /// The template used for the project
    #[ts(as = "Option<String>")]
    template: Option<ProjectTemplate>,

    /// The key that may need to be provided to show that the
    /// project is really allowed to access a particular type
    /// of project (i.e. it may be very easy to guess an allowed
    /// template name, but it would not be easy to guess the
    /// associated key)
    key: Option<String>,

    /// The description of the project
    description: Option<String>,

    /// The email address(es) of the members of the project,
    /// (keys) and their roles (values).
    members: Option<BTreeMap<String, String>>,

    /// Proposed start date of the project (ISO 8601 date string)
    #[ts(as = "Option<String>")]
    start_date: Option<Date>,

    /// Proposed end date of the project (ISO 8601 date string)
    #[ts(as = "Option<String>")]
    end_date: Option<Date>,

    /// The allocation of resource for this project (e.g. "1000 NHR")
    #[ts(as = "Option<String>")]
    allocation: Option<Allocation>,

    /// A free-form breakdown of the allocation into named components.
    /// Keys and values are arbitrary strings agreed between the local
    /// and remote portals — OpenPortal does not interpret them.
    /// e.g. {"project_storage": "5 TB", "gpu_hours": "500 GPUHR"}
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    breakdown: BTreeMap<String, String>,

    /// Link back to the award record on the funding body's system
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    award: Option<Link>,

    /// Link to the funding call from which the award was made
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    call: Option<Link>,

    /// Link to the project page on the remote/awarding portal
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    project_link: Option<Link>,

    /// Link to the page where more time / renewal can be requested
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    renewal: Option<Link>,

    /// Notes attached to this award (append-only log of messages)
    #[serde(default)]
    notes: Vec<Note>,

    /// The earliest UTC time at which this award may be approved on the
    /// receiving portal. Lets the awarder make corrections in the window
    /// between creating the award and it being provisioned.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    earliest_approve: Option<DateTime<Utc>>,

    /// Controls whether the receiving portal may independently modify
    /// membership or roles. When absent, behaviour is `Open`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    membership_control: Option<MembershipControl>,

    /// The list of allowed domains for this project.
    /// If this is None, then all domains are allowed.
    /// If this is Some(vec![]), then no domains are allowed.
    /// If this is Some(vec![...]), then only the domains that match
    /// those in the list are allowed.
    #[ts(as = "Option<Vec<String>>")]
    allowed_domains: Option<Vec<DomainPattern>>,
}

impl NamedType for AwardDetails {
    fn type_name() -> String {
        "ProjectDetails".to_string()
    }
}

impl AwardDetails {
    pub fn new() -> Self {
        Self {
            name: None,
            template: None,
            key: None,
            description: None,
            members: None,
            start_date: None,
            end_date: None,
            allocation: None,
            breakdown: BTreeMap::new(),
            award: None,
            call: None,
            project_link: None,
            renewal: None,
            notes: Vec::new(),
            earliest_approve: None,
            membership_control: None,
            allowed_domains: None,
        }
    }

    pub fn parse(json: &str) -> Result<Self, Error> {
        AwardDetails::from_json(json)
    }

    pub fn from_json(json: &str) -> Result<Self, Error> {
        serde_json::from_str(json).map_err(|e| Error::Parse(e.to_string()))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn name(&self) -> Option<String> {
        self.name.clone()
    }

    pub fn set_name(&mut self, name: &str) {
        let name = name.trim();

        if name.is_empty() {
            self.name = None;
        } else {
            self.name = Some(name.to_string());
        }
    }

    pub fn clear_name(&mut self) {
        self.name = None;
    }

    pub fn template(&self) -> Option<ProjectTemplate> {
        self.template.clone()
    }

    pub fn set_template(&mut self, template: ProjectTemplate) {
        self.template = Some(template);
    }

    pub fn clear_template(&mut self) {
        self.template = None;
    }

    pub fn key(&self) -> Option<String> {
        self.key.clone()
    }

    pub fn set_key(&mut self, key: &str) {
        let key = key.trim();

        if key.is_empty() {
            self.key = None;
        } else {
            self.key = Some(key.to_string());
        }
    }

    pub fn clear_key(&mut self) {
        self.key = None;
    }

    pub fn description(&self) -> Option<String> {
        self.description.clone()
    }

    pub fn set_description(&mut self, description: &str) {
        let description = description.trim();

        if description.is_empty() {
            self.description = None;
        } else {
            self.description = Some(description.to_string());
        }
    }

    pub fn clear_description(&mut self) {
        self.description = None;
    }

    pub fn members(&self) -> Option<BTreeMap<String, String>> {
        self.members.clone()
    }

    /// Validates a single (email, role) pair against the allowed-domains list.
    fn validate_member(&self, email: &str, role: &str) -> Result<(), Error> {
        if role.is_empty() {
            return Err(Error::Parse("Member role cannot be empty".to_string()));
        }
        validate_email_address(email)?;
        if !self.is_email_allowed(email) {
            return Err(Error::Parse(format!(
                "Email '{}' is not in the allowed domains for this project",
                email
            )));
        }
        Ok(())
    }

    pub fn add_member(&mut self, email: &str, role: &str) -> Result<(), Error> {
        let email = email.trim();
        let role = role.trim();
        self.validate_member(email, role)?;
        let members = self.members.get_or_insert_with(BTreeMap::new);
        members.insert(email.to_string(), role.to_string());
        Ok(())
    }

    /// Validates and adds all members in `new_members` without replacing existing ones.
    /// All entries are validated before any are applied; if any entry is invalid the
    /// existing members are left unchanged.
    pub fn add_members(&mut self, new_members: BTreeMap<String, String>) -> Result<(), Error> {
        for (email, role) in &new_members {
            self.validate_member(email.trim(), role.trim())?;
        }
        let members = self.members.get_or_insert_with(BTreeMap::new);
        for (email, role) in new_members {
            members.insert(email, role);
        }
        Ok(())
    }

    pub fn remove_member(&mut self, email: &str) {
        let email = email.trim();

        if email.is_empty() {
            tracing::warn!("Invalid ProjectDetails - email cannot be empty");
            return;
        };

        if let Some(members) = &mut self.members {
            members.remove(email);
        }
    }

    /// Validates and replaces all members atomically. All entries are validated before
    /// any changes are made; if any entry is invalid the existing members are unchanged.
    pub fn set_members(&mut self, members: BTreeMap<String, String>) -> Result<(), Error> {
        for (email, role) in &members {
            self.validate_member(email.trim(), role.trim())?;
        }
        if members.is_empty() {
            self.members = None;
        } else {
            self.members = Some(members);
        }
        Ok(())
    }

    pub fn clear_members(&mut self) {
        self.members = None;
    }

    pub fn start_date(&self) -> Option<Date> {
        self.start_date.clone()
    }

    pub fn set_start_date(&mut self, start_date: Date) {
        self.start_date = Some(start_date)
    }

    pub fn clear_start_date(&mut self) {
        self.start_date = None;
    }

    pub fn end_date(&self) -> Option<Date> {
        self.end_date.clone()
    }

    pub fn set_end_date(&mut self, end_date: Date) {
        self.end_date = Some(end_date)
    }

    pub fn clear_end_date(&mut self) {
        self.end_date = None;
    }

    pub fn allocation(&self) -> Option<Allocation> {
        self.allocation.clone()
    }

    pub fn set_allocation(&mut self, allocation: Allocation) {
        if allocation.is_empty() {
            self.allocation = None;
        } else {
            self.allocation = Some(allocation);
        }
    }

    pub fn clear_allocation(&mut self) {
        self.allocation = None;
    }

    pub fn breakdown(&self) -> &BTreeMap<String, String> {
        &self.breakdown
    }

    pub fn set_breakdown_entry(&mut self, key: &str, value: &str) {
        self.breakdown.insert(key.to_string(), value.to_string());
    }

    pub fn remove_breakdown_entry(&mut self, key: &str) {
        self.breakdown.remove(key);
    }

    pub fn set_breakdown(&mut self, breakdown: BTreeMap<String, String>) {
        self.breakdown = breakdown;
    }

    pub fn clear_breakdown(&mut self) {
        self.breakdown.clear();
    }

    pub fn award(&self) -> Option<Link> {
        self.award.clone()
    }

    pub fn set_award(&mut self, link: Link) {
        if link.is_empty() {
            self.award = None;
        } else {
            self.award = Some(link);
        }
    }

    pub fn clear_award(&mut self) {
        self.award = None;
    }

    pub fn call(&self) -> Option<Link> {
        self.call.clone()
    }

    pub fn set_call(&mut self, link: Link) {
        if link.is_empty() {
            self.call = None;
        } else {
            self.call = Some(link);
        }
    }

    pub fn clear_call(&mut self) {
        self.call = None;
    }

    pub fn project_link(&self) -> Option<Link> {
        self.project_link.clone()
    }

    pub fn set_project_link(&mut self, link: Link) {
        if link.is_empty() {
            self.project_link = None;
        } else {
            self.project_link = Some(link);
        }
    }

    pub fn clear_project_link(&mut self) {
        self.project_link = None;
    }

    pub fn renewal(&self) -> Option<Link> {
        self.renewal.clone()
    }

    pub fn set_renewal(&mut self, link: Link) {
        if link.is_empty() {
            self.renewal = None;
        } else {
            self.renewal = Some(link);
        }
    }

    pub fn clear_renewal(&mut self) {
        self.renewal = None;
    }

    pub fn notes(&self) -> &[Note] {
        &self.notes
    }

    pub fn add_note(&mut self, note: Note) {
        self.notes.push(note);
    }

    pub fn clear_notes(&mut self) {
        self.notes.clear();
    }

    pub fn earliest_approve(&self) -> Option<DateTime<Utc>> {
        self.earliest_approve
    }

    pub fn set_earliest_approve(&mut self, dt: DateTime<Utc>) {
        self.earliest_approve = Some(dt);
    }

    pub fn clear_earliest_approve(&mut self) {
        self.earliest_approve = None;
    }

    /// Returns the effective membership control policy. When the field is
    /// absent the policy is `Open` (receiving portal manages freely).
    pub fn membership_control(&self) -> MembershipControl {
        self.membership_control
            .clone()
            .unwrap_or(MembershipControl::Open)
    }

    pub fn set_membership_control(&mut self, control: Option<MembershipControl>) {
        self.membership_control = control;
    }

    pub fn clear_membership_control(&mut self) {
        self.membership_control = None;
    }

    /// Returns `true` if the receiving portal may add or remove members.
    /// Equivalent to `self.membership_control().can_change_membership()`.
    pub fn can_change_membership(&self) -> bool {
        self.membership_control().can_change_membership()
    }

    /// Returns `true` if the receiving portal may change the role of a member.
    /// Equivalent to `self.membership_control().can_change_roles()`.
    pub fn can_change_roles(&self) -> bool {
        self.membership_control().can_change_roles()
    }

    pub fn allowed_domains(&self) -> Option<Vec<DomainPattern>> {
        self.allowed_domains.clone()
    }

    pub fn add_allowed_domain(&mut self, domain: DomainPattern) {
        let domains = self.allowed_domains.get_or_insert_with(Vec::new);
        if !domains.contains(&domain) {
            domains.push(domain);
        }
    }

    pub fn set_allowed_domains(&mut self, domains: Vec<DomainPattern>) {
        if domains.is_empty() {
            self.allowed_domains = None;
        } else {
            self.allowed_domains = Some(domains);
        }
    }

    pub fn clear_allowed_domains(&mut self) {
        self.allowed_domains = None;
    }

    /// Returns true if the given domain is permitted by the allowed-domains list.
    /// Email patterns in the list are ignored — use `is_email_allowed` for full email checks.
    pub fn is_domain_allowed(&self, domain: &str) -> bool {
        if let Some(allowed_domains) = &self.allowed_domains {
            if allowed_domains.is_empty() {
                return false;
            }

            for d in allowed_domains {
                if d.matches(domain) {
                    return true;
                }
            }

            false
        } else {
            true
        }
    }

    /// Returns true if the given email address is permitted by the allowed-domains list.
    /// An email is permitted when the list is absent (no restriction), or when at least one
    /// entry in the list matches: either an exact email pattern matches the full address, or
    /// a domain pattern matches the domain part of the address.
    pub fn is_email_allowed(&self, email: &str) -> bool {
        let Some(allowed_domains) = &self.allowed_domains else {
            return true;
        };

        if allowed_domains.is_empty() {
            return false;
        }

        let domain_part = email.split_once('@').map(|x| x.1).unwrap_or("");

        for d in allowed_domains {
            if d.matches_email(email) {
                return true;
            }
            if !domain_part.is_empty() && d.matches(domain_part) {
                return true;
            }
        }

        false
    }

    pub fn merge(&self, other: &AwardDetails) -> Result<AwardDetails, Error> {
        let mut merged = self.clone();

        if merged.template.is_none() {
            merged.template = other.template.clone();
        } else if other.template.is_some() && merged.template != other.template {
            let this_template: String = merged
                .template
                .as_ref()
                .map(|t| t.name().to_string())
                .unwrap_or_default();
            let other_template: String = other
                .template
                .as_ref()
                .map(|t| t.name().to_string())
                .unwrap_or_default();

            tracing::error!(
                "Cannot merge project details with different project templates: '{}' != '{}'",
                this_template,
                other_template
            );

            return Err(Error::Parse(format!(
                "Cannot merge project details with different project templates: '{}' != '{}'",
                this_template, other_template
            )));
        }

        if other.name.is_some() {
            merged.name = other.name.clone();
        }

        if other.description.is_some() {
            merged.description = other.description.clone();
        }

        if other.start_date.is_some() {
            merged.start_date = other.start_date.clone();
        }

        if other.end_date.is_some() {
            merged.end_date = other.end_date.clone();
        }

        if other.allocation.is_some() {
            merged.allocation = other.allocation.clone();
        }

        for (key, value) in &other.breakdown {
            merged.breakdown.insert(key.clone(), value.clone());
        }

        if other.members.is_some() {
            merged.members = other.members.clone();
        }

        if other.key.is_some() {
            merged.key = other.key.clone();
        }

        if other.award.is_some() {
            merged.award = other.award.clone();
        }

        if other.call.is_some() {
            merged.call = other.call.clone();
        }

        if other.project_link.is_some() {
            merged.project_link = other.project_link.clone();
        }

        if other.renewal.is_some() {
            merged.renewal = other.renewal.clone();
        }

        // Merge notes: append notes from other that are not already present
        for note in &other.notes {
            if !merged.notes.contains(note) {
                merged.notes.push(note.clone());
            }
        }
        merged.notes.sort_by_key(|n| n.timestamp);

        if other.earliest_approve.is_some() {
            merged.earliest_approve = other.earliest_approve;
        }

        if other.membership_control.is_some() {
            merged.membership_control = other.membership_control.clone();
        }

        if other.allowed_domains.is_some() {
            if self.allowed_domains.is_none() {
                merged.allowed_domains = other.allowed_domains.clone();
            } else {
                let mut domains = self.allowed_domains.clone().unwrap_or_default();
                let other_domains = other.allowed_domains.clone().unwrap_or_default();

                for domain in other_domains {
                    if !domains.contains(&domain) {
                        domains.push(domain);
                    }
                }

                merged.allowed_domains = Some(domains);
            }
        }

        Ok(merged)
    }
}

impl std::fmt::Display for AwardDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_json())
    }
}

/// ProjectDetails is an alias for AwardDetails for backward compatibility.
/// New code should use AwardDetails directly.
pub type ProjectDetails = AwardDetails;

///
/// Enum of all of the instructions that can be sent to agents
///
#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    /// An instruction to submit a job to the portal
    Submit(Destination, Arc<Instruction>),

    /// An instruction to create a project in a portal
    CreateProject(ProjectIdentifier, ProjectDetails),

    /// An instruction to update a project in a portal
    UpdateProject(ProjectIdentifier, ProjectDetails),

    /// An instruction to get the details of a single project
    GetProject(ProjectIdentifier),

    /// An instruction to get all projects managed by a portal
    GetProjects(PortalIdentifier),

    /// An instruction to get the award details for a single project
    GetAward(ProjectIdentifier),

    /// An instruction to get the award details for all projects managed by a portal
    GetAwards(PortalIdentifier),

    /// An instruction to add a project
    AddProject(ProjectIdentifier),

    /// An instruction to remove a project
    RemoveProject(ProjectIdentifier),

    /// An instruction to get all users in a project
    GetUsers(ProjectIdentifier),

    /// An instruction to check if a user is protected from being
    /// managed by OpenPortal
    IsProtectedUser(UserIdentifier),

    /// An instruction to check if a user exists
    IsExistingUser(UserIdentifier),

    /// An instruction to check if a project exists
    IsExistingProject(ProjectIdentifier),

    /// An instruction to add a user
    AddUser(UserIdentifier),

    /// An instruction to remove a user
    RemoveUser(UserIdentifier),

    /// An instruction to block a user from logging in without removing their
    /// account, home directory, or scheduler configuration
    BlockUser(UserIdentifier),

    /// An instruction to unblock a previously blocked user, re-enabling login
    UnblockUser(UserIdentifier),

    /// An instruction to check if a user is blocked
    IsBlockedUser(UserIdentifier),

    /// An instruction to block all users in a project
    BlockProject(ProjectIdentifier),

    /// An instruction to unblock all users in a project
    UnblockProject(ProjectIdentifier),

    /// An instruction to check if all users in a project are blocked
    IsBlockedProject(ProjectIdentifier),

    /// An instruction to look up the mapping for a user
    GetUserMapping(UserIdentifier),

    /// An instruction to look up the mapping for a project
    GetProjectMapping(ProjectIdentifier),

    /// An instruction to look up the path to the home directory
    /// for a user - note this may not yet exist
    GetHomeDir(UserIdentifier),

    /// An instruction to look up the paths to the user directories
    /// for a user - not that these may not yet exist
    GetUserDirs(UserIdentifier),

    /// An instruction to look up the paths to the project directories
    /// for a project - not that these may not yet exist
    GetProjectDirs(ProjectIdentifier),

    /// An instruction to add a local user
    AddLocalUser(UserMapping),

    /// An instruction to remove a local user
    RemoveLocalUser(UserMapping),

    /// An instruction to add a local project
    AddLocalProject(ProjectMapping),

    /// An instruction to remove a local project
    RemoveLocalProject(ProjectMapping),

    /// An instruction to get a local project report
    GetLocalUsageReport(ProjectMapping, DateRange),

    /// An instruction to get the limit of a local project
    GetLocalLimit(ProjectMapping),

    /// An instruction to set the limit of a local project
    SetLocalLimit(ProjectMapping, Usage),

    /// An instruction to clear the quota of a local project on a volume
    ClearLocalProjectQuota(ProjectMapping, Volume),

    /// An instruction to set the quota of a local project on a volume
    SetLocalProjectQuota(ProjectMapping, Volume, QuotaLimit),

    /// An instruction to get the quota of a local project on a volume
    GetLocalProjectQuota(ProjectMapping, Volume),

    /// An instruction to get all quotas of a local project
    GetLocalProjectQuotas(ProjectMapping),

    /// An instruction to clear the quota of a local user on a volume
    ClearLocalUserQuota(UserMapping, Volume),

    /// An instruction to set the quota of a local user on a volume
    SetLocalUserQuota(UserMapping, Volume, QuotaLimit),

    /// An instruction to get the quota of a local user on a volume
    GetLocalUserQuota(UserMapping, Volume),

    /// An instruction to get all quotas of a local user
    GetLocalUserQuotas(UserMapping),

    /// Return the home directory of a local user
    /// (note this does not guarantee the directory exists)
    GetLocalHomeDir(UserMapping),

    /// Return the user directories of a local user
    /// (note this does not guarantee the directories exist)
    GetLocalUserDirs(UserMapping),

    /// Return the project directories of a local project
    /// (note this does not guarantee the directories exist)
    GetLocalProjectDirs(ProjectMapping),

    /// An instruction to update the home directory of a user
    UpdateHomeDir(UserIdentifier, String),

    /// An instruction to get the local storage report for a project
    /// from the filesystem agent in the specified date range (defaults to today)
    GetLocalStorageReport(ProjectMapping, DateRange),

    /// An instruction to get the storage report for a single
    /// project in the specified date range (defaults to today)
    GetStorageReport(ProjectIdentifier, DateRange),

    /// An instruction to get the storage reports for all active
    /// projects associated with a portal in the specified date range
    /// (defaults to today)
    GetStorageReports(PortalIdentifier, DateRange),

    /// An instruction to get the usage report for a single
    /// project in the specified date range
    GetUsageReport(ProjectIdentifier, DateRange),

    /// An instruction to get the usage report for all active
    /// projects associated with a portal in the specified
    /// date range
    GetUsageReports(PortalIdentifier, DateRange),

    /// An instruction to set the usage limit for a project
    SetLimit(ProjectIdentifier, Usage),

    /// An instruction to get the usage limit for a project
    GetLimit(ProjectIdentifier),

    /// An instruction to clear a storage quota for a project on a volume
    ClearProjectQuota(ProjectIdentifier, Volume),

    /// An instruction to set a storage quota for a project on a volume
    SetProjectQuota(ProjectIdentifier, Volume, QuotaLimit),

    /// An instruction to get the storage quota for a project on a volume
    GetProjectQuota(ProjectIdentifier, Volume),

    /// An instruction to get all of the storage quotas for a project
    GetProjectQuotas(ProjectIdentifier),

    /// An instruction to clear a storage quota for a user on a volume
    ClearUserQuota(UserIdentifier, Volume),

    /// An instruction to set a storage quota for a user on a volume
    SetUserQuota(UserIdentifier, Volume, QuotaLimit),

    /// An instruction to get the storage quota for a user on a volume
    GetUserQuota(UserIdentifier, Volume),

    /// An instruction to get all of the storage quotas for a user
    GetUserQuotas(UserIdentifier),

    /// An instruction to sync the list of offerings provided
    /// by an agent
    SyncOfferings(Destinations),

    /// An instruction to add new offering(s) to an agent
    AddOfferings(Destinations),

    /// An instruction to remove offering(s) from an agent
    RemoveOfferings(Destinations),

    /// An instruction to get the list of offerings from an agent
    GetOfferings(),
}

impl Instruction {
    pub fn parse(s: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = s.split(' ').collect();

        // Positional-argument accessors that cannot panic.
        //
        // Indexing `parts` directly panics whenever an instruction arrives
        // with fewer arguments than its arm expects. That is reachable from
        // the wire - `Command`'s `Deserialize` runs this parser on the
        // `command` string of every incoming `Job` - and fatal rather than
        // recoverable, because the release profile sets `panic = "abort"`.
        // Returning an empty string for a missing argument instead lets each
        // arm's existing error handling reject the instruction cleanly, since
        // every sub-parser already rejects the empty string. See
        // docs/specifications/security-review-2.md (finding R1).
        let arg = |n: usize| -> &str { parts.get(n).copied().unwrap_or_default() };
        let rest = |n: usize| -> String { parts.get(n..).unwrap_or_default().join(" ") };

        match arg(0) {
            "submit" => match Destination::parse(arg(1)) {
                Ok(destination) => match Instruction::parse(&rest(2)) {
                    Ok(instruction) => Ok(Instruction::Submit(
                        destination,
                        Arc::<Instruction>::new(instruction),
                    )),
                    Err(e) => {
                        tracing::error!(
                            "submit failed to parse the instruction for destination {}: {}. {}",
                            arg(1),
                            &rest(2),
                            e
                        );
                        Err(Error::Parse(format!(
                            "submit failed to parse the instruction for destination {}: {}. {}",
                            arg(1),
                            rest(2),
                            e
                        )))
                    }
                },
                Err(e) => {
                    tracing::error!(
                        "submit failed to parse the destination for: {}. {}",
                        &rest(1),
                        e
                    );
                    Err(Error::Parse(format!(
                        "submit failed to parse the destination for: {}. {}",
                        rest(1),
                        e
                    )))
                }
            },
            "create_project" | "create_award" => match ProjectIdentifier::parse(arg(1)) {
                Ok(project) => match ProjectDetails::parse(&rest(2)) {
                    Ok(details) => Ok(Instruction::CreateProject(project, details)),
                    Err(_) => {
                        tracing::error!("create_project failed to parse: {}", &rest(3));
                        Err(Error::Parse(format!(
                            "create_project failed to parse: {}",
                            rest(3)
                        )))
                    }
                },
                Err(_) => {
                    tracing::error!("create_project failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "create_project failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "update_project" | "update_award" => match ProjectIdentifier::parse(arg(1)) {
                Ok(project) => match ProjectDetails::parse(&rest(2)) {
                    Ok(details) => Ok(Instruction::UpdateProject(project, details)),
                    Err(_) => {
                        tracing::error!("update_project failed to parse: {}", &rest(2));
                        Err(Error::Parse(format!(
                            "update_project failed to parse: {}",
                            rest(2)
                        )))
                    }
                },
                Err(_) => {
                    tracing::error!("update_project failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "update_project failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "get_project" => match ProjectIdentifier::parse(&rest(1)) {
                Ok(project) => Ok(Instruction::GetProject(project)),
                Err(_) => {
                    tracing::error!("get_project failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "get_project failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "get_projects" => match PortalIdentifier::parse(&rest(1)) {
                Ok(portal) => Ok(Instruction::GetProjects(portal)),
                Err(_) => {
                    tracing::error!("get_projects failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "get_projects failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "get_award" => match ProjectIdentifier::parse(&rest(1)) {
                Ok(project) => Ok(Instruction::GetAward(project)),
                Err(_) => {
                    tracing::error!("get_award failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "get_award failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "get_awards" | "list_awards" => match PortalIdentifier::parse(&rest(1)) {
                Ok(portal) => Ok(Instruction::GetAwards(portal)),
                Err(_) => {
                    tracing::error!("get_awards failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "get_awards failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "add_project" => match ProjectIdentifier::parse(&rest(1)) {
                Ok(project) => Ok(Instruction::AddProject(project)),
                Err(_) => {
                    tracing::error!("add_project failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "add_project failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "remove_project" | "remove_award" => match ProjectIdentifier::parse(&rest(1)) {
                Ok(project) => Ok(Instruction::RemoveProject(project)),
                Err(_) => {
                    tracing::error!("remove_project failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "remove_project failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "add_local_project" => match ProjectMapping::parse(&rest(1)) {
                Ok(mapping) => Ok(Instruction::AddLocalProject(mapping)),
                Err(_) => {
                    tracing::error!("add_local_project failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "add_local_project failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "remove_local_project" => match ProjectMapping::parse(&rest(1)) {
                Ok(mapping) => Ok(Instruction::RemoveLocalProject(mapping)),
                Err(_) => {
                    tracing::error!("remove_local_project failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "remove_local_project failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "get_users" => match ProjectIdentifier::parse(&rest(1)) {
                Ok(project) => Ok(Instruction::GetUsers(project)),
                Err(_) => {
                    tracing::error!("get_users failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "get_users failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "add_user" => match UserIdentifier::parse(&rest(1)) {
                Ok(user) => Ok(Instruction::AddUser(user)),
                Err(_) => {
                    tracing::error!("add_user failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "add_user failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "remove_user" => match UserIdentifier::parse(&rest(1)) {
                Ok(user) => Ok(Instruction::RemoveUser(user)),
                Err(_) => {
                    tracing::error!("remove_user failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "remove_user failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "block_user" => match UserIdentifier::parse(&rest(1)) {
                Ok(user) => Ok(Instruction::BlockUser(user)),
                Err(_) => {
                    tracing::error!("block_user failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "block_user failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "unblock_user" => match UserIdentifier::parse(&rest(1)) {
                Ok(user) => Ok(Instruction::UnblockUser(user)),
                Err(_) => {
                    tracing::error!("unblock_user failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "unblock_user failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "is_blocked_user" => match UserIdentifier::parse(&rest(1)) {
                Ok(user) => Ok(Instruction::IsBlockedUser(user)),
                Err(_) => {
                    tracing::error!("is_blocked_user failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "is_blocked_user failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "block_project" => match ProjectIdentifier::parse(&rest(1)) {
                Ok(project) => Ok(Instruction::BlockProject(project)),
                Err(_) => {
                    tracing::error!("block_project failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "block_project failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "unblock_project" => match ProjectIdentifier::parse(&rest(1)) {
                Ok(project) => Ok(Instruction::UnblockProject(project)),
                Err(_) => {
                    tracing::error!("unblock_project failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "unblock_project failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "is_blocked_project" => match ProjectIdentifier::parse(&rest(1)) {
                Ok(project) => Ok(Instruction::IsBlockedProject(project)),
                Err(_) => {
                    tracing::error!("is_blocked_project failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "is_blocked_project failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "get_project_mapping" => match ProjectIdentifier::parse(&rest(1)) {
                Ok(project) => Ok(Instruction::GetProjectMapping(project)),
                Err(_) => {
                    tracing::error!("get_project_mapping failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "get_project_mapping failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "get_user_mapping" => match UserIdentifier::parse(&rest(1)) {
                Ok(user) => Ok(Instruction::GetUserMapping(user)),
                Err(_) => {
                    tracing::error!("get_user_mapping failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "get_user_mapping failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "add_local_user" => match UserMapping::parse(&rest(1)) {
                Ok(mapping) => Ok(Instruction::AddLocalUser(mapping)),
                Err(_) => {
                    tracing::error!("add_local_user failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "add_local_user failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "remove_local_user" => match UserMapping::parse(&rest(1)) {
                Ok(mapping) => Ok(Instruction::RemoveLocalUser(mapping)),
                Err(_) => {
                    tracing::error!("remove_local_user failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "remove_local_user failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "update_homedir" => {
                if parts.len() < 3 {
                    tracing::error!("update_homedir failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "update_homedir failed to parse: {}",
                        rest(1)
                    )));
                }

                let homedir = arg(2).trim().to_string();

                if homedir.is_empty() {
                    tracing::error!("update_homedir failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "update_homedir failed to parse: {}",
                        rest(1)
                    )));
                }

                match UserIdentifier::parse(arg(1)) {
                    Ok(user) => Ok(Instruction::UpdateHomeDir(user, homedir)),
                    Err(_) => {
                        tracing::error!("update_homedir failed to parse: {}", &rest(1));
                        Err(Error::Parse(format!(
                            "update_homedir failed to parse: {}",
                            rest(1)
                        )))
                    }
                }
            }
            "get_local_usage_report" => {
                if parts.len() < 2 {
                    tracing::error!("get_local_usage_report failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_local_usage_report failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectMapping::parse(arg(1)) {
                    Ok(mapping) => {
                        match DateRange::parse(parts.get(2).cloned().unwrap_or("this_week")) {
                            Ok(date_range) => {
                                Ok(Instruction::GetLocalUsageReport(mapping, date_range))
                            }
                            Err(e) => {
                                tracing::error!(
                                    "get_local_usage_report failed to parse '{}': {}",
                                    &rest(1),
                                    e
                                );
                                Err(Error::Parse(format!(
                                    "get_local_usage_report failed to parse '{}': {}",
                                    rest(1),
                                    e
                                )))
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "get_local_usage_report failed to parse '{}': {}",
                            &rest(1),
                            e
                        );
                        Err(Error::Parse(format!(
                            "get_local_usage_report failed to parse '{}': {}",
                            rest(1),
                            e
                        )))
                    }
                }
            }
            "get_storage_report" => {
                if parts.len() < 2 {
                    tracing::error!("get_storage_report failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_storage_report failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectIdentifier::parse(arg(1)) {
                    Ok(project) => {
                        match DateRange::parse(parts.get(2).cloned().unwrap_or("today")) {
                            Ok(date_range) => {
                                Ok(Instruction::GetStorageReport(project, date_range))
                            }
                            Err(e) => {
                                tracing::error!(
                                    "get_storage_report failed to parse '{}': {}",
                                    &rest(1),
                                    e
                                );
                                Err(Error::Parse(format!(
                                    "get_storage_report failed to parse '{}': {}",
                                    rest(1),
                                    e
                                )))
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("get_storage_report failed to parse '{}': {}", &rest(1), e);
                        Err(Error::Parse(format!(
                            "get_storage_report failed to parse '{}': {}",
                            rest(1),
                            e
                        )))
                    }
                }
            }
            "get_storage_reports" => {
                if parts.len() < 2 {
                    tracing::error!("get_storage_reports failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_storage_reports failed to parse: {}",
                        rest(1)
                    )));
                }

                match PortalIdentifier::parse(arg(1)) {
                    Ok(portal) => {
                        match DateRange::parse(parts.get(2).cloned().unwrap_or("today")) {
                            Ok(date_range) => {
                                Ok(Instruction::GetStorageReports(portal, date_range))
                            }
                            Err(e) => {
                                tracing::error!(
                                    "get_storage_reports failed to parse '{}': {}",
                                    &rest(1),
                                    e
                                );
                                Err(Error::Parse(format!(
                                    "get_storage_reports failed to parse '{}': {}",
                                    rest(1),
                                    e
                                )))
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "get_storage_reports failed to parse '{}': {}",
                            &rest(1),
                            e
                        );
                        Err(Error::Parse(format!(
                            "get_storage_reports failed to parse '{}': {}",
                            rest(1),
                            e
                        )))
                    }
                }
            }
            "get_usage_report" => {
                if parts.len() < 2 {
                    tracing::error!("get_usage_report failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_usage_report failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectIdentifier::parse(arg(1)) {
                    Ok(project) => {
                        match DateRange::parse(parts.get(2).cloned().unwrap_or("this_week")) {
                            Ok(date_range) => Ok(Instruction::GetUsageReport(project, date_range)),
                            Err(e) => {
                                tracing::error!(
                                    "get_usage_report failed to parse '{}': {}",
                                    &rest(1),
                                    e
                                );
                                Err(Error::Parse(format!(
                                    "get_usage_report failed to parse '{}': {}",
                                    rest(1),
                                    e
                                )))
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("get_usage_report failed to parse '{}': {}", &rest(1), e);
                        Err(Error::Parse(format!(
                            "get_usage_report failed to parse '{}': {}",
                            rest(1),
                            e
                        )))
                    }
                }
            }
            "get_usage_reports" => {
                if parts.len() < 2 {
                    tracing::error!("get_usage_reports failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_usage_reports failed to parse: {}",
                        rest(1)
                    )));
                }

                match PortalIdentifier::parse(arg(1)) {
                    Ok(portal) => {
                        match DateRange::parse(parts.get(2).cloned().unwrap_or("this_week")) {
                            Ok(date_range) => Ok(Instruction::GetUsageReports(portal, date_range)),
                            Err(e) => {
                                tracing::error!(
                                    "get_usage_reports failed to parse '{}': {}",
                                    &rest(1),
                                    e
                                );
                                Err(Error::Parse(format!(
                                    "get_usage_reports failed to parse '{}': {}",
                                    rest(1),
                                    e
                                )))
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("get_usage_reports failed to parse '{}': {}", &rest(1), e);
                        Err(Error::Parse(format!(
                            "get_usage_reports failed to parse '{}': {}",
                            rest(1),
                            e
                        )))
                    }
                }
            }
            "set_local_limit" => {
                if parts.len() < 3 {
                    tracing::error!("set_local_limit failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "set_local_limit failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectMapping::parse(arg(1)) {
                    Ok(mapping) => match Usage::parse(arg(2)) {
                        Ok(usage) => Ok(Instruction::SetLocalLimit(mapping, usage)),
                        Err(e) => {
                            tracing::error!(
                                "set_local_limit failed to parse '{}': {}",
                                &rest(1),
                                e
                            );
                            Err(Error::Parse(format!(
                                "set_local_limit failed to parse '{}': {}",
                                rest(1),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!("set_local_limit failed to parse '{}': {}", &rest(1), e);
                        Err(Error::Parse(format!(
                            "set_local_limit failed to parse '{}': {}",
                            rest(1),
                            e
                        )))
                    }
                }
            }
            "get_local_limit" => {
                if parts.len() < 2 {
                    tracing::error!("get_local_limit failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_local_limit failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectMapping::parse(arg(1)) {
                    Ok(mapping) => Ok(Instruction::GetLocalLimit(mapping)),
                    Err(e) => {
                        tracing::error!("get_local_limit failed to parse '{}': {}", &rest(1), e);
                        Err(Error::Parse(format!(
                            "get_local_limit failed to parse '{}': {}",
                            rest(1),
                            e
                        )))
                    }
                }
            }
            "set_limit" => {
                if parts.len() < 3 {
                    tracing::error!("set_limit failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "set_limit failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectIdentifier::parse(arg(1)) {
                    Ok(project) => match Usage::parse(&rest(2)) {
                        Ok(usage) => Ok(Instruction::SetLimit(project, usage)),
                        Err(e) => {
                            tracing::error!("set_limit failed to parse '{}': {}", &rest(1), e);
                            Err(Error::Parse(format!(
                                "set_limit failed to parse '{}': {}",
                                rest(1),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!("set_limit failed to parse '{}': {}", &rest(1), e);
                        Err(Error::Parse(format!(
                            "set_limit failed to parse '{}': {}",
                            rest(1),
                            e
                        )))
                    }
                }
            }
            "get_limit" => {
                if parts.len() < 2 {
                    tracing::error!("get_limit failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_limit failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectIdentifier::parse(arg(1)) {
                    Ok(project) => Ok(Instruction::GetLimit(project)),
                    Err(e) => {
                        tracing::error!("get_limit failed to parse '{}': {}", &rest(1), e);
                        Err(Error::Parse(format!(
                            "get_limit failed to parse '{}': {}",
                            rest(1),
                            e
                        )))
                    }
                }
            }
            "clear_project_quota" => {
                if parts.len() < 3 {
                    tracing::error!("clear_project_quota failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "clear_project_quota failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectIdentifier::parse(arg(1)) {
                    Ok(project) => match Volume::parse(arg(2)) {
                        Ok(volume) => Ok(Instruction::ClearProjectQuota(project, volume)),
                        Err(e) => {
                            tracing::error!(
                                "clear_project_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            );
                            Err(Error::Parse(format!(
                                "clear_project_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            "clear_project_quota failed to parse project '{}': {}",
                            arg(1),
                            e
                        );
                        Err(Error::Parse(format!(
                            "clear_project_quota failed to parse project '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "set_project_quota" => {
                if parts.len() < 4 {
                    tracing::error!("set_project_quota failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "set_project_quota failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectIdentifier::parse(arg(1)) {
                    Ok(project) => match Volume::parse(arg(2)) {
                        Ok(volume) => match QuotaLimit::parse(&rest(3)) {
                            Ok(limit) => Ok(Instruction::SetProjectQuota(project, volume, limit)),
                            Err(e) => {
                                tracing::error!(
                                    "set_project_quota failed to parse quota '{}': {}",
                                    &rest(3),
                                    e
                                );
                                Err(Error::Parse(format!(
                                    "set_project_quota failed to parse quota '{}': {}",
                                    rest(3),
                                    e
                                )))
                            }
                        },
                        Err(e) => {
                            tracing::error!(
                                "set_project_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            );
                            Err(Error::Parse(format!(
                                "set_project_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            "set_project_quota failed to parse project '{}': {}",
                            arg(1),
                            e
                        );
                        Err(Error::Parse(format!(
                            "set_project_quota failed to parse project '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "get_project_quota" => {
                if parts.len() < 3 {
                    tracing::error!("get_project_quota failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_project_quota failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectIdentifier::parse(arg(1)) {
                    Ok(project) => match Volume::parse(arg(2)) {
                        Ok(volume) => Ok(Instruction::GetProjectQuota(project, volume)),
                        Err(e) => {
                            tracing::error!(
                                "get_project_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            );
                            Err(Error::Parse(format!(
                                "get_project_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            "get_project_quota failed to parse project '{}': {}",
                            arg(1),
                            e
                        );
                        Err(Error::Parse(format!(
                            "get_project_quota failed to parse project '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "get_project_quotas" => {
                if parts.len() < 2 {
                    tracing::error!("get_project_quotas failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_project_quotas failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectIdentifier::parse(arg(1)) {
                    Ok(project) => Ok(Instruction::GetProjectQuotas(project)),
                    Err(e) => {
                        tracing::error!("get_project_quotas failed to parse '{}': {}", arg(1), e);
                        Err(Error::Parse(format!(
                            "get_project_quotas failed to parse '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "clear_user_quota" => {
                if parts.len() < 3 {
                    tracing::error!("clear_user_quota failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "clear_user_quota failed to parse: {}",
                        rest(1)
                    )));
                }

                match UserIdentifier::parse(arg(1)) {
                    Ok(user) => match Volume::parse(arg(2)) {
                        Ok(volume) => Ok(Instruction::ClearUserQuota(user, volume)),
                        Err(e) => {
                            tracing::error!(
                                "clear_user_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            );
                            Err(Error::Parse(format!(
                                "clear_user_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            "clear_user_quota failed to parse user '{}': {}",
                            arg(1),
                            e
                        );
                        Err(Error::Parse(format!(
                            "clear_user_quota failed to parse user '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "set_user_quota" => {
                if parts.len() < 4 {
                    tracing::error!("set_user_quota failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "set_user_quota failed to parse: {}",
                        rest(1)
                    )));
                }

                match UserIdentifier::parse(arg(1)) {
                    Ok(user) => match Volume::parse(arg(2)) {
                        Ok(volume) => match QuotaLimit::parse(&rest(3)) {
                            Ok(limit) => Ok(Instruction::SetUserQuota(user, volume, limit)),
                            Err(e) => {
                                tracing::error!(
                                    "set_user_quota failed to parse quota '{}': {}",
                                    &rest(3),
                                    e
                                );
                                Err(Error::Parse(format!(
                                    "set_user_quota failed to parse quota '{}': {}",
                                    rest(3),
                                    e
                                )))
                            }
                        },
                        Err(e) => {
                            tracing::error!(
                                "set_user_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            );
                            Err(Error::Parse(format!(
                                "set_user_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!("set_user_quota failed to parse user '{}': {}", arg(1), e);
                        Err(Error::Parse(format!(
                            "set_user_quota failed to parse user '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "get_user_quota" => {
                if parts.len() < 3 {
                    tracing::error!("get_user_quota failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_user_quota failed to parse: {}",
                        rest(1)
                    )));
                }

                match UserIdentifier::parse(arg(1)) {
                    Ok(user) => match Volume::parse(arg(2)) {
                        Ok(volume) => Ok(Instruction::GetUserQuota(user, volume)),
                        Err(e) => {
                            tracing::error!(
                                "get_user_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            );
                            Err(Error::Parse(format!(
                                "get_user_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!("get_user_quota failed to parse user '{}': {}", arg(1), e);
                        Err(Error::Parse(format!(
                            "get_user_quota failed to parse user '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "get_user_quotas" => {
                if parts.len() < 2 {
                    tracing::error!("get_user_quotas failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_user_quotas failed to parse: {}",
                        rest(1)
                    )));
                }

                match UserIdentifier::parse(arg(1)) {
                    Ok(user) => Ok(Instruction::GetUserQuotas(user)),
                    Err(e) => {
                        tracing::error!("get_user_quotas failed to parse '{}': {}", arg(1), e);
                        Err(Error::Parse(format!(
                            "get_user_quotas failed to parse '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "clear_local_project_quota" => {
                if parts.len() < 3 {
                    tracing::error!("clear_local_project_quota failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "clear_local_project_quota failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectMapping::parse(arg(1)) {
                    Ok(mapping) => match Volume::parse(arg(2)) {
                        Ok(volume) => Ok(Instruction::ClearLocalProjectQuota(mapping, volume)),
                        Err(e) => {
                            tracing::error!(
                                "clear_local_project_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            );
                            Err(Error::Parse(format!(
                                "clear_local_project_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            "clear_local_project_quota failed to parse mapping '{}': {}",
                            arg(1),
                            e
                        );
                        Err(Error::Parse(format!(
                            "clear_local_project_quota failed to parse mapping '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "set_local_project_quota" => {
                if parts.len() < 4 {
                    tracing::error!("set_local_project_quota failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "set_local_project_quota failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectMapping::parse(arg(1)) {
                    Ok(mapping) => match Volume::parse(arg(2)) {
                        Ok(volume) => match QuotaLimit::parse(&rest(3)) {
                            Ok(limit) => {
                                Ok(Instruction::SetLocalProjectQuota(mapping, volume, limit))
                            }
                            Err(e) => {
                                tracing::error!(
                                    "set_local_project_quota failed to parse quota '{}': {}",
                                    &rest(3),
                                    e
                                );
                                Err(Error::Parse(format!(
                                    "set_local_project_quota failed to parse quota '{}': {}",
                                    rest(3),
                                    e
                                )))
                            }
                        },
                        Err(e) => {
                            tracing::error!(
                                "set_local_project_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            );
                            Err(Error::Parse(format!(
                                "set_local_project_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            "set_local_project_quota failed to parse mapping '{}': {}",
                            arg(1),
                            e
                        );
                        Err(Error::Parse(format!(
                            "set_local_project_quota failed to parse mapping '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "get_local_project_quota" => {
                if parts.len() < 3 {
                    tracing::error!("get_local_project_quota failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_local_project_quota failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectMapping::parse(arg(1)) {
                    Ok(mapping) => match Volume::parse(arg(2)) {
                        Ok(volume) => Ok(Instruction::GetLocalProjectQuota(mapping, volume)),
                        Err(e) => {
                            tracing::error!(
                                "get_local_project_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            );
                            Err(Error::Parse(format!(
                                "get_local_project_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            "get_local_project_quota failed to parse mapping '{}': {}",
                            arg(1),
                            e
                        );
                        Err(Error::Parse(format!(
                            "get_local_project_quota failed to parse mapping '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "get_local_project_quotas" => {
                if parts.len() < 2 {
                    tracing::error!("get_local_project_quotas failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_local_project_quotas failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectMapping::parse(arg(1)) {
                    Ok(mapping) => Ok(Instruction::GetLocalProjectQuotas(mapping)),
                    Err(e) => {
                        tracing::error!(
                            "get_local_project_quotas failed to parse '{}': {}",
                            arg(1),
                            e
                        );
                        Err(Error::Parse(format!(
                            "get_local_project_quotas failed to parse '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "clear_local_user_quota" => {
                if parts.len() < 3 {
                    tracing::error!("clear_local_user_quota failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "clear_local_user_quota failed to parse: {}",
                        rest(1)
                    )));
                }

                match UserMapping::parse(arg(1)) {
                    Ok(mapping) => match Volume::parse(arg(2)) {
                        Ok(volume) => Ok(Instruction::ClearLocalUserQuota(mapping, volume)),
                        Err(e) => {
                            tracing::error!(
                                "clear_local_user_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            );
                            Err(Error::Parse(format!(
                                "clear_local_user_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            "clear_local_user_quota failed to parse mapping '{}': {}",
                            arg(1),
                            e
                        );
                        Err(Error::Parse(format!(
                            "clear_local_user_quota failed to parse mapping '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "set_local_user_quota" => {
                if parts.len() < 4 {
                    tracing::error!("set_local_user_quota failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "set_local_user_quota failed to parse: {}",
                        rest(1)
                    )));
                }

                match UserMapping::parse(arg(1)) {
                    Ok(mapping) => match Volume::parse(arg(2)) {
                        Ok(volume) => match QuotaLimit::parse(&rest(3)) {
                            Ok(limit) => Ok(Instruction::SetLocalUserQuota(mapping, volume, limit)),
                            Err(e) => {
                                tracing::error!(
                                    "set_local_user_quota failed to parse quota '{}': {}",
                                    &rest(3),
                                    e
                                );
                                Err(Error::Parse(format!(
                                    "set_local_user_quota failed to parse quota '{}': {}",
                                    rest(3),
                                    e
                                )))
                            }
                        },
                        Err(e) => {
                            tracing::error!(
                                "set_local_user_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            );
                            Err(Error::Parse(format!(
                                "set_local_user_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            "set_local_user_quota failed to parse mapping '{}': {}",
                            arg(1),
                            e
                        );
                        Err(Error::Parse(format!(
                            "set_local_user_quota failed to parse mapping '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "get_local_user_quota" => {
                if parts.len() < 3 {
                    tracing::error!("get_local_user_quota failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_local_user_quota failed to parse: {}",
                        rest(1)
                    )));
                }

                match UserMapping::parse(arg(1)) {
                    Ok(mapping) => match Volume::parse(arg(2)) {
                        Ok(volume) => Ok(Instruction::GetLocalUserQuota(mapping, volume)),
                        Err(e) => {
                            tracing::error!(
                                "get_local_user_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            );
                            Err(Error::Parse(format!(
                                "get_local_user_quota failed to parse volume '{}': {}",
                                arg(2),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            "get_local_user_quota failed to parse mapping '{}': {}",
                            arg(1),
                            e
                        );
                        Err(Error::Parse(format!(
                            "get_local_user_quota failed to parse mapping '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "get_local_user_quotas" => {
                if parts.len() < 2 {
                    tracing::error!("get_local_user_quotas failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_local_user_quotas failed to parse: {}",
                        rest(1)
                    )));
                }

                match UserMapping::parse(arg(1)) {
                    Ok(mapping) => Ok(Instruction::GetLocalUserQuotas(mapping)),
                    Err(e) => {
                        tracing::error!(
                            "get_local_user_quotas failed to parse '{}': {}",
                            arg(1),
                            e
                        );
                        Err(Error::Parse(format!(
                            "get_local_user_quotas failed to parse '{}': {}",
                            arg(1),
                            e
                        )))
                    }
                }
            }
            "is_protected_user" => match UserIdentifier::parse(&rest(1)) {
                Ok(user) => Ok(Instruction::IsProtectedUser(user)),
                Err(_) => {
                    tracing::error!("is_protected_user failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "is_protected_user failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "is_existing_user" => match UserIdentifier::parse(&rest(1)) {
                Ok(user) => Ok(Instruction::IsExistingUser(user)),
                Err(_) => {
                    tracing::error!("is_existing_user failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "is_existing_user failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "is_existing_project" => match ProjectIdentifier::parse(&rest(1)) {
                Ok(project) => Ok(Instruction::IsExistingProject(project)),
                Err(_) => {
                    tracing::error!("is_existing_project failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "is_existing_project failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "get_home_dir" => match UserIdentifier::parse(&rest(1)) {
                Ok(user) => Ok(Instruction::GetHomeDir(user)),
                Err(_) => {
                    tracing::error!("get_home_dir failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "get_home_dir failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "get_project_dirs" => match ProjectIdentifier::parse(&rest(1)) {
                Ok(project) => Ok(Instruction::GetProjectDirs(project)),
                Err(_) => {
                    tracing::error!("get_project_dirs failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "get_project_dirs failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "get_user_dirs" => match UserIdentifier::parse(&rest(1)) {
                Ok(user) => Ok(Instruction::GetUserDirs(user)),
                Err(_) => {
                    tracing::error!("get_user_dirs failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "get_user_dirs failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "get_local_home_dir" => match UserMapping::parse(&rest(1)) {
                Ok(mapping) => Ok(Instruction::GetLocalHomeDir(mapping)),
                Err(_) => {
                    tracing::error!("get_local_home_dir failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "get_local_home_dir failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "get_local_storage_report" => {
                if parts.len() < 2 {
                    tracing::error!("get_local_storage_report failed to parse: {}", &rest(1));
                    return Err(Error::Parse(format!(
                        "get_local_storage_report failed to parse: {}",
                        rest(1)
                    )));
                }

                match ProjectMapping::parse(arg(1)) {
                    Ok(mapping) => {
                        match DateRange::parse(parts.get(2).cloned().unwrap_or("today")) {
                            Ok(date_range) => {
                                Ok(Instruction::GetLocalStorageReport(mapping, date_range))
                            }
                            Err(e) => {
                                tracing::error!(
                                    "get_local_storage_report failed to parse '{}': {}",
                                    &rest(1),
                                    e
                                );
                                Err(Error::Parse(format!(
                                    "get_local_storage_report failed to parse '{}': {}",
                                    rest(1),
                                    e
                                )))
                            }
                        }
                    }
                    Err(_) => {
                        tracing::error!("get_local_storage_report failed to parse: {}", &rest(1));
                        Err(Error::Parse(format!(
                            "get_local_storage_report failed to parse: {}",
                            rest(1)
                        )))
                    }
                }
            }
            "get_local_project_dirs" => match ProjectMapping::parse(&rest(1)) {
                Ok(mapping) => Ok(Instruction::GetLocalProjectDirs(mapping)),
                Err(_) => {
                    tracing::error!("get_local_project_dirs failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "get_local_project_dirs failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "get_local_user_dirs" => match UserMapping::parse(&rest(1)) {
                Ok(mapping) => Ok(Instruction::GetLocalUserDirs(mapping)),
                Err(_) => {
                    tracing::error!("get_local_user_dirs failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "get_local_user_dirs failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "add_offerings" => match Destinations::parse(&rest(1)) {
                Ok(offerings) => Ok(Instruction::AddOfferings(offerings)),
                Err(_) => {
                    tracing::error!("add_offerings failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "add_offerings failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "remove_offerings" => match Destinations::parse(&rest(1)) {
                Ok(offerings) => Ok(Instruction::RemoveOfferings(offerings)),
                Err(_) => {
                    tracing::error!("remove_offerings failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "remove_offerings failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "sync_offerings" => match Destinations::parse(&rest(1)) {
                Ok(offerings) => Ok(Instruction::SyncOfferings(offerings)),
                Err(_) => {
                    tracing::error!("sync_offerings failed to parse: {}", &rest(1));
                    Err(Error::Parse(format!(
                        "sync_offerings failed to parse: {}",
                        rest(1)
                    )))
                }
            },
            "get_offerings" => Ok(Instruction::GetOfferings()),
            _ => {
                tracing::error!("Invalid instruction: {}", s);
                Err(Error::Parse(format!("Invalid instruction: {}", s)))
            }
        }
    }

    pub fn command(&self) -> String {
        match self {
            Instruction::Submit(_, _) => "submit".to_string(),
            Instruction::CreateProject(_, _) => "create_project".to_string(),
            Instruction::UpdateProject(_, _) => "update_project".to_string(),
            Instruction::GetProject(_) => "get_project".to_string(),
            Instruction::GetProjects(_) => "get_projects".to_string(),
            Instruction::GetAward(_) => "get_award".to_string(),
            Instruction::GetAwards(_) => "get_awards".to_string(),
            Instruction::AddProject(_) => "add_project".to_string(),
            Instruction::RemoveProject(_) => "remove_project".to_string(),
            Instruction::GetUsers(_) => "get_users".to_string(),
            Instruction::AddUser(_) => "add_user".to_string(),
            Instruction::RemoveUser(_) => "remove_user".to_string(),
            Instruction::BlockUser(_) => "block_user".to_string(),
            Instruction::UnblockUser(_) => "unblock_user".to_string(),
            Instruction::IsBlockedUser(_) => "is_blocked_user".to_string(),
            Instruction::BlockProject(_) => "block_project".to_string(),
            Instruction::UnblockProject(_) => "unblock_project".to_string(),
            Instruction::IsBlockedProject(_) => "is_blocked_project".to_string(),
            Instruction::GetUserMapping(_) => "get_user_mapping".to_string(),
            Instruction::GetProjectMapping(_) => "get_project_mapping".to_string(),
            Instruction::GetHomeDir(_) => "get_home_dir".to_string(),
            Instruction::GetUserDirs(_) => "get_user_dirs".to_string(),
            Instruction::GetProjectDirs(_) => "get_project_dirs".to_string(),
            Instruction::AddLocalUser(_) => "add_local_user".to_string(),
            Instruction::RemoveLocalUser(_) => "remove_local_user".to_string(),
            Instruction::AddLocalProject(_) => "add_local_project".to_string(),
            Instruction::RemoveLocalProject(_) => "remove_local_project".to_string(),
            Instruction::GetLocalUsageReport(_, _) => "get_local_usage_report".to_string(),
            Instruction::GetLocalLimit(_) => "get_local_limit".to_string(),
            Instruction::SetLocalLimit(_, _) => "set_local_limit".to_string(),
            Instruction::GetLocalProjectQuota(_, _) => "get_local_project_quota".to_string(),
            Instruction::ClearLocalProjectQuota(_, _) => "clear_local_project_quota".to_string(),
            Instruction::SetLocalProjectQuota(_, _, _) => "set_local_project_quota".to_string(),
            Instruction::GetLocalProjectQuotas(_) => "get_local_project_quotas".to_string(),
            Instruction::GetLocalUserQuota(_, _) => "get_local_user_quota".to_string(),
            Instruction::ClearLocalUserQuota(_, _) => "clear_local_user_quota".to_string(),
            Instruction::SetLocalUserQuota(_, _, _) => "set_local_user_quota".to_string(),
            Instruction::GetLocalUserQuotas(_) => "get_local_user_quotas".to_string(),
            Instruction::GetLocalHomeDir(_) => "get_local_home_dir".to_string(),
            Instruction::GetLocalUserDirs(_) => "get_local_user_dirs".to_string(),
            Instruction::GetLocalProjectDirs(_) => "get_local_project_dirs".to_string(),
            Instruction::UpdateHomeDir(_, _) => "update_homedir".to_string(),
            Instruction::GetLocalStorageReport(_, _) => "get_local_storage_report".to_string(),
            Instruction::GetStorageReport(_, _) => "get_storage_report".to_string(),
            Instruction::GetStorageReports(_, _) => "get_storage_reports".to_string(),
            Instruction::GetUsageReport(_, _) => "get_usage_report".to_string(),
            Instruction::GetUsageReports(_, _) => "get_usage_reports".to_string(),
            Instruction::SetLimit(_, _) => "set_limit".to_string(),
            Instruction::GetLimit(_) => "get_limit".to_string(),
            Instruction::GetProjectQuota(_, _) => "get_project_quota".to_string(),
            Instruction::SetProjectQuota(_, _, _) => "set_project_quota".to_string(),
            Instruction::ClearProjectQuota(_, _) => "clear_project_quota".to_string(),
            Instruction::GetProjectQuotas(_) => "get_project_quotas".to_string(),
            Instruction::GetUserQuota(_, _) => "get_user_quota".to_string(),
            Instruction::ClearUserQuota(_, _) => "clear_user_quota".to_string(),
            Instruction::SetUserQuota(_, _, _) => "set_user_quota".to_string(),
            Instruction::GetUserQuotas(_) => "get_user_quotas".to_string(),
            Instruction::IsProtectedUser(_) => "is_protected_user".to_string(),
            Instruction::IsExistingUser(_) => "is_existing_user".to_string(),
            Instruction::IsExistingProject(_) => "is_existing_project".to_string(),
            Instruction::SyncOfferings(_) => "sync_offerings".to_string(),
            Instruction::AddOfferings(_) => "add_offerings".to_string(),
            Instruction::RemoveOfferings(_) => "remove_offerings".to_string(),
            Instruction::GetOfferings() => "get_offerings".to_string(),
        }
    }

    pub fn arguments(&self) -> Vec<String> {
        match self {
            Instruction::Submit(destination, command) => {
                vec![destination.to_string(), command.to_string()]
            }
            Instruction::CreateProject(project, details) => {
                vec![project.to_string(), details.to_string()]
            }
            Instruction::UpdateProject(project, details) => {
                vec![project.to_string(), details.to_string()]
            }
            Instruction::GetProject(project) => vec![project.to_string()],
            Instruction::GetProjects(portal) => vec![portal.to_string()],
            Instruction::GetAward(project) => vec![project.to_string()],
            Instruction::GetAwards(portal) => vec![portal.to_string()],
            Instruction::AddProject(project) => vec![project.to_string()],
            Instruction::RemoveProject(project) => vec![project.to_string()],
            Instruction::GetUsers(project) => vec![project.to_string()],
            Instruction::AddUser(user) => vec![user.to_string()],
            Instruction::RemoveUser(user) => vec![user.to_string()],
            Instruction::BlockUser(user) => vec![user.to_string()],
            Instruction::UnblockUser(user) => vec![user.to_string()],
            Instruction::IsBlockedUser(user) => vec![user.to_string()],
            Instruction::BlockProject(project) => vec![project.to_string()],
            Instruction::UnblockProject(project) => vec![project.to_string()],
            Instruction::IsBlockedProject(project) => vec![project.to_string()],
            Instruction::GetUserMapping(user) => vec![user.to_string()],
            Instruction::GetProjectMapping(project) => vec![project.to_string()],
            Instruction::GetHomeDir(user) => vec![user.to_string()],
            Instruction::GetProjectDirs(project) => vec![project.to_string()],
            Instruction::GetUserDirs(user) => vec![user.to_string()],
            Instruction::AddLocalUser(mapping) => vec![mapping.to_string()],
            Instruction::RemoveLocalUser(mapping) => vec![mapping.to_string()],
            Instruction::AddLocalProject(mapping) => vec![mapping.to_string()],
            Instruction::RemoveLocalProject(mapping) => vec![mapping.to_string()],
            Instruction::GetLocalUsageReport(mapping, date_range) => {
                vec![mapping.to_string(), date_range.to_string()]
            }
            Instruction::GetLocalLimit(mapping) => vec![mapping.to_string()],
            Instruction::SetLocalLimit(mapping, usage) => {
                vec![mapping.to_string(), usage.seconds().to_string()]
            }
            Instruction::GetLocalProjectQuota(mapping, volume) => {
                vec![mapping.to_string(), volume.to_string()]
            }
            Instruction::ClearLocalProjectQuota(mapping, volume) => {
                vec![mapping.to_string(), volume.to_string()]
            }
            Instruction::SetLocalProjectQuota(mapping, volume, quota) => {
                vec![mapping.to_string(), volume.to_string(), quota.to_string()]
            }
            Instruction::GetLocalProjectQuotas(mapping) => vec![mapping.to_string()],
            Instruction::GetLocalUserQuota(mapping, volume) => {
                vec![mapping.to_string(), volume.to_string()]
            }
            Instruction::ClearLocalUserQuota(mapping, volume) => {
                vec![mapping.to_string(), volume.to_string()]
            }
            Instruction::SetLocalUserQuota(mapping, volume, quota) => {
                vec![mapping.to_string(), volume.to_string(), quota.to_string()]
            }
            Instruction::GetLocalUserQuotas(mapping) => vec![mapping.to_string()],
            Instruction::GetLocalHomeDir(mapping) => vec![mapping.to_string()],
            Instruction::GetLocalUserDirs(mapping) => vec![mapping.to_string()],
            Instruction::GetLocalProjectDirs(mapping) => vec![mapping.to_string()],
            Instruction::GetLocalStorageReport(mapping, date_range) => {
                vec![mapping.to_string(), date_range.to_string()]
            }
            Instruction::UpdateHomeDir(user, homedir) => {
                vec![user.to_string(), homedir.clone()]
            }
            Instruction::GetStorageReport(project, date_range) => {
                vec![project.to_string(), date_range.to_string()]
            }
            Instruction::GetStorageReports(portal, date_range) => {
                vec![portal.to_string(), date_range.to_string()]
            }
            Instruction::GetUsageReport(project, date_range) => {
                vec![project.to_string(), date_range.to_string()]
            }
            Instruction::GetUsageReports(portal, date_range) => {
                vec![portal.to_string(), date_range.to_string()]
            }
            Instruction::SetLimit(project, usage) => {
                vec![project.to_string(), usage.seconds().to_string()]
            }
            Instruction::GetLimit(project) => vec![project.to_string()],
            Instruction::GetProjectQuota(project, volume) => {
                vec![project.to_string(), volume.to_string()]
            }
            Instruction::ClearProjectQuota(project, volume) => {
                vec![project.to_string(), volume.to_string()]
            }
            Instruction::SetProjectQuota(project, volume, quota) => {
                vec![project.to_string(), volume.to_string(), quota.to_string()]
            }
            Instruction::GetProjectQuotas(project) => vec![project.to_string()],
            Instruction::GetUserQuota(user, volume) => {
                vec![user.to_string(), volume.to_string()]
            }
            Instruction::ClearUserQuota(user, volume) => {
                vec![user.to_string(), volume.to_string()]
            }
            Instruction::SetUserQuota(user, volume, quota) => {
                vec![user.to_string(), volume.to_string(), quota.to_string()]
            }
            Instruction::GetUserQuotas(user) => vec![user.to_string()],
            Instruction::IsProtectedUser(user) => vec![user.to_string()],
            Instruction::IsExistingUser(user) => vec![user.to_string()],
            Instruction::IsExistingProject(project) => vec![project.to_string()],
            Instruction::SyncOfferings(offerings) => vec![offerings.to_string()],
            Instruction::AddOfferings(offerings) => vec![offerings.to_string()],
            Instruction::RemoveOfferings(offerings) => vec![offerings.to_string()],
            Instruction::GetOfferings() => vec![],
        }
    }
}

impl std::fmt::Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Instruction::Submit(destination, command) => {
                write!(f, "submit {} {}", destination, command)
            }
            Instruction::CreateProject(project, details) => {
                write!(f, "create_project {} {}", project, details)
            }
            Instruction::UpdateProject(project, details) => {
                write!(f, "update_project {} {}", project, details)
            }
            Instruction::GetProject(project) => write!(f, "get_project {}", project),
            Instruction::GetProjects(portal) => write!(f, "get_projects {}", portal),
            Instruction::GetAward(project) => write!(f, "get_award {}", project),
            Instruction::GetAwards(portal) => write!(f, "get_awards {}", portal),
            Instruction::AddProject(project) => write!(f, "add_project {}", project),
            Instruction::RemoveProject(project) => write!(f, "remove_project {}", project),
            Instruction::GetUsers(project) => write!(f, "get_users {}", project),
            Instruction::AddUser(user) => write!(f, "add_user {}", user),
            Instruction::RemoveUser(user) => write!(f, "remove_user {}", user),
            Instruction::BlockUser(user) => write!(f, "block_user {}", user),
            Instruction::UnblockUser(user) => write!(f, "unblock_user {}", user),
            Instruction::IsBlockedUser(user) => write!(f, "is_blocked_user {}", user),
            Instruction::BlockProject(project) => write!(f, "block_project {}", project),
            Instruction::UnblockProject(project) => write!(f, "unblock_project {}", project),
            Instruction::IsBlockedProject(project) => write!(f, "is_blocked_project {}", project),
            Instruction::AddLocalProject(mapping) => write!(f, "add_local_project {}", mapping),
            Instruction::RemoveLocalProject(mapping) => {
                write!(f, "remove_local_project {}", mapping)
            }
            Instruction::AddLocalUser(mapping) => write!(f, "add_local_user {}", mapping),
            Instruction::RemoveLocalUser(mapping) => write!(f, "remove_local_user {}", mapping),
            Instruction::UpdateHomeDir(user, homedir) => {
                write!(f, "update_homedir {} {}", user, homedir)
            }
            Instruction::GetUserMapping(user) => write!(f, "get_user_mapping {}", user),
            Instruction::GetProjectMapping(project) => write!(f, "get_project_mapping {}", project),
            Instruction::GetLocalUsageReport(mapping, date_range) => {
                write!(f, "get_local_usage_report {} {}", mapping, date_range)
            }
            Instruction::GetStorageReport(project, date_range) => {
                write!(f, "get_storage_report {} {}", project, date_range)
            }
            Instruction::GetStorageReports(portal, date_range) => {
                write!(f, "get_storage_reports {} {}", portal, date_range)
            }
            Instruction::GetUsageReport(project, date_range) => {
                write!(f, "get_usage_report {} {}", project, date_range)
            }
            Instruction::GetUsageReports(portal, date_range) => {
                write!(f, "get_usage_reports {} {}", portal, date_range)
            }
            Instruction::GetLocalLimit(mapping) => write!(f, "get_local_limit {}", mapping),
            Instruction::SetLocalLimit(mapping, usage) => {
                write!(f, "set_local_limit {} {}", mapping, usage.seconds())
            }
            Instruction::SetLimit(project, usage) => {
                write!(f, "set_limit {} {}", project, usage.seconds())
            }
            Instruction::GetLocalProjectQuota(mapping, volume) => {
                write!(f, "get_local_project_quota {} {}", mapping, volume)
            }
            Instruction::ClearLocalProjectQuota(mapping, volume) => {
                write!(f, "clear_local_project_quota {} {}", mapping, volume)
            }
            Instruction::SetLocalProjectQuota(mapping, volume, quota) => {
                write!(
                    f,
                    "set_local_project_quota {} {} {}",
                    mapping, volume, quota
                )
            }
            Instruction::GetLocalProjectQuotas(mapping) => {
                write!(f, "get_local_project_quotas {}", mapping)
            }
            Instruction::GetLocalUserQuota(mapping, volume) => {
                write!(f, "get_local_user_quota {} {}", mapping, volume)
            }
            Instruction::ClearLocalUserQuota(mapping, volume) => {
                write!(f, "clear_local_user_quota {} {}", mapping, volume)
            }
            Instruction::SetLocalUserQuota(mapping, volume, quota) => {
                write!(f, "set_local_user_quota {} {} {}", mapping, volume, quota)
            }
            Instruction::GetLocalUserQuotas(mapping) => {
                write!(f, "get_local_user_quotas {}", mapping)
            }
            Instruction::GetProjectQuota(project, volume) => {
                write!(f, "get_project_quota {} {}", project, volume)
            }
            Instruction::ClearProjectQuota(project, volume) => {
                write!(f, "clear_project_quota {} {}", project, volume)
            }
            Instruction::SetProjectQuota(project, volume, quota) => {
                write!(f, "set_project_quota {} {} {}", project, volume, quota)
            }
            Instruction::GetProjectQuotas(project) => write!(f, "get_project_quotas {}", project),
            Instruction::GetUserQuota(user, volume) => {
                write!(f, "get_user_quota {} {}", user, volume)
            }
            Instruction::ClearUserQuota(user, volume) => {
                write!(f, "clear_user_quota {} {}", user, volume)
            }
            Instruction::SetUserQuota(user, volume, quota) => {
                write!(f, "set_user_quota {} {} {}", user, volume, quota)
            }
            Instruction::GetUserQuotas(user) => write!(f, "get_user_quotas {}", user),
            Instruction::GetLimit(project) => write!(f, "get_limit {}", project),
            Instruction::IsProtectedUser(user) => write!(f, "is_protected_user {}", user),
            Instruction::IsExistingUser(user) => write!(f, "is_existing_user {}", user),
            Instruction::IsExistingProject(project) => {
                write!(f, "is_existing_project {}", project)
            }
            Instruction::GetHomeDir(user) => write!(f, "get_home_dir {}", user),
            Instruction::GetUserDirs(user) => write!(f, "get_user_dirs {}", user),
            Instruction::GetProjectDirs(project) => write!(f, "get_project_dirs {}", project),
            Instruction::GetLocalHomeDir(mapping) => write!(f, "get_local_home_dir {}", mapping),
            Instruction::GetLocalUserDirs(mapping) => write!(f, "get_local_user_dirs {}", mapping),
            Instruction::GetLocalProjectDirs(mapping) => {
                write!(f, "get_local_project_dirs {}", mapping)
            }
            Instruction::GetLocalStorageReport(mapping, date_range) => {
                write!(f, "get_local_storage_report {} {}", mapping, date_range)
            }
            Instruction::SyncOfferings(offerings) => write!(f, "sync_offerings {}", offerings),
            Instruction::AddOfferings(offerings) => write!(f, "add_offerings {}", offerings),
            Instruction::RemoveOfferings(offerings) => write!(f, "remove_offerings {}", offerings),
            Instruction::GetOfferings() => write!(f, "get_offerings"),
        }
    }
}

/// Serialize and Deserialize via the string representation
/// of the Instructionimpl Serialize for Instruction {
impl Serialize for Instruction {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Instruction {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match Instruction::parse(&s) {
            Ok(instruction) => Ok(instruction),
            Err(e) => Err(serde::de::Error::custom(e.to_string())),
        }
    }
}

///
/// The portal that "owns" this instruction, i.e. whose name a job's
/// destination's first hop must match - ported verbatim from the
/// `check_portal` logic that used to live directly inside
/// `templemeads::job::Command::parse` before the command grammar was split
/// out into this crate. Wired up via `Domain::owning_portal` for `Hpc`.
///
pub fn owning_portal(instruction: &Instruction) -> Option<PortalIdentifier> {
    let user = match instruction.clone() {
        Instruction::AddUser(user) => Some(user),
        Instruction::RemoveUser(user) => Some(user),
        Instruction::AddLocalUser(user) => Some(user.user().clone()),
        Instruction::RemoveLocalUser(user) => Some(user.user().clone()),
        Instruction::UpdateHomeDir(user, _) => Some(user),
        Instruction::GetUserMapping(user) => Some(user),
        Instruction::IsProtectedUser(user) => Some(user),
        Instruction::IsExistingUser(user) => Some(user),
        Instruction::GetHomeDir(user) => Some(user),
        Instruction::GetLocalHomeDir(user) => Some(user.user().clone()),
        Instruction::GetUserQuota(user, _) => Some(user),
        Instruction::SetUserQuota(user, _, _) => Some(user),
        Instruction::ClearUserQuota(user, _) => Some(user),
        Instruction::GetUserQuotas(user) => Some(user),
        Instruction::GetLocalUserQuota(user, _) => Some(user.user().clone()),
        Instruction::SetLocalUserQuota(user, _, _) => Some(user.user().clone()),
        Instruction::ClearLocalUserQuota(user, _) => Some(user.user().clone()),
        Instruction::GetLocalUserQuotas(user) => Some(user.user().clone()),
        Instruction::GetUserDirs(user) => Some(user),
        Instruction::GetLocalUserDirs(user) => Some(user.user().clone()),
        // The block/unblock family was missing, so the portal-ownership check
        // silently no-op'd for it - letting one portal's client block or
        // unblock another portal's users. See
        // `docs/specifications/security-review-2.md` (finding R17).
        Instruction::BlockUser(user) => Some(user),
        Instruction::UnblockUser(user) => Some(user),
        Instruction::IsBlockedUser(user) => Some(user),
        _ => None,
    };

    if let Some(user) = user {
        return Some(user.portal_identifier());
    }

    let project = match instruction.clone() {
        Instruction::CreateProject(project, _) => Some(project),
        Instruction::UpdateProject(project, _) => Some(project),
        Instruction::GetProject(project) => Some(project),
        Instruction::GetAward(project) => Some(project),
        Instruction::AddProject(project) => Some(project),
        Instruction::AddLocalProject(project) => Some(project.project().clone()),
        Instruction::RemoveLocalProject(project) => Some(project.project().clone()),
        Instruction::IsExistingProject(project) => Some(project),
        Instruction::GetUsers(project) => Some(project),
        Instruction::RemoveProject(project) => Some(project),
        Instruction::GetUsageReport(project, _) => Some(project),
        Instruction::GetLocalUsageReport(project, _) => Some(project.project().clone()),
        Instruction::GetProjectMapping(project) => Some(project),
        Instruction::GetLocalLimit(project) => Some(project.project().clone()),
        Instruction::SetLocalLimit(project, _) => Some(project.project().clone()),
        Instruction::GetLimit(project) => Some(project),
        Instruction::SetLimit(project, _) => Some(project),
        Instruction::GetProjectDirs(project) => Some(project),
        Instruction::GetLocalProjectDirs(project) => Some(project.project().clone()),
        Instruction::GetProjectQuota(project, _) => Some(project),
        Instruction::SetProjectQuota(project, _, _) => Some(project),
        Instruction::ClearProjectQuota(project, _) => Some(project),
        Instruction::GetProjectQuotas(project) => Some(project),
        Instruction::GetLocalProjectQuota(project, _) => Some(project.project().clone()),
        Instruction::SetLocalProjectQuota(project, _, _) => Some(project.project().clone()),
        Instruction::ClearLocalProjectQuota(project, _) => Some(project.project().clone()),
        Instruction::GetLocalProjectQuotas(project) => Some(project.project().clone()),
        // As above, plus the storage-report family - see finding R17.
        Instruction::BlockProject(project) => Some(project),
        Instruction::UnblockProject(project) => Some(project),
        Instruction::IsBlockedProject(project) => Some(project),
        Instruction::GetStorageReport(project, _) => Some(project),
        Instruction::GetLocalStorageReport(project, _) => Some(project.project().clone()),
        _ => None,
    };

    if let Some(project) = project {
        return Some(project.portal_identifier());
    }

    match instruction.clone() {
        Instruction::GetProjects(portal) => Some(portal),
        Instruction::GetUsageReports(portal, _) => Some(portal),
        // Also missing - see finding R17.
        Instruction::GetAwards(portal) => Some(portal),
        Instruction::GetStorageReports(portal, _) => Some(portal),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_identifier() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        assert_eq!(user.username(), "user");
        assert_eq!(user.project(), "project");
        assert_eq!(user.portal(), "portal");
        assert_eq!(user.to_string(), "user.project.portal");
    }

    #[test]
    fn test_user_mapping() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        #[allow(clippy::unwrap_used)]
        let mapping = UserMapping::new(&user, "local_user", "local_group").unwrap();
        assert_eq!(mapping.user(), &user);
        assert_eq!(mapping.local_user().as_str(), "local_user");
        assert_eq!(mapping.local_group(), "local_group");
        assert_eq!(
            mapping.to_string(),
            "user.project.portal:local_user:local_group"
        );
    }

    #[test]
    fn test_mapping_targets_reject_argument_injection_characters() {
        // Regression test for finding R14. Mapping targets had a deny-list that
        // still permitted whitespace, `,`, `=`, `%`, `?` and `#`. Those matter
        // because a mapping is not only a spawned tool's operand: `cluster`
        // rebuilds instructions by interpolating it into a *space-delimited*
        // string, so a space shifts every later argument - letting a
        // compromised account agent choose the limit the scheduler applies.
        let project = ProjectIdentifier::parse("proj.portal")
            .unwrap_or_else(|e| unreachable!("project: {:?}", e));
        let user = UserIdentifier::parse("bob.proj.portal")
            .unwrap_or_else(|e| unreachable!("user: {:?}", e));

        // The legitimate shapes still work - a local account named after
        // user.project, and a plain group name.
        assert!(ProjectMapping::new(&project, "grp").is_ok());
        assert!(ProjectMapping::new(&project, "portal.proj").is_ok());
        assert!(UserMapping::new(&user, "bob.proj", "grp").is_ok());

        for bad in [
            "grp evil",            // shifts every later instruction argument
            "grp\tevil",           // ditto
            "a,b",                 // sacctmgr list separator
            "a=b",                 // sacctmgr key=value
            "a?with_deleted=true", // Slurm REST query injection
            "a#b",
            "a%2fb",
            "a/b",
            "-grp",
            ".grp",
            "grp.",
            "a..b",
            "",
        ] {
            assert!(
                ProjectMapping::new(&project, bad).is_err(),
                "ProjectMapping must reject local_group {:?}",
                bad
            );
            assert!(
                UserMapping::new(&user, bad, "grp").is_err(),
                "UserMapping must reject local_user {:?}",
                bad
            );
            assert!(
                UserMapping::new(&user, "bob.proj", bad).is_err(),
                "UserMapping must reject local_group {:?}",
                bad
            );
        }

        // A mapping is also length-capped, like every other component.
        assert!(ProjectMapping::new(&project, &"x".repeat(65)).is_err());
    }

    #[test]
    fn test_mapping_round_trips_through_its_own_wire_form() {
        // The concrete consequence of the above: a mapping is serialised into a
        // space-delimited instruction and re-parsed positionally, so any
        // accepted mapping must survive that round trip unchanged.
        let project = ProjectIdentifier::parse("proj.portal")
            .unwrap_or_else(|e| unreachable!("project: {:?}", e));
        let mapping = ProjectMapping::new(&project, "portal.proj")
            .unwrap_or_else(|e| unreachable!("mapping: {:?}", e));

        let reparsed = ProjectMapping::parse(&mapping.to_string())
            .unwrap_or_else(|e| unreachable!("reparse: {:?}", e));
        assert_eq!(mapping, reparsed);

        // And the interpolated-instruction form that `cluster` builds parses
        // back to the same three arguments rather than a shifted set.
        let command = format!("set_local_limit {} {}", mapping, 3600);
        match Instruction::parse(&command) {
            Ok(Instruction::SetLocalLimit(parsed, usage)) => {
                assert_eq!(parsed, mapping);
                assert_eq!(usage.seconds(), 3600);
            }
            other => unreachable!("expected SetLocalLimit, got {:?}", other),
        }
    }

    #[test]
    fn test_link_urls_are_restricted_to_http_schemes() {
        // These links are documented as being for display in a portal UI, and
        // `Url::parse` happily accepts `javascript:`, `data:` and `file:` - a
        // stored-XSS or local-file-read primitive if any consumer renders one as an
        // anchor. See finding R33.
        let mut link = Link::default();

        assert!(link.set_url("https://example.org/award/1").is_ok());
        assert!(link.set_url("http://example.org/award/1").is_ok());

        // empty clears rather than errors, as before
        assert!(link.set_url("").is_ok());
        assert_eq!(link.url(), None);

        for bad in [
            "javascript:alert(1)",
            "data:text/html;base64,PHNjcmlwdD4=",
            "file:///etc/passwd",
            "ftp://example.org/x",
        ] {
            assert!(
                link.set_url(bad).is_err(),
                "{:?} must be rejected as a link URL",
                bad
            );
        }

        // The wire path must agree with the programmatic one - it used to re-validate
        // separately with a plain `Url::parse` and so missed the allow-list.
        assert!(serde_json::from_str::<Link>(r#"{"url":"https://example.org"}"#).is_ok());
        for bad in [
            r#"{"url":"javascript:alert(1)"}"#,
            r#"{"url":"file:///etc/passwd"}"#,
        ] {
            assert!(
                serde_json::from_str::<Link>(bad).is_err(),
                "{} must be rejected on the wire too",
                bad
            );
        }
    }

    #[test]
    fn test_allocation_rejects_non_finite_sizes() {
        // `size < 0.0` is *false* for NaN, so "NaN" and "inf" both parsed
        // cleanly and then saturated to u64::MAX downstream (finding R33).
        assert!(Allocation::parse("10 GB").is_ok());
        assert!(Allocation::parse("0 GB").is_ok());

        for bad in ["NaN GB", "nan GB", "inf GB", "-inf GB", "infinity GB"] {
            assert!(
                Allocation::parse(bad).is_err(),
                "{:?} must be rejected",
                bad
            );
        }

        assert!(Allocation::parse("-1 GB").is_err());
    }

    #[test]
    fn test_date_parsing_is_bounded() {
        // Regression test for finding R25. `%Y` accepts a signed, unbounded
        // digit count, so the whole chrono range used to parse - and a range
        // spanning it made `days()` try to build a 191-million-element Vec.
        assert!(Date::parse("2026-07-30").is_ok());
        assert!(Date::parse("1970-01-01").is_ok());
        assert!(Date::parse("2200-12-31").is_ok());

        for out_of_range in [
            "+262142-12-31",
            "-262143-01-01",
            "0001-01-01",
            "1969-12-31",
            "2201-01-01",
            "+10000-01-01",
        ] {
            assert!(
                Date::parse(out_of_range).is_err(),
                "{:?} must be rejected as out of range",
                out_of_range
            );
        }
    }

    #[test]
    fn test_date_range_span_is_capped() {
        // A plausible reporting query still works...
        assert!(DateRange::parse("2026-01-01:2026-12-31").is_ok());
        assert!(DateRange::parse("2026-01-01").is_ok());

        // ...while a span beyond the cap is refused rather than turned into a
        // multi-hundred-megabyte allocation (finding R25).
        assert!(DateRange::parse("1970-01-01:2200-12-31").is_err());
        assert!(DateRange::parse("2026-01-01:2100-01-01").is_err());
    }

    #[test]
    fn test_date_range_iteration_terminates_at_the_range_boundary() {
        // `months()` and `years()` looped forever at the top of the
        // representable range: `from_ymd_opt(year + 1, 1, 1)` returns `None`,
        // the fallback produced an end date *earlier* than the cursor, and the
        // cursor therefore never advanced - while pushing a `DateRange` per
        // iteration. `Date::parse` now bounds the year, but `from_chrono`
        // bypasses that, so the loops must terminate on their own (finding
        // R25).
        let top = chrono::NaiveDate::MAX;
        let range = DateRange::from_chrono(&top, &top);

        // Each of these must return, and must not grow without bound.
        assert!(range.days().len() <= 1);
        assert!(range.weeks().len() <= 1);
        assert!(range.months().len() <= 1);
        assert!(range.years().len() <= 1);

        // ...and must not panic computing the exclusive end instant.
        let _ = range.end_time();

        // The same at the bottom of the range.
        let bottom = chrono::NaiveDate::MIN;
        let range = DateRange::from_chrono(&bottom, &bottom);
        assert!(range.days().len() <= 1);
        assert!(range.months().len() <= 1);
        assert!(range.years().len() <= 1);
        let _ = range.end_time();

        // A normal range still iterates correctly - the guard must not have
        // broken the ordinary case.
        let range = DateRange::parse("2026-01-01:2026-03-31")
            .unwrap_or_else(|e| unreachable!("range: {:?}", e));
        assert_eq!(range.days().len(), 90);
        assert_eq!(range.months().len(), 3);
        assert_eq!(range.years().len(), 1);
    }

    #[test]
    fn test_owning_portal_covers_every_identifier_bearing_instruction() {
        // Regression test for finding R17. `Command::parse(.., check_portal =
        // true)` enforces "an instruction naming portal X may only be issued
        // via a destination whose first agent is X" - and it is silently
        // skipped wherever `owning_portal` returns `None`. Ten
        // identifier-bearing variants were missing, including the whole
        // block/unblock family, so a bridge client of one portal could block
        // another portal's projects.
        //
        // This test enumerates every variant explicitly rather than sampling,
        // so adding a new identifier-bearing instruction without an
        // `owning_portal` arm fails here instead of quietly losing the check.
        let user = UserIdentifier::parse("bob.proj.brics")
            .unwrap_or_else(|e| unreachable!("user: {:?}", e));
        let project = ProjectIdentifier::parse("proj.brics")
            .unwrap_or_else(|e| unreachable!("project: {:?}", e));
        let portal =
            PortalIdentifier::parse("brics").unwrap_or_else(|e| unreachable!("portal: {:?}", e));
        let user_mapping = UserMapping::new(&user, "bob.proj", "grp")
            .unwrap_or_else(|e| unreachable!("user mapping: {:?}", e));
        let project_mapping = ProjectMapping::new(&project, "grp")
            .unwrap_or_else(|e| unreachable!("project mapping: {:?}", e));
        let dates = DateRange::parse("2026-01-01:2026-01-31")
            .unwrap_or_else(|e| unreachable!("dates: {:?}", e));
        let volume = Volume::parse("home").unwrap_or_else(|e| unreachable!("volume: {:?}", e));
        let quota = QuotaLimit::parse("1 GB").unwrap_or_else(|e| unreachable!("quota: {:?}", e));
        let usage = Usage::new(3600);
        let details = ProjectDetails::default();
        let homedir = "/home/bob.proj".to_string();

        // Every variant that names a user, project or portal, with the portal
        // each one should resolve to.
        let identifier_bearing: Vec<Instruction> = vec![
            Instruction::AddUser(user.clone()),
            Instruction::RemoveUser(user.clone()),
            Instruction::AddLocalUser(user_mapping.clone()),
            Instruction::RemoveLocalUser(user_mapping.clone()),
            Instruction::UpdateHomeDir(user.clone(), homedir.clone()),
            Instruction::GetUserMapping(user.clone()),
            Instruction::IsProtectedUser(user.clone()),
            Instruction::IsExistingUser(user.clone()),
            Instruction::GetHomeDir(user.clone()),
            Instruction::GetLocalHomeDir(user_mapping.clone()),
            Instruction::GetUserQuota(user.clone(), volume.clone()),
            Instruction::SetUserQuota(user.clone(), volume.clone(), quota.clone()),
            Instruction::ClearUserQuota(user.clone(), volume.clone()),
            Instruction::GetUserQuotas(user.clone()),
            Instruction::GetLocalUserQuota(user_mapping.clone(), volume.clone()),
            Instruction::SetLocalUserQuota(user_mapping.clone(), volume.clone(), quota.clone()),
            Instruction::ClearLocalUserQuota(user_mapping.clone(), volume.clone()),
            Instruction::GetLocalUserQuotas(user_mapping.clone()),
            Instruction::GetUserDirs(user.clone()),
            Instruction::GetLocalUserDirs(user_mapping.clone()),
            Instruction::BlockUser(user.clone()),
            Instruction::UnblockUser(user.clone()),
            Instruction::IsBlockedUser(user.clone()),
            Instruction::CreateProject(project.clone(), details.clone()),
            Instruction::UpdateProject(project.clone(), details.clone()),
            Instruction::GetProject(project.clone()),
            Instruction::GetAward(project.clone()),
            Instruction::AddProject(project.clone()),
            Instruction::AddLocalProject(project_mapping.clone()),
            Instruction::RemoveLocalProject(project_mapping.clone()),
            Instruction::IsExistingProject(project.clone()),
            Instruction::GetUsers(project.clone()),
            Instruction::RemoveProject(project.clone()),
            Instruction::GetUsageReport(project.clone(), dates.clone()),
            Instruction::GetLocalUsageReport(project_mapping.clone(), dates.clone()),
            Instruction::GetProjectMapping(project.clone()),
            Instruction::GetLocalLimit(project_mapping.clone()),
            Instruction::SetLocalLimit(project_mapping.clone(), usage),
            Instruction::GetLimit(project.clone()),
            Instruction::SetLimit(project.clone(), usage),
            Instruction::GetProjectDirs(project.clone()),
            Instruction::GetLocalProjectDirs(project_mapping.clone()),
            Instruction::GetProjectQuota(project.clone(), volume.clone()),
            Instruction::SetProjectQuota(project.clone(), volume.clone(), quota.clone()),
            Instruction::ClearProjectQuota(project.clone(), volume.clone()),
            Instruction::GetProjectQuotas(project.clone()),
            Instruction::GetLocalProjectQuota(project_mapping.clone(), volume.clone()),
            Instruction::SetLocalProjectQuota(project_mapping.clone(), volume.clone(), quota),
            Instruction::ClearLocalProjectQuota(project_mapping.clone(), volume),
            Instruction::GetLocalProjectQuotas(project_mapping.clone()),
            Instruction::BlockProject(project.clone()),
            Instruction::UnblockProject(project.clone()),
            Instruction::IsBlockedProject(project.clone()),
            Instruction::GetStorageReport(project.clone(), dates.clone()),
            Instruction::GetLocalStorageReport(project_mapping, dates.clone()),
            Instruction::GetProjects(portal.clone()),
            Instruction::GetUsageReports(portal.clone(), dates.clone()),
            Instruction::GetAwards(portal.clone()),
            Instruction::GetStorageReports(portal.clone(), dates),
        ];

        for instruction in identifier_bearing {
            assert_eq!(
                owning_portal(&instruction),
                Some(portal.clone()),
                "owning_portal must resolve '{}' to its portal, or the \
                 portal-ownership check silently does not apply to it",
                instruction.command()
            );
        }

        // The variants that genuinely carry no identifier must stay `None` -
        // `Submit` wraps another instruction (checked on its own when parsed),
        // and the offerings family is addressed by destination, not by portal.
        for instruction in [
            Instruction::GetOfferings(),
            Instruction::AddOfferings(Destinations::default()),
            Instruction::RemoveOfferings(Destinations::default()),
            Instruction::SyncOfferings(Destinations::default()),
        ] {
            assert_eq!(
                owning_portal(&instruction),
                None,
                "'{}' carries no identifier and should have no owning portal",
                instruction.command()
            );
        }
    }

    #[test]
    fn test_instruction_parse_never_panics_on_missing_arguments() {
        // Regression test for finding R1. Four arms of `Instruction::parse`
        // indexed `parts` without a length guard, so an instruction keyword
        // with fewer arguments than that arm expected panicked - and because
        // `Command`'s `Deserialize` runs this parser on the `command` string of
        // every incoming `Job`, and the release profile sets `panic = "abort"`,
        // a ~200-byte message from any authenticated peer terminated the
        // process. Every one of these must be a clean `Err`.
        let truncated = [
            "submit",
            "create_project",
            "create_award",
            "update_project",
            "update_award",
            // two tokens: enough for `parts[1]` but not for `parts[2..]`/`[3..]`
            "create_project proj.portal",
            "update_project proj.portal",
            "create_award proj.portal",
            "submit a.b",
        ];

        for command in truncated {
            assert!(
                Instruction::parse(command).is_err(),
                "'{}' must be a parse error, not a panic",
                command
            );
        }

        // Nor on an empty or whitespace-only instruction.
        assert!(Instruction::parse("").is_err());
        assert!(Instruction::parse(" ").is_err());
        assert!(Instruction::parse("   ").is_err());

        // Exhaustive sweep: every keyword the parser recognises, given 0, 1
        // and 2 arguments, must either parse or error - never panic. This is
        // what stops the same class of bug returning in a future arm.
        let keywords = [
            "submit",
            "create_project",
            "create_award",
            "update_project",
            "update_award",
            "get_project",
            "get_projects",
            "get_award",
            "get_awards",
            "add_project",
            "remove_project",
            "remove_award",
            "add_local_project",
            "remove_local_project",
            "add_user",
            "remove_user",
            "add_local_user",
            "remove_local_user",
            "get_usage_report",
            "get_local_usage_report",
            "get_limit",
            "set_limit",
            "get_local_limit",
            "set_local_limit",
            "block_user",
            "unblock_user",
            "is_blocked_user",
            "block_project",
            "unblock_project",
            "is_blocked_project",
            "get_storage_report",
            "get_home_dir",
            "update_home_dir",
            "get_offerings",
            "add_offerings",
            "remove_offerings",
            "sync_offerings",
            "not_a_real_instruction",
        ];

        for keyword in keywords {
            for args in ["", " x", " x y", " x y z"] {
                let command = format!("{}{}", keyword, args);
                // The result is irrelevant - not panicking is the assertion.
                let _ = Instruction::parse(&command);
            }
        }
    }

    #[test]
    fn test_identifier_validation_rejects_dangerous_characters() {
        // Legitimate identifiers still parse.
        assert!(UserIdentifier::parse("user.project.portal").is_ok());
        assert!(UserIdentifier::parse("a-b_c.proj-1.brics").is_ok());
        assert!(ProjectIdentifier::parse("project.portal").is_ok());

        // Path separators (traversal / absolute-path escape) are rejected.
        assert!(ProjectIdentifier::parse("/etc/cron.portal").is_err());
        assert!(UserIdentifier::parse("us/er.project.portal").is_err());

        // A leading '-' (argument injection into spawned tools) is rejected.
        assert!(UserIdentifier::parse("-rf.project.portal").is_err());
        assert!(ProjectIdentifier::parse("project.-g").is_err());

        // Shell/quoting metacharacters and whitespace are rejected.
        for bad in [
            "a;b.project.portal",
            "a b.project.portal",
            "a$b.project.portal",
            "a\tb.project.portal",
        ] {
            assert!(UserIdentifier::parse(bad).is_err(), "should reject {bad:?}");
        }

        // Over-length components are rejected; exactly at the limit is fine.
        let at_limit = "a".repeat(templemeads::validate::MAX_IDENTIFIER_COMPONENT_LEN);
        let over_limit = "a".repeat(templemeads::validate::MAX_IDENTIFIER_COMPONENT_LEN + 1);
        assert!(ProjectIdentifier::parse(&format!("{at_limit}.portal")).is_ok());
        assert!(ProjectIdentifier::parse(&format!("{over_limit}.portal")).is_err());
    }

    #[test]
    fn test_mapping_validation_rejects_dangerous_local_names() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        #[allow(clippy::unwrap_used)]
        let project = ProjectIdentifier::parse("project.portal").unwrap();

        // A '.'-containing local group is still allowed (a portal with no Unix
        // groups of its own reuses "project.portal" as a placeholder group).
        assert!(ProjectMapping::new(&project, "project.portal").is_ok());

        // Path separators and leading dashes in local names are rejected.
        assert!(UserMapping::new(&user, "-rf", "local_group").is_err());
        assert!(UserMapping::new(&user, "local_user", "grp/../x").is_err());
        assert!(ProjectMapping::new(&project, "-g").is_err());
        assert!(ProjectMapping::new(&project, "a/b").is_err());
    }

    #[test]
    fn test_user_mapping_accepts_an_email_but_keeps_it_away_from_unix_use() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("alice.project.portal").unwrap();

        // A portal reports the member's email as the local user - this is what
        // `get_users` returns from `op-portal`, and it was rejected outright
        // before `LocalUser` existed.
        let mapping = UserMapping::new(&user, "alice@example.com", "project.portal")
            .unwrap_or_else(|e| unreachable!("an email local_user must parse: {:?}", e));

        assert!(mapping.local_user().is_email());
        assert_eq!(mapping.local_user().as_str(), "alice@example.com");

        // ...but it is not a Unix account name, and the only accessor that
        // yields one says so.
        assert!(mapping.local_user().unix().is_err());

        // The wire form is unchanged - still `user:local_user:local_group` -
        // and survives the round trip through a space-delimited instruction.
        assert_eq!(
            mapping.to_string(),
            "alice.project.portal:alice@example.com:project.portal"
        );
        assert_eq!(
            UserMapping::parse(&mapping.to_string())
                .unwrap_or_else(|e| unreachable!("reparse: {:?}", e)),
            mapping
        );

        // An account agent's mapping still yields a usable Unix name.
        let unix = UserMapping::new(&user, "alice.project", "project.portal")
            .unwrap_or_else(|e| unreachable!("a Unix local_user must parse: {:?}", e));
        assert_eq!(
            unix.local_user()
                .unix()
                .unwrap_or_else(|e| unreachable!("unix: {:?}", e)),
            "alice.project"
        );

        // `local_group` is not widened: it names a Unix group at every layer.
        assert!(UserMapping::new(&user, "alice@example.com", "grp@example.com").is_err());

        // And the email form does not become a hole in the wire parser - a
        // mapping whose address is malformed is still rejected.
        for bad in [
            "alice.project.portal:alice evil@example.com:project.portal",
            "alice.project.portal:alice@localhost:project.portal",
            "alice.project.portal:a,b@example.com:project.portal",
        ] {
            assert!(
                UserMapping::parse(bad).is_err(),
                "{:?} must be rejected",
                bad
            );
        }
    }

    #[test]
    fn test_award_aliases_parse_to_the_project_instructions() {
        // The `*_award` spellings are the vocabulary a portal-to-portal caller
        // uses; they must be exact synonyms of the `*_project` forms rather
        // than a parallel set of instructions.
        #[allow(clippy::unwrap_used)]
        let project = ProjectIdentifier::parse("proj.portal").unwrap();

        assert_eq!(
            Instruction::parse("remove_award proj.portal")
                .unwrap_or_else(|e| unreachable!("remove_award: {:?}", e)),
            Instruction::RemoveProject(project.clone())
        );

        for (award, canonical) in [
            ("remove_award proj.portal", "remove_project proj.portal"),
            ("get_award proj.portal", "get_award proj.portal"),
            (
                "create_award proj.portal {\"name\":\"p\"}",
                "create_project proj.portal {\"name\":\"p\"}",
            ),
            (
                "update_award proj.portal {\"name\":\"p\"}",
                "update_project proj.portal {\"name\":\"p\"}",
            ),
        ] {
            assert_eq!(
                Instruction::parse(award).unwrap_or_else(|e| unreachable!("{}: {:?}", award, e)),
                Instruction::parse(canonical)
                    .unwrap_or_else(|e| unreachable!("{}: {:?}", canonical, e)),
                "'{}' must parse the same as '{}'",
                award,
                canonical
            );
        }

        // The canonical spelling is what goes back out on the wire, so an
        // alias cannot leak into a destination or a log line.
        assert_eq!(
            Instruction::RemoveProject(project).command(),
            "remove_project"
        );
    }

    #[test]
    fn test_instruction() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        #[allow(clippy::unwrap_used)]
        let mapping = UserMapping::new(&user, "local_user", "local_group").unwrap();

        #[allow(clippy::unwrap_used)]
        let instruction = Instruction::parse("add_user user.project.portal").unwrap();
        assert_eq!(instruction, Instruction::AddUser(user.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction = Instruction::parse("remove_user user.project.portal").unwrap();
        assert_eq!(instruction, Instruction::RemoveUser(user.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction =
            Instruction::parse("add_local_user user.project.portal:local_user:local_group")
                .unwrap();
        assert_eq!(instruction, Instruction::AddLocalUser(mapping.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction =
            Instruction::parse("remove_local_user user.project.portal:local_user:local_group")
                .unwrap();
        assert_eq!(instruction, Instruction::RemoveLocalUser(mapping.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction =
            Instruction::parse("update_homedir user.project.portal /home/user").unwrap();
        assert_eq!(
            instruction,
            Instruction::UpdateHomeDir(user.clone(), "/home/user".to_string())
        );
    }

    #[test]
    fn assert_serialize_user() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        let serialized = serde_json::to_string(&user).unwrap_or_default();
        assert_eq!(serialized, "\"user.project.portal\"");
    }

    #[test]
    fn assert_deserialize_user() {
        #[allow(clippy::unwrap_used)]
        let user: UserIdentifier = serde_json::from_str("\"user.project.portal\"").unwrap();
        assert_eq!(user.to_string(), "user.project.portal");
    }

    #[test]
    fn assert_serialize_mapping() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        #[allow(clippy::unwrap_used)]
        let mapping = UserMapping::new(&user, "local_user", "local_group").unwrap();
        let serialized = serde_json::to_string(&mapping).unwrap_or_default();
        assert_eq!(serialized, "\"user.project.portal:local_user:local_group\"");
    }

    #[test]
    fn assert_deserialize_mapping() {
        #[allow(clippy::unwrap_used)]
        let mapping: UserMapping =
            serde_json::from_str("\"user.project.portal:local_user:local_group\"").unwrap();
        assert_eq!(
            mapping.to_string(),
            "user.project.portal:local_user:local_group"
        );
    }

    #[test]
    fn assert_serialize_instruction() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        #[allow(clippy::unwrap_used)]
        let mapping = UserMapping::new(&user, "local_user", "local_group").unwrap();

        let instruction = Instruction::AddUser(user.clone());
        let serialized = serde_json::to_string(&instruction).unwrap_or_default();
        assert_eq!(serialized, "\"add_user user.project.portal\"");

        let instruction = Instruction::RemoveUser(user.clone());
        let serialized = serde_json::to_string(&instruction).unwrap_or_default();
        assert_eq!(serialized, "\"remove_user user.project.portal\"");

        let instruction = Instruction::AddLocalUser(mapping.clone());
        let serialized = serde_json::to_string(&instruction).unwrap_or_default();
        assert_eq!(
            serialized,
            "\"add_local_user user.project.portal:local_user:local_group\""
        );

        let instruction = Instruction::RemoveLocalUser(mapping.clone());
        let serialized = serde_json::to_string(&instruction).unwrap_or_default();
        assert_eq!(
            serialized,
            "\"remove_local_user user.project.portal:local_user:local_group\""
        );

        let instruction = Instruction::UpdateHomeDir(user.clone(), "/home/user".to_string());
        let serialized = serde_json::to_string(&instruction).unwrap_or_default();
        assert_eq!(
            serialized,
            "\"update_homedir user.project.portal /home/user\""
        );
    }

    #[test]
    fn assert_deserialize_instruction() {
        #[allow(clippy::unwrap_used)]
        let user = UserIdentifier::parse("user.project.portal").unwrap();
        #[allow(clippy::unwrap_used)]
        let mapping = UserMapping::new(&user, "local_user", "local_group").unwrap();

        #[allow(clippy::unwrap_used)]
        let instruction: Instruction =
            serde_json::from_str("\"add_user user.project.portal\"").unwrap();
        assert_eq!(instruction, Instruction::AddUser(user.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction: Instruction =
            serde_json::from_str("\"remove_user user.project.portal\"").unwrap();
        assert_eq!(instruction, Instruction::RemoveUser(user.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction: Instruction =
            serde_json::from_str("\"add_local_user user.project.portal:local_user:local_group\"")
                .unwrap();
        assert_eq!(instruction, Instruction::AddLocalUser(mapping.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction: Instruction = serde_json::from_str(
            "\"remove_local_user user.project.portal:local_user:local_group\"",
        )
        .unwrap();
        assert_eq!(instruction, Instruction::RemoveLocalUser(mapping.clone()));

        #[allow(clippy::unwrap_used)]
        let instruction: Instruction =
            serde_json::from_str("\"update_homedir user.project.portal /home/user\"").unwrap();
        assert_eq!(
            instruction,
            Instruction::UpdateHomeDir(user.clone(), "/home/user".to_string())
        );
    }

    #[test]
    fn test_domain_pattern_wildcard_depth() {
        #[allow(clippy::unwrap_used)]
        {
            let p = DomainPattern::parse("*.ac.uk").unwrap();
            assert!(p.matches("bristol.ac.uk"));
            assert!(p.matches("cs.bristol.ac.uk"));
            assert!(p.matches("dept.cs.bristol.ac.uk"));
            assert!(!p.matches("ac.uk"));
            assert!(!p.matches("notac.uk"));
        }
    }

    #[test]
    fn test_domain_pattern_parse() {
        #[allow(clippy::unwrap_used)]
        {
            assert!(DomainPattern::parse("example.com").is_ok());
            assert!(DomainPattern::parse("*.example.com").is_ok());
            assert!(DomainPattern::parse("chris@gmail.com").is_ok());
            assert!(DomainPattern::parse("Chris.Woods@bristol.ac.uk").is_ok());
        }
        assert!(DomainPattern::parse("").is_err());
        assert!(DomainPattern::parse("@gmail.com").is_err());
        assert!(DomainPattern::parse("chris@").is_err());
        assert!(DomainPattern::parse("chris@@gmail.com").is_err());
        assert!(DomainPattern::parse("*.*.com").is_err());
    }

    #[test]
    fn test_domain_pattern_is_email_pattern() {
        #[allow(clippy::unwrap_used)]
        {
            assert!(!DomainPattern::parse("example.com")
                .unwrap()
                .is_email_pattern());
            assert!(!DomainPattern::parse("*.example.com")
                .unwrap()
                .is_email_pattern());
            assert!(DomainPattern::parse("chris@gmail.com")
                .unwrap()
                .is_email_pattern());
        }
    }

    #[test]
    fn test_domain_pattern_matches_email() {
        #[allow(clippy::unwrap_used)]
        {
            let email_pattern = DomainPattern::parse("chris@gmail.com").unwrap();
            assert!(email_pattern.matches_email("chris@gmail.com"));
            assert!(email_pattern.matches_email("Chris@Gmail.COM"));
            assert!(!email_pattern.matches_email("other@gmail.com"));
            assert!(!email_pattern.matches_email("chris@example.com"));

            // domain patterns never match via matches_email
            let domain_pattern = DomainPattern::parse("*.gmail.com").unwrap();
            assert!(!domain_pattern.matches_email("chris@gmail.com"));
        }
    }

    #[test]
    fn test_validate_email_address() {
        assert!(validate_email_address("alice@example.com").is_ok());
        assert!(validate_email_address("chris.woods+tag@bristol.ac.uk").is_ok());
        assert!(validate_email_address("").is_err());
        assert!(validate_email_address("notanemail").is_err());
        assert!(validate_email_address("@example.com").is_err());
        assert!(validate_email_address("alice@").is_err());
        assert!(validate_email_address("alice@@example.com").is_err());
    }

    #[test]
    fn test_add_member_validation() {
        #[allow(clippy::unwrap_used)]
        {
            let mut details = AwardDetails::default();

            // Basic valid add
            assert!(details.add_member("alice@example.com", "member").is_ok());

            // Non-email strings are errors
            assert!(details.add_member("notanemail", "member").is_err());
            assert!(details.add_member("", "member").is_err());
            assert!(details.add_member("alice@example.com", "").is_err());

            // Domain restriction enforced
            details.add_allowed_domain(DomainPattern::parse("*.example.com").unwrap());
            assert!(details.add_member("bob@sub.example.com", "member").is_ok());
            assert!(details.add_member("chris@gmail.com", "member").is_err());

            // Explicit email allowance overrides domain restriction
            details.add_allowed_domain(DomainPattern::parse("chris@gmail.com").unwrap());
            assert!(details.add_member("chris@gmail.com", "member").is_ok());
        }
    }

    #[test]
    fn test_set_members_atomic() {
        #[allow(clippy::unwrap_used)]
        {
            let mut details = AwardDetails::default();
            details.add_allowed_domain(DomainPattern::parse("example.com").unwrap());

            // Seed an existing member
            details.add_member("alice@example.com", "pi").unwrap();

            // set_members with one invalid entry must leave members unchanged
            let mut bad = BTreeMap::new();
            bad.insert("bob@example.com".to_string(), "member".to_string());
            bad.insert("intruder@evil.com".to_string(), "member".to_string());
            assert!(details.set_members(bad).is_err());
            // alice is still there, bob was not added
            let members = details.members().unwrap();
            assert!(members.contains_key("alice@example.com"));
            assert!(!members.contains_key("bob@example.com"));

            // set_members with all valid entries replaces atomically
            let mut good = BTreeMap::new();
            good.insert("bob@example.com".to_string(), "member".to_string());
            assert!(details.set_members(good).is_ok());
            let members = details.members().unwrap();
            assert!(members.contains_key("bob@example.com"));
            assert!(!members.contains_key("alice@example.com"));
        }
    }

    #[test]
    fn test_add_members_atomic() {
        #[allow(clippy::unwrap_used)]
        {
            let mut details = AwardDetails::default();
            details.add_allowed_domain(DomainPattern::parse("example.com").unwrap());
            details.add_member("alice@example.com", "pi").unwrap();

            // add_members with one invalid entry must leave members unchanged
            let mut bad = BTreeMap::new();
            bad.insert("bob@example.com".to_string(), "member".to_string());
            bad.insert("intruder@evil.com".to_string(), "member".to_string());
            assert!(details.add_members(bad).is_err());
            let members = details.members().unwrap();
            assert!(members.contains_key("alice@example.com"));
            assert!(!members.contains_key("bob@example.com"));

            // add_members with all valid entries adds without replacing alice
            let mut good = BTreeMap::new();
            good.insert("bob@example.com".to_string(), "member".to_string());
            assert!(details.add_members(good).is_ok());
            let members = details.members().unwrap();
            assert!(members.contains_key("alice@example.com"));
            assert!(members.contains_key("bob@example.com"));
        }
    }

    #[test]
    fn test_is_email_allowed() {
        #[allow(clippy::unwrap_used)]
        {
            let mut details = AwardDetails::default();

            // None = all allowed
            assert!(details.is_email_allowed("anyone@anywhere.com"));

            // Add patterns: wildcard domain + specific email
            details.add_allowed_domain(DomainPattern::parse("*.example.com").unwrap());
            details.add_allowed_domain(DomainPattern::parse("chris@gmail.com").unwrap());

            // Subdomain matches via domain pattern
            assert!(details.is_email_allowed("user@sub.example.com"));
            // Exact email match (and case-insensitive)
            assert!(details.is_email_allowed("chris@gmail.com"));
            assert!(details.is_email_allowed("CHRIS@GMAIL.COM"));
            // Different user at gmail is not allowed
            assert!(!details.is_email_allowed("other@gmail.com"));
            // Wildcard *.example.com does not match bare example.com
            assert!(!details.is_email_allowed("user@example.com"));
        }
    }
}
