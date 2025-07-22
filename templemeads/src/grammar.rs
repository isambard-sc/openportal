// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::destination::Destination;
use crate::error::Error;
use crate::usagereport::Usage;

use anyhow::Context;
use chrono::Datelike;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::{hash::Hash, sync::Arc};

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
/// The class of Project to create in a portal. This can be used
/// e.g. to specify that a project is for a particular class of
/// infrastructure (e.g. "cpu-cluster", "gpu-cluster" etc.).
/// The classes available on a portal are controlled by the
/// portal administrator, and can be arbitrarily defined. Note
/// however that once a project has been created in a class,
/// it cannot be changed.
///
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectClass {
    /// The name of the class - this must not have any spaces
    /// or special characters
    name: String,
}

impl ProjectClass {
    pub fn parse(name: &str) -> Result<Self, Error> {
        let name = name.trim();

        if name.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid ProjectClass - cannot be empty '{}'",
                name
            )));
        };

        if name.contains(' ') {
            return Err(Error::Parse(format!(
                "Invalid ProjectClass - cannot contain spaces '{}'",
                name
            )));
        };

        // name can only be alphanumeric, underscores and dashes
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(Error::Parse(format!(
                "Invalid ProjectClass - can only contain alphanumeric characters, underscores and dashes '{}'",
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

impl NamedType for ProjectClass {
    fn type_name() -> &'static str {
        "ProjectClass"
    }
}

impl std::fmt::Display for ProjectClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Serialize for ProjectClass {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProjectClass {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        ProjectClass::parse(&s).map_err(serde::de::Error::custom)
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
}

impl std::fmt::Display for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Node(cpus: {}, cores_per_cpu: {}, gpus: {}, memory: {} GB)",
            self.cpus,
            self.cores_per_cpu,
            self.gpus,
            self.memory_gb()
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
        }
    }

    pub fn construct(cpus: u32, cores_per_cpu: u32, gpus: u32, memory_mb: u32) -> Self {
        Self {
            cpus,
            cores_per_cpu,
            gpus,
            memory_mb,
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
}

impl NamedType for Node {
    fn type_name() -> &'static str {
        "Node"
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
        }

        // Add more canonicalizations as needed
        canonical
    }

    pub fn from_size_and_units(size: f64, units: &str) -> Result<Self, Error> {
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

        if parts.is_empty() || parts.len() < 2 {
            return Err(Error::Parse(format!(
                "Invalid Allocation - must contain a size and units '{}'",
                allocation
            )));
        }

        let size = parts[0].parse::<f64>().map_err(|_| {
            Error::Parse(format!(
                "Invalid Allocation - size must be a number '{}'",
                parts[0]
            ))
        })?;

        if size < 0.0 {
            return Err(Error::Parse(format!(
                "Invalid Allocation - size cannot be negative '{}'",
                size
            )));
        }

        let units = if parts.len() > 1 {
            let u = parts[1..].join(" ");
            let u = u.trim();

            if u.is_empty() {
                return Err(Error::Parse(format!(
                    "Invalid Allocation - units cannot be empty '{}'",
                    allocation
                )));
            }

            u.to_string()
        } else {
            return Err(Error::Parse(format!(
                "Invalid Allocation - must contain a size and units '{}'",
                allocation
            )));
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
}

impl NamedType for Allocation {
    fn type_name() -> &'static str {
        "Allocation"
    }
}

impl NamedType for Vec<Allocation> {
    fn type_name() -> &'static str {
        "Vec<Allocation>"
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

///
/// Details about a project that exists in a portal.
/// This holds all data as "option" as not all details
/// will be set by all portals. Also, using "option" allows
/// this struct to be used in "update" requests, as only
/// the fields that are set will be updated.
///
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectDetails {
    /// The name of the project
    name: Option<String>,

    /// The class of the project
    class: Option<ProjectClass>,

    /// The description of the project
    description: Option<String>,

    /// The email address(es) of the members of the project,
    /// (keys) and their roles (values).
    members: Option<HashMap<String, String>>,

    /// Proposed start date of the project
    start_date: Option<Date>,

    /// Proposed end date of the project
    end_date: Option<Date>,

    /// The allocation of resource for this project
    allocation: Option<Allocation>,
}

impl NamedType for ProjectDetails {
    fn type_name() -> &'static str {
        "ProjectDetails"
    }
}

impl ProjectDetails {
    pub fn new() -> Self {
        Self {
            name: None,
            class: None,
            description: None,
            members: None,
            start_date: None,
            end_date: None,
            allocation: None,
        }
    }

    pub fn parse(json: &str) -> Result<Self, Error> {
        ProjectDetails::from_json(json)
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

    pub fn class(&self) -> Option<ProjectClass> {
        self.class.clone()
    }

    pub fn set_class(&mut self, class: ProjectClass) {
        self.class = Some(class);
    }

    pub fn clear_class(&mut self) {
        self.class = None;
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

    pub fn members(&self) -> Option<HashMap<String, String>> {
        self.members.clone()
    }

    pub fn add_member(&mut self, email: &str, role: &str) {
        let email = email.trim();
        let role = role.trim();

        if email.is_empty() || role.is_empty() {
            tracing::warn!(
                "Invalid ProjectDetails - email or role cannot be empty: email='{}', role='{}'",
                email,
                role
            );
            return;
        };

        let members = self.members.get_or_insert_with(HashMap::new);
        members.insert(email.to_string(), role.to_string());
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

    pub fn set_members(&mut self, members: HashMap<String, String>) {
        if members.is_empty() {
            self.members = None;
        } else {
            self.members = Some(members);
        }
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
}

impl std::fmt::Display for ProjectDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_json())
    }
}

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
            "create_project" => match ProjectIdentifier::parse(parts[1]) {
                Ok(project) => match ProjectDetails::parse(&parts[2..].join(" ")) {
                    Ok(details) => Ok(Instruction::CreateProject(project, details)),
                    Err(_) => {
                        tracing::error!(
                            "create_project failed to parse: {}",
                            &parts[3..].join(" ")
                        );
                        Err(Error::Parse(format!(
                            "create_project failed to parse: {}",
                            &parts[3..].join(" ")
                        )))
                    }
                },
                Err(_) => {
                    tracing::error!("create_project failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "create_project failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "update_project" => match ProjectIdentifier::parse(parts[1]) {
                Ok(project) => match ProjectDetails::parse(&parts[2..].join(" ")) {
                    Ok(details) => Ok(Instruction::UpdateProject(project, details)),
                    Err(_) => {
                        tracing::error!(
                            "update_project failed to parse: {}",
                            &parts[2..].join(" ")
                        );
                        Err(Error::Parse(format!(
                            "update_project failed to parse: {}",
                            &parts[2..].join(" ")
                        )))
                    }
                },
                Err(_) => {
                    tracing::error!("update_project failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "update_project failed to parse: {}",
                        &parts[1..].join(" ")
                    )))
                }
            },
            "get_project" => match ProjectIdentifier::parse(&parts[1..].join(" ")) {
                Ok(project) => Ok(Instruction::GetProject(project)),
                Err(_) => {
                    tracing::error!("get_project failed to parse: {}", &parts[1..].join(" "));
                    Err(Error::Parse(format!(
                        "get_project failed to parse: {}",
                        &parts[1..].join(" ")
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
                    Ok(project) => match Usage::parse(&parts[2..].join(" ")) {
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

    pub fn command(&self) -> String {
        match self {
            Instruction::Submit(_, _) => "submit".to_string(),
            Instruction::CreateProject(_, _) => "create_project".to_string(),
            Instruction::UpdateProject(_, _) => "update_project".to_string(),
            Instruction::GetProject(_) => "get_project".to_string(),
            Instruction::GetProjects(_) => "get_projects".to_string(),
            Instruction::AddProject(_) => "add_project".to_string(),
            Instruction::RemoveProject(_) => "remove_project".to_string(),
            Instruction::GetUsers(_) => "get_users".to_string(),
            Instruction::AddUser(_) => "add_user".to_string(),
            Instruction::RemoveUser(_) => "remove_user".to_string(),
            Instruction::GetUserMapping(_) => "get_user_mapping".to_string(),
            Instruction::GetProjectMapping(_) => "get_project_mapping".to_string(),
            Instruction::GetHomeDir(_) => "get_home_dir".to_string(),
            Instruction::GetProjectDirs(_) => "get_project_dirs".to_string(),
            Instruction::AddLocalUser(_) => "add_local_user".to_string(),
            Instruction::RemoveLocalUser(_) => "remove_local_user".to_string(),
            Instruction::AddLocalProject(_) => "add_local_project".to_string(),
            Instruction::RemoveLocalProject(_) => "remove_local_project".to_string(),
            Instruction::GetLocalUsageReport(_, _) => "get_local_usage_report".to_string(),
            Instruction::GetLocalLimit(_) => "get_local_limit".to_string(),
            Instruction::SetLocalLimit(_, _) => "set_local_limit".to_string(),
            Instruction::GetLocalHomeDir(_) => "get_local_home_dir".to_string(),
            Instruction::GetLocalProjectDirs(_) => "get_local_project_dirs".to_string(),
            Instruction::UpdateHomeDir(_, _) => "update_homedir".to_string(),
            Instruction::GetUsageReport(_, _) => "get_usage_report".to_string(),
            Instruction::GetUsageReports(_, _) => "get_usage_reports".to_string(),
            Instruction::SetLimit(_, _) => "set_limit".to_string(),
            Instruction::GetLimit(_) => "get_limit".to_string(),
            Instruction::IsProtectedUser(_) => "is_protected_user".to_string(),
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
            Instruction::AddProject(project) => vec![project.to_string()],
            Instruction::RemoveProject(project) => vec![project.to_string()],
            Instruction::GetUsers(project) => vec![project.to_string()],
            Instruction::AddUser(user) => vec![user.to_string()],
            Instruction::RemoveUser(user) => vec![user.to_string()],
            Instruction::GetUserMapping(user) => vec![user.to_string()],
            Instruction::GetProjectMapping(project) => vec![project.to_string()],
            Instruction::GetHomeDir(user) => vec![user.to_string()],
            Instruction::GetProjectDirs(project) => vec![project.to_string()],
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
            Instruction::GetLocalHomeDir(mapping) => vec![mapping.to_string()],
            Instruction::GetLocalProjectDirs(mapping) => vec![mapping.to_string()],
            Instruction::UpdateHomeDir(user, homedir) => {
                vec![user.to_string(), homedir.clone()]
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
            Instruction::IsProtectedUser(user) => vec![user.to_string()],
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
