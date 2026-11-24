// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! A date and time of day together, with no time zone.

use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};
use std::str::FromStr;

use crate::format::DelayedFormat;
use crate::parse::{parse_naive_datetime, ParseError};
use crate::{DateTime, Datelike, NaiveDate, NaiveTime, TimeDelta, Timelike, Utc, Weekday};

const NANOS_PER_SEC: i128 = 1_000_000_000;

/// A date and a time of day, with no time zone. In OpenPortal it is always
/// read as UTC - `and_utc()` makes that explicit.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct NaiveDateTime {
    date: NaiveDate,
    time: NaiveTime,
}

impl NaiveDateTime {
    /// The start of `NaiveDate::MIN`.
    pub const MIN: NaiveDateTime = NaiveDateTime {
        date: NaiveDate::MIN,
        time: NaiveTime::MIN,
    };

    /// The last nanosecond of `NaiveDate::MAX`.
    pub const MAX: NaiveDateTime = NaiveDateTime {
        date: NaiveDate::MAX,
        time: match NaiveTime::from_hms_nano_opt(23, 59, 59, 999_999_999) {
            Some(time) => time,
            None => NaiveTime::MIN,
        },
    };

    pub const fn new(date: NaiveDate, time: NaiveTime) -> NaiveDateTime {
        NaiveDateTime { date, time }
    }

    pub const fn date(&self) -> NaiveDate {
        self.date
    }

    pub const fn time(&self) -> NaiveTime {
        self.time
    }

    /// This date and time, read as UTC.
    pub const fn and_utc(&self) -> DateTime<Utc> {
        DateTime::from_naive_utc(*self)
    }

    /// Nanoseconds since 1970-01-01T00:00:00, exactly.
    pub(crate) fn total_nanos(&self) -> i128 {
        let secs = self.date.days_since_epoch() as i128 * 86_400
            + self.time.seconds_from_midnight() as i128;
        secs * NANOS_PER_SEC + self.time.nanosecond() as i128
    }

    /// The date and time `total` nanoseconds after 1970-01-01T00:00:00, if
    /// in range.
    pub(crate) fn from_total_nanos(total: i128) -> Option<NaiveDateTime> {
        let secs = total.div_euclid(NANOS_PER_SEC);
        let nanos = total.rem_euclid(NANOS_PER_SEC) as u32;
        let days = secs.div_euclid(86_400);
        let seconds_of_day = secs.rem_euclid(86_400) as u32;

        let days = i64::try_from(days).ok()?;

        Some(NaiveDateTime {
            date: NaiveDate::from_days_since_epoch(days)?,
            time: NaiveTime::from_num_seconds_from_midnight_opt(seconds_of_day, nanos)?,
        })
    }

    fn saturating_from_total_nanos(total: i128) -> NaiveDateTime {
        match Self::from_total_nanos(total) {
            Some(datetime) => datetime,
            None if total < 0 => Self::MIN,
            None => Self::MAX,
        }
    }

    pub fn checked_add_signed(&self, delta: TimeDelta) -> Option<NaiveDateTime> {
        Self::from_total_nanos(self.total_nanos() + delta.total_nanos())
    }

    pub fn checked_sub_signed(&self, delta: TimeDelta) -> Option<NaiveDateTime> {
        Self::from_total_nanos(self.total_nanos() - delta.total_nanos())
    }

    pub fn signed_duration_since(&self, rhs: NaiveDateTime) -> TimeDelta {
        TimeDelta::saturating(self.total_nanos() - rhs.total_nanos())
    }

    /// Format with a `strftime`-style pattern - see `crate::format`.
    pub fn format<'a>(&self, pattern: &'a str) -> DelayedFormat<'a> {
        DelayedFormat::new(Some(self.date), Some(self.time), None, pattern)
    }

    /// Parse with a `strptime`-style pattern - see `crate::parse`. Fields the
    /// pattern does not give (e.g. minutes for `"%Y-%m-%d %H"`) are zero.
    pub fn parse_from_str(s: &str, pattern: &str) -> Result<NaiveDateTime, ParseError> {
        crate::parse::parse_with_pattern(s, pattern)?.datetime()
    }
}

impl Datelike for NaiveDateTime {
    fn year(&self) -> i32 {
        self.date.year()
    }

    fn month(&self) -> u32 {
        self.date.month()
    }

    fn day(&self) -> u32 {
        self.date.day()
    }

    fn ordinal(&self) -> u32 {
        self.date.ordinal()
    }

    fn weekday(&self) -> Weekday {
        self.date.weekday()
    }

    fn with_year(&self, year: i32) -> Option<Self> {
        Some(Self::new(self.date.with_year(year)?, self.time))
    }

    fn with_month(&self, month: u32) -> Option<Self> {
        Some(Self::new(self.date.with_month(month)?, self.time))
    }

    fn with_day(&self, day: u32) -> Option<Self> {
        Some(Self::new(self.date.with_day(day)?, self.time))
    }

    fn num_days_from_ce(&self) -> i32 {
        self.date.num_days_from_ce()
    }
}

impl Timelike for NaiveDateTime {
    fn hour(&self) -> u32 {
        self.time.hour()
    }

    fn minute(&self) -> u32 {
        self.time.minute()
    }

    fn second(&self) -> u32 {
        self.time.second()
    }

    fn nanosecond(&self) -> u32 {
        self.time.nanosecond()
    }

    fn with_hour(&self, hour: u32) -> Option<Self> {
        Some(Self::new(self.date, self.time.with_hour(hour)?))
    }

    fn with_minute(&self, minute: u32) -> Option<Self> {
        Some(Self::new(self.date, self.time.with_minute(minute)?))
    }

    fn with_second(&self, second: u32) -> Option<Self> {
        Some(Self::new(self.date, self.time.with_second(second)?))
    }

    fn with_nanosecond(&self, nanosecond: u32) -> Option<Self> {
        Some(Self::new(self.date, self.time.with_nanosecond(nanosecond)?))
    }
}

/// **Saturates** at `NaiveDateTime::MIN` / `MAX` rather than panicking; use
/// `checked_add_signed` to detect that.
impl Add<TimeDelta> for NaiveDateTime {
    type Output = NaiveDateTime;

    fn add(self, rhs: TimeDelta) -> NaiveDateTime {
        Self::saturating_from_total_nanos(self.total_nanos() + rhs.total_nanos())
    }
}

/// Saturates - see `Add`.
impl Sub<TimeDelta> for NaiveDateTime {
    type Output = NaiveDateTime;

    fn sub(self, rhs: TimeDelta) -> NaiveDateTime {
        Self::saturating_from_total_nanos(self.total_nanos() - rhs.total_nanos())
    }
}

impl AddAssign<TimeDelta> for NaiveDateTime {
    fn add_assign(&mut self, rhs: TimeDelta) {
        *self = *self + rhs;
    }
}

impl SubAssign<TimeDelta> for NaiveDateTime {
    fn sub_assign(&mut self, rhs: TimeDelta) {
        *self = *self - rhs;
    }
}

impl Sub<NaiveDateTime> for NaiveDateTime {
    type Output = TimeDelta;

    fn sub(self, rhs: NaiveDateTime) -> TimeDelta {
        self.signed_duration_since(rhs)
    }
}

impl From<NaiveDate> for NaiveDateTime {
    /// Midnight at the start of the date.
    fn from(date: NaiveDate) -> Self {
        Self::new(date, NaiveTime::MIN)
    }
}

/// `2026-11-23 10:00:00`, with a fraction of 3, 6 or 9 digits when there is
/// one (as in chrono).
impl fmt::Display for NaiveDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.date, self.time)
    }
}

/// `2026-11-23T10:00:00` - also the serialised form (as in chrono).
impl fmt::Debug for NaiveDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}T{}", self.date, self.time)
    }
}

/// `2026-11-23T10:00:00`, with an optional fraction and `T`, `t` or a space
/// between the date and the time (RFC 3339 without the offset).
impl FromStr for NaiveDateTime {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<NaiveDateTime, ParseError> {
        parse_naive_datetime(s)
    }
}
