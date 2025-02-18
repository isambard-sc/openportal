// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::destination::Destination;
use crate::error::Error;
use crate::usagereport::Usage;

use anyhow::Context;
use chrono::Datelike;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub trait NamedType {
    fn type_name() -> &'static str;
}

impl NamedType for String {
    fn type_name() -> &'static str {
        "String"
    }
}

impl NamedType for bool {
    fn type_name() -> &'static str {
        "bool"
    }
}

impl NamedType for Vec<String> {
    fn type_name() -> &'static str {
        "Vec<String>"
    }
}

///
/// A portal identifier - this is just a string with no spaces or periods
///
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PortalIdentifier {
    portal: String,
}

impl NamedType for PortalIdentifier {
    fn type_name() -> &'static str {
        "PortalIdentifier"
    }
}

impl NamedType for Vec<PortalIdentifier> {
    fn type_name() -> &'static str {
        "Vec<PortalIdentifier>"
    }
}

impl PortalIdentifier {
    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let portal = identifier.trim();

        if portal.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid PortalIdentifier - portal cannot be empty '{}'",
                identifier
            )));
        };

        if portal.contains(' ') || portal.contains('.') {
            return Err(Error::Parse(format!(
                "Invalid PortalIdentifier - portal cannot contain spaces or periods '{}'",
                identifier
            )));
        };

        Ok(Self {
            portal: portal.to_string(),
        })
    }

    pub fn portal(&self) -> String {
        self.portal.clone()
    }
}

impl std::fmt::Display for PortalIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.portal)
    }
}

/// Serialize and Deserialize via the string representation
impl Serialize for PortalIdentifier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PortalIdentifier {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl From<ProjectIdentifier> for PortalIdentifier {
    fn from(project: ProjectIdentifier) -> Self {
        project.portal_identifier()
    }
}

impl From<UserIdentifier> for PortalIdentifier {
    fn from(user: UserIdentifier) -> Self {
        user.portal_identifier()
    }
}

///
/// A project identifier - this is a double of project.portal
///
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectIdentifier {
    project: String,
    portal: String,
}

impl NamedType for ProjectIdentifier {
    fn type_name() -> &'static str {
        "ProjectIdentifier"
    }
}

impl NamedType for Vec<ProjectIdentifier> {
    fn type_name() -> &'static str {
        "Vec<ProjectIdentifier>"
    }
}

impl ProjectIdentifier {
    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = identifier.split('.').collect();

        if parts.len() != 2 {
            return Err(Error::Parse(format!(
                "Invalid ProjectIdentifier: {}",
                identifier
            )));
        }

        let project = parts[0].trim();
        let portal = parts[1].trim();

        if project.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid ProjectIdentifier - project cannot be empty '{}'",
                identifier
            )));
        };

        if portal.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid ProjectIdentifier - portal cannot be empty '{}'",
                identifier
            )));
        };

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
        PortalIdentifier {
            portal: self.portal.clone(),
        }
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
    fn type_name() -> &'static str {
        "UserIdentifier"
    }
}

impl NamedType for Vec<UserIdentifier> {
    fn type_name() -> &'static str {
        "Vec<UserIdentifier>"
    }
}

impl UserIdentifier {
    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = identifier.split('.').collect();

        if parts.len() != 3 {
            return Err(Error::Parse(format!(
                "Invalid UserIdentifier: {}",
                identifier
            )));
        }

        let username = parts[0].trim();
        let project = parts[1].trim();
        let portal = parts[2].trim();

        if username.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid UserIdentifier - username cannot be empty '{}'",
                identifier
            )));
        };

        if project.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid UserIdentifier - project cannot be empty '{}'",
                identifier
            )));
        };

        if portal.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid UserIdentifier - portal cannot be empty '{}'",
                identifier
            )));
        };

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
        PortalIdentifier {
            portal: self.portal.clone(),
        }
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
    fn type_name() -> &'static str {
        "ProjectMapping"
    }
}

impl NamedType for Vec<ProjectMapping> {
    fn type_name() -> &'static str {
        "Vec<ProjectMapping>"
    }
}

impl ProjectMapping {
    pub fn new(project: &ProjectIdentifier, local_group: &str) -> Result<Self, Error> {
        let local_group = local_group.trim();

        if local_group.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid ProjectMapping - local_group cannot be empty '{}'",
                local_group
            )));
        };

        if local_group.starts_with(".")
            || local_group.ends_with(".")
            || local_group.starts_with("/")
            || local_group.ends_with("/")
        {
            return Err(Error::Parse(format!(
                "Invalid ProjectMapping - local group contains invalid characters '{}'",
                local_group
            )));
        };

        Ok(Self {
            project: project.clone(),
            local_group: local_group.to_string(),
        })
    }

    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = identifier.split(':').collect();

        if parts.len() != 2 {
            return Err(Error::Parse(format!(
                "Invalid ProjectMapping: {}",
                identifier
            )));
        }

        let project = ProjectIdentifier::parse(parts[0])?;
        let local_group = parts[1].trim();

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
#[derive(Debug, Clone, PartialEq)]
pub struct UserMapping {
    user: UserIdentifier,
    local_user: String,
    local_group: String,
}

impl NamedType for UserMapping {
    fn type_name() -> &'static str {
        "UserMapping"
    }
}

impl NamedType for Vec<UserMapping> {
    fn type_name() -> &'static str {
        "Vec<UserMapping>"
    }
}

impl UserMapping {
    pub fn new(user: &UserIdentifier, local_user: &str, local_group: &str) -> Result<Self, Error> {
        let local_user = local_user.trim();
        let local_group = local_group.trim();

        if local_user.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid UserMapping - local_user cannot be empty '{}'",
                local_user
            )));
        };

        if local_user.starts_with(".")
            || local_user.ends_with(".")
            || local_user.starts_with("/")
            || local_user.ends_with("/")
        {
            return Err(Error::Parse(format!(
                "Invalid UserMapping - local_user account contains invalid characters '{}'",
                local_user
            )));
        };

        if local_group.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid UserMapping - local_group cannot be empty '{}'",
                local_group
            )));
        };

        if local_group.starts_with(".")
            || local_group.ends_with(".")
            || local_group.starts_with("/")
            || local_group.ends_with("/")
        {
            return Err(Error::Parse(format!(
                "Invalid UserMapping - local_group contains invalid characters '{}'",
                local_group
            )));
        };

        Ok(Self {
            user: user.clone(),
            local_user: local_user.to_string(),
            local_group: local_group.to_string(),
        })
    }

    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = identifier.split(':').collect();

        if parts.len() != 3 {
            return Err(Error::Parse(format!("Invalid UserMapping: {}", identifier)));
        }

        let user = UserIdentifier::parse(parts[0])?;
        let local_user = parts[1].trim();
        let local_group = parts[2].trim();

        Self::new(&user, local_user, local_group)
    }

    pub fn user(&self) -> &UserIdentifier {
        &self.user
    }

    pub fn local_user(&self) -> &str {
        &self.local_user
    }

    pub fn local_group(&self) -> &str {
        &self.local_group
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
/// Struct used to represent a single date
///
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Date {
    date: chrono::NaiveDate,
}

impl NamedType for Date {
    fn type_name() -> &'static str {
        "Date"
    }
}

impl NamedType for Vec<Date> {
    fn type_name() -> &'static str {
        "Vec<Date>"
    }
}

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
    fn type_name() -> &'static str {
        "DateRange"
    }
}

impl NamedType for Vec<DateRange> {
    fn type_name() -> &'static str {
        "Vec<DateRange>"
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

        let mut parts: Vec<&str> = date_range.split(':').collect();

        parts = match parts.len() {
            // start and end date are the same
            1 => vec![parts[0], parts[0]],
            2 => parts,
            _ => {
                return Err(Error::Parse(format!(
                    "Invalid DateRange - must contain two dates, separated by a colon '{}'",
                    date_range
                )));
            }
        };

        Ok(Self {
            start_date: Date::parse(parts[0])?,
            end_date: Date::parse(parts[1])?,
        })
    }

    pub fn start_date(&self) -> &Date {
        &self.start_date
    }

    pub fn end_date(&self) -> &Date {
        &self.end_date
    }

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

    pub fn end_time(&self) -> chrono::NaiveDateTime {
        self.end_date
            .date
            .and_hms_opt(23, 59, 59)
            .unwrap_or_else(|| {
                tracing::error!(
                    "Invalid end date '{}' - cannot convert to a end_time",
                    self.end_date
                );
                chrono::NaiveDateTime::default()
            })
    }

    pub fn days(&self) -> Vec<Date> {
        let mut days = Vec::new();

        let mut current = self.start_date.date;
        while current <= self.end_date.date {
            days.push(Date { date: current });
            current += chrono::Duration::days(1);
        }

        days
    }

    pub fn weeks(&self) -> Vec<DateRange> {
        let mut weeks = Vec::new();

        let mut current = self.start_date.date;
        while current <= self.end_date.date {
            let start_date = match current.weekday() {
                chrono::Weekday::Mon => current,
                chrono::Weekday::Tue => current - chrono::Duration::days(1),
                chrono::Weekday::Wed => current - chrono::Duration::days(2),
                chrono::Weekday::Thu => current - chrono::Duration::days(3),
                chrono::Weekday::Fri => current - chrono::Duration::days(4),
                chrono::Weekday::Sat => current - chrono::Duration::days(5),
                chrono::Weekday::Sun => current - chrono::Duration::days(6),
            };

            let end_date = start_date + chrono::Duration::days(6);

            weeks.push(DateRange {
                start_date: Date { date: start_date },
                end_date: Date { date: end_date },
            });

            current = end_date + chrono::Duration::days(1);
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

            current = end_date + chrono::Duration::days(1);
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

            current = end_date + chrono::Duration::days(1);
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
/// Enum of all of the instructions that can be sent to agents
///
#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    /// An instruction to submit a job to the portal
    Submit(Destination, Arc<Instruction>),

    /// An instruction to get all projects managed by a portal
    GetProjects(PortalIdentifier),

    /// An instruction to add a project
    AddProject(ProjectIdentifier),

    /// An instruction to remove a project
    RemoveProject(ProjectIdentifier),

    /// An instruction to get all users in a project
    GetUsers(ProjectIdentifier),

    /// An instruction to check if a user is protected from being
    /// managed by OpenPortal
    IsProtectedUser(UserIdentifier),

    /// An instruction to add a user
    AddUser(UserIdentifier),

    /// An instruction to remove a user
    RemoveUser(UserIdentifier),

    /// An instruction to look up the mapping for a user
    GetUserMapping(UserIdentifier),

    /// An instruction to look up the mapping for a project
    GetProjectMapping(ProjectIdentifier),

    /// An instruction to look up the path to the home directory
    /// for a user - note this may not yet exist
    GetHomeDir(UserIdentifier),

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

    /// Return the home directory of a local user
    /// (note this does not guarantee the directory exists)
    GetLocalHomeDir(UserMapping),

    /// Return the project directories of a local project
    /// (note this does not guarantee the directories exist)
    GetLocalProjectDirs(ProjectMapping),

    /// An instruction to update the home directory of a user
    UpdateHomeDir(UserIdentifier, String),

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
}

impl Instruction {
    pub fn parse(s: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = s.split(' ').collect();
        match parts[0] {
            "submit" => match Destination::parse(parts[1]) {
                Ok(destination) => match Instruction::parse(&parts[2..].join(" ")) {
                    Ok(instruction) => Ok(Instruction::Submit(
                        destination,
                        Arc::<Instruction>::new(instruction),
                    )),
                    Err(e) => {
                        tracing::error!(
                            "submit failed to parse the instruction for destination {}: {}. {}",
                            parts[1],
                            &parts[2..].join(" "),
                            e
                        );
                        Err(Error::Parse(format!(
                            "submit failed to parse the instruction for destination {}: {}. {}",
                            parts[1],
                            &parts[2..].join(" "),
                            e
                        )))
                    }
                },
                Err(e) => {
                    tracing::error!(
                        "submit failed to parse the destination for: {}. {}",
                        &parts[1..].join(" "),
                        e
                    );
                    Err(Error::Parse(format!(
                        "submit failed to parse the destination for: {}. {}",
                        &parts[1..].join(" "),
                        e
                    )))
                }
            },
            "get_projects" => match PortalIdentifier::parse(&parts[1..].join(" ")) {
                Ok(portal) => Ok(Instruction::GetProjects(portal)),
                Err(_) => {
                    tracing::error!("get_projects failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "get_projects failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "add_project" => match ProjectIdentifier::parse(&parts[1..].join(" ")) {
                Ok(project) => Ok(Instruction::AddProject(project)),
                Err(_) => {
                    tracing::error!("add_project failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "add_project failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "remove_project" => match ProjectIdentifier::parse(&parts[1..].join(" ")) {
                Ok(project) => Ok(Instruction::RemoveProject(project)),
                Err(_) => {
                    tracing::error!("remove_project failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "remove_project failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "add_local_project" => match ProjectMapping::parse(&parts[1..].join(" ")) {
                Ok(mapping) => Ok(Instruction::AddLocalProject(mapping)),
                Err(_) => {
                    tracing::error!(
                        "add_local_project failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    Err(Error::Parse(format!(
                        "add_local_project failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "remove_local_project" => match ProjectMapping::parse(&parts[1..].join(" ")) {
                Ok(mapping) => Ok(Instruction::RemoveLocalProject(mapping)),
                Err(_) => {
                    tracing::error!(
                        "remove_local_project failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    Err(Error::Parse(format!(
                        "remove_local_project failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "get_users" => match ProjectIdentifier::parse(&parts[1..].join(" ")) {
                Ok(project) => Ok(Instruction::GetUsers(project)),
                Err(_) => {
                    tracing::error!("get_users failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "get_users failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "add_user" => match UserIdentifier::parse(&parts[1..].join(" ")) {
                Ok(user) => Ok(Instruction::AddUser(user)),
                Err(_) => {
                    tracing::error!("add_user failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "add_user failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "remove_user" => match UserIdentifier::parse(&parts[1..].join(" ")) {
                Ok(user) => Ok(Instruction::RemoveUser(user)),
                Err(_) => {
                    tracing::error!("remove_user failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "remove_user failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "get_project_mapping" => match ProjectIdentifier::parse(&parts[1..].join(" ")) {
                Ok(project) => Ok(Instruction::GetProjectMapping(project)),
                Err(_) => {
                    tracing::error!(
                        "get_project_mapping failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    Err(Error::Parse(format!(
                        "get_project_mapping failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "get_user_mapping" => match UserIdentifier::parse(&parts[1..].join(" ")) {
                Ok(user) => Ok(Instruction::GetUserMapping(user)),
                Err(_) => {
                    tracing::error!(
                        "get_user_mapping failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    Err(Error::Parse(format!(
                        "get_user_mapping failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "add_local_user" => match UserMapping::parse(&parts[1..].join(" ")) {
                Ok(mapping) => Ok(Instruction::AddLocalUser(mapping)),
                Err(_) => {
                    tracing::error!("add_local_user failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "add_local_user failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "remove_local_user" => match UserMapping::parse(&parts[1..].join(" ")) {
                Ok(mapping) => Ok(Instruction::RemoveLocalUser(mapping)),
                Err(_) => {
                    tracing::error!(
                        "remove_local_user failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    Err(Error::Parse(format!(
                        "remove_local_user failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "update_homedir" => {
                if parts.len() < 3 {
                    tracing::error!("update_homedir failed to parse: {}", &parts[1..].join(" "));
                    return Err(Error::Parse(format!(
                        "update_homedir failed to parse: {}",
                        &parts[1..].join(" ")
                    )));
                }

                let homedir = parts[2].trim().to_string();

                if homedir.is_empty() {
                    tracing::error!("update_homedir failed to parse: {}", &parts[1..].join(" "));
                    return Err(Error::Parse(format!(
                        "update_homedir failed to parse: {}",
                        &parts[1..].join(" ")
                    )));
                }

                match UserIdentifier::parse(parts[1]) {
                    Ok(user) => Ok(Instruction::UpdateHomeDir(user, homedir)),
                    Err(_) => {
                        tracing::error!(
                            "update_homedir failed to parse: {}",
                            &parts[1..].join(" ")
                        );
                        Err(Error::Parse(format!(
                            "update_homedir failed to parse: {}",
                            &parts[1..].join(" ")
                        )))
                    }
                }
            }
            "get_local_usage_report" => {
                if parts.len() < 2 {
                    tracing::error!(
                        "get_local_usage_report failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    return Err(Error::Parse(format!(
                        "get_local_usage_report failed to parse: {}",
                        &parts[1..].join(" ")
                    )));
                }

                match ProjectMapping::parse(parts[1]) {
                    Ok(mapping) => {
                        match DateRange::parse(parts.get(2).cloned().unwrap_or("this_week")) {
                            Ok(date_range) => {
                                Ok(Instruction::GetLocalUsageReport(mapping, date_range))
                            }
                            Err(e) => {
                                tracing::error!(
                                    "get_local_usage_report failed to parse '{}': {}",
                                    &parts[1..].join(" "),
                                    e
                                );
                                Err(Error::Parse(format!(
                                    "get_local_usage_report failed to parse '{}': {}",
                                    &parts[1..].join(" "),
                                    e
                                )))
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "get_local_usage_report failed to parse '{}': {}",
                            &parts[1..].join(" "),
                            e
                        );
                        Err(Error::Parse(format!(
                            "get_local_usage_report failed to parse '{}': {}",
                            &parts[1..].join(" "),
                            e
                        )))
                    }
                }
            }
            "get_usage_report" => {
                if parts.len() < 2 {
                    tracing::error!(
                        "get_usage_report failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    return Err(Error::Parse(format!(
                        "get_usage_report failed to parse: {}",
                        &parts[1..].join(" ")
                    )));
                }

                match ProjectIdentifier::parse(parts[1]) {
                    Ok(project) => {
                        match DateRange::parse(parts.get(2).cloned().unwrap_or("this_week")) {
                            Ok(date_range) => Ok(Instruction::GetUsageReport(project, date_range)),
                            Err(e) => {
                                tracing::error!(
                                    "get_usage_report failed to parse '{}': {}",
                                    &parts[1..].join(" "),
                                    e
                                );
                                Err(Error::Parse(format!(
                                    "get_usage_report failed to parse '{}': {}",
                                    &parts[1..].join(" "),
                                    e
                                )))
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "get_usage_report failed to parse '{}': {}",
                            &parts[1..].join(" "),
                            e
                        );
                        Err(Error::Parse(format!(
                            "get_usage_report failed to parse '{}': {}",
                            &parts[1..].join(" "),
                            e
                        )))
                    }
                }
            }
            "get_usage_reports" => {
                if parts.len() < 2 {
                    tracing::error!(
                        "get_usage_reports failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    return Err(Error::Parse(format!(
                        "get_usage_reports failed to parse: {}",
                        &parts[1..].join(" ")
                    )));
                }

                match PortalIdentifier::parse(parts[1]) {
                    Ok(portal) => {
                        match DateRange::parse(parts.get(2).cloned().unwrap_or("this_week")) {
                            Ok(date_range) => Ok(Instruction::GetUsageReports(portal, date_range)),
                            Err(e) => {
                                tracing::error!(
                                    "get_usage_reports failed to parse '{}': {}",
                                    &parts[1..].join(" "),
                                    e
                                );
                                Err(Error::Parse(format!(
                                    "get_usage_reports failed to parse '{}': {}",
                                    &parts[1..].join(" "),
                                    e
                                )))
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "get_usage_reports failed to parse '{}': {}",
                            &parts[1..].join(" "),
                            e
                        );
                        Err(Error::Parse(format!(
                            "get_usage_reports failed to parse '{}': {}",
                            &parts[1..].join(" "),
                            e
                        )))
                    }
                }
            }
            "set_local_limit" => {
                if parts.len() < 3 {
                    tracing::error!("set_local_limit failed to parse: {}", &parts[1..].join(" "));
                    return Err(Error::Parse(format!(
                        "set_local_limit failed to parse: {}",
                        &parts[1..].join(" ")
                    )));
                }

                match ProjectMapping::parse(parts[1]) {
                    Ok(mapping) => match Usage::parse(parts[2]) {
                        Ok(usage) => Ok(Instruction::SetLocalLimit(mapping, usage)),
                        Err(e) => {
                            tracing::error!(
                                "set_local_limit failed to parse '{}': {}",
                                &parts[1..].join(" "),
                                e
                            );
                            Err(Error::Parse(format!(
                                "set_local_limit failed to parse '{}': {}",
                                &parts[1..].join(" "),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            "set_local_limit failed to parse '{}': {}",
                            &parts[1..].join(" "),
                            e
                        );
                        Err(Error::Parse(format!(
                            "set_local_limit failed to parse '{}': {}",
                            &parts[1..].join(" "),
                            e
                        )))
                    }
                }
            }
            "get_local_limit" => {
                if parts.len() < 2 {
                    tracing::error!("get_local_limit failed to parse: {}", &parts[1..].join(" "));
                    return Err(Error::Parse(format!(
                        "get_local_limit failed to parse: {}",
                        &parts[1..].join(" ")
                    )));
                }

                match ProjectMapping::parse(parts[1]) {
                    Ok(mapping) => Ok(Instruction::GetLocalLimit(mapping)),
                    Err(e) => {
                        tracing::error!(
                            "get_local_limit failed to parse '{}': {}",
                            &parts[1..].join(" "),
                            e
                        );
                        Err(Error::Parse(format!(
                            "get_local_limit failed to parse '{}': {}",
                            &parts[1..].join(" "),
                            e
                        )))
                    }
                }
            }
            "set_limit" => {
                if parts.len() < 3 {
                    tracing::error!("set_limit failed to parse: {}", &parts[1..].join(" "));
                    return Err(Error::Parse(format!(
                        "set_limit failed to parse: {}",
                        &parts[1..].join(" ")
                    )));
                }

                match ProjectIdentifier::parse(parts[1]) {
                    Ok(project) => match Usage::parse(parts[2]) {
                        Ok(usage) => Ok(Instruction::SetLimit(project, usage)),
                        Err(e) => {
                            tracing::error!(
                                "set_limit failed to parse '{}': {}",
                                &parts[1..].join(" "),
                                e
                            );
                            Err(Error::Parse(format!(
                                "set_limit failed to parse '{}': {}",
                                &parts[1..].join(" "),
                                e
                            )))
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            "set_limit failed to parse '{}': {}",
                            &parts[1..].join(" "),
                            e
                        );
                        Err(Error::Parse(format!(
                            "set_limit failed to parse '{}': {}",
                            &parts[1..].join(" "),
                            e
                        )))
                    }
                }
            }
            "get_limit" => {
                if parts.len() < 2 {
                    tracing::error!("get_limit failed to parse: {}", &parts[1..].join(" "));
                    return Err(Error::Parse(format!(
                        "get_limit failed to parse: {}",
                        &parts[1..].join(" ")
                    )));
                }

                match ProjectIdentifier::parse(parts[1]) {
                    Ok(project) => Ok(Instruction::GetLimit(project)),
                    Err(e) => {
                        tracing::error!(
                            "get_limit failed to parse '{}': {}",
                            &parts[1..].join(" "),
                            e
                        );
                        Err(Error::Parse(format!(
                            "get_limit failed to parse '{}': {}",
                            &parts[1..].join(" "),
                            e
                        )))
                    }
                }
            }
            "is_protected_user" => match UserIdentifier::parse(&parts[1..].join(" ")) {
                Ok(user) => Ok(Instruction::IsProtectedUser(user)),
                Err(_) => {
                    tracing::error!(
                        "is_protected_user failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    Err(Error::Parse(format!(
                        "is_protected_user failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "get_home_dir" => match UserIdentifier::parse(&parts[1..].join(" ")) {
                Ok(user) => Ok(Instruction::GetHomeDir(user)),
                Err(_) => {
                    tracing::error!("get_home_dir failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "get_home_dir failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "get_project_dirs" => match ProjectIdentifier::parse(&parts[1..].join(" ")) {
                Ok(project) => Ok(Instruction::GetProjectDirs(project)),
                Err(_) => {
                    tracing::error!(
                        "get_project_dirs failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    Err(Error::Parse(format!(
                        "get_project_dirs failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "get_local_home_dir" => match UserMapping::parse(&parts[1..].join(" ")) {
                Ok(mapping) => Ok(Instruction::GetLocalHomeDir(mapping)),
                Err(_) => {
                    tracing::error!(
                        "get_local_home_dir failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    Err(Error::Parse(format!(
                        "get_local_home_dir failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "get_local_project_dirs" => match ProjectMapping::parse(&parts[1..].join(" ")) {
                Ok(mapping) => Ok(Instruction::GetLocalProjectDirs(mapping)),
                Err(_) => {
                    tracing::error!(
                        "get_local_project_dirs failed to parse: {}",
                        &parts[1..].join(" ")
                    );
                    Err(Error::Parse(format!(
                        "get_local_project_dirs failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            _ => {
                tracing::error!("Invalid instruction: {}", s);
                Err(Error::Parse(format!("Invalid instruction: {}", s)))
            }
        }
    }
}

impl std::fmt::Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Instruction::Submit(destination, command) => {
                write!(f, "submit {} {}", destination, command)
            }
            Instruction::GetProjects(portal) => write!(f, "get_projects {}", portal),
            Instruction::AddProject(project) => write!(f, "add_project {}", project),
            Instruction::RemoveProject(project) => write!(f, "remove_project {}", project),
            Instruction::GetUsers(project) => write!(f, "get_users {}", project),
            Instruction::AddUser(user) => write!(f, "add_user {}", user),
            Instruction::RemoveUser(user) => write!(f, "remove_user {}", user),
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
            Instruction::GetLimit(project) => write!(f, "get_limit {}", project),
            Instruction::IsProtectedUser(user) => write!(f, "is_protected_user {}", user),
            Instruction::GetHomeDir(user) => write!(f, "get_home_dir {}", user),
            Instruction::GetProjectDirs(project) => write!(f, "get_project_dirs {}", project),
            Instruction::GetLocalHomeDir(mapping) => write!(f, "get_local_home_dir {}", mapping),
            Instruction::GetLocalProjectDirs(mapping) => {
                write!(f, "get_local_project_dirs {}", mapping)
            }
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
        assert_eq!(mapping.local_user(), "local_user");
        assert_eq!(mapping.local_group(), "local_group");
        assert_eq!(
            mapping.to_string(),
            "user.project.portal:local_user:local_group"
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
}
