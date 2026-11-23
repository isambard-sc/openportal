// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! An instant in UTC.

use std::fmt;
use std::marker::PhantomData;
use std::ops::{Add, AddAssign, Sub, SubAssign};
use std::str::FromStr;

use crate::format::DelayedFormat;
use crate::parse::{parse_rfc2822, parse_rfc3339, ParseError};
use crate::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Timelike, Weekday};

const NANOS_PER_SEC: i128 = 1_000_000_000;

/// Coordinated Universal Time: the only time zone OpenPortal uses.
///
/// `DateTime<Utc>` is written as in chrono, so that code reads the same; there
/// are no other zones, and so no `Local` - an agent's idea of "now" never
/// depends on the time zone of the host it runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Utc;

impl Utc {
    /// The current time, from the system clock.
    pub fn now() -> DateTime<Utc> {
        DateTime::from(std::time::SystemTime::now())
    }
}

impl fmt::Display for Utc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UTC")
    }
}

/// An instant, to the nanosecond, in UTC. The type parameter is only ever
/// `Utc`; it is kept so that code is written as it is with chrono.
///
/// Ordering is chronological. Serialised as RFC 3339 - see `Debug`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DateTime<Tz> {
    utc: NaiveDateTime,
    zone: PhantomData<Tz>,
}

impl DateTime<Utc> {
    /// The earliest instant: the start of `NaiveDate::MIN`.
    pub const MIN_UTC: DateTime<Utc> = DateTime::from_naive_utc(NaiveDateTime::MIN);

    /// The latest instant: the last nanosecond of `NaiveDate::MAX`.
    pub const MAX_UTC: DateTime<Utc> = DateTime::from_naive_utc(NaiveDateTime::MAX);

    /// 1970-01-01T00:00:00Z
    pub const UNIX_EPOCH: DateTime<Utc> =
        DateTime::from_naive_utc(NaiveDateTime::new(NaiveDate::EPOCH, NaiveTime::MIN));

    pub(crate) const fn from_naive_utc(utc: NaiveDateTime) -> DateTime<Utc> {
        DateTime {
            utc,
            zone: PhantomData,
        }
    }

    /// The instant `secs` seconds and `nanos` nanoseconds after
    /// 1970-01-01T00:00:00Z, if `nanos` is under a second and it is in range.
    pub fn from_timestamp(secs: i64, nanos: u32) -> Option<DateTime<Utc>> {
        if nanos >= 1_000_000_000 {
            return None;
        }

        NaiveDateTime::from_total_nanos(secs as i128 * NANOS_PER_SEC + nanos as i128)
            .map(Self::from_naive_utc)
    }

    pub fn from_timestamp_millis(millis: i64) -> Option<DateTime<Utc>> {
        NaiveDateTime::from_total_nanos(millis as i128 * 1_000_000).map(Self::from_naive_utc)
    }

    pub fn from_timestamp_micros(micros: i64) -> Option<DateTime<Utc>> {
        NaiveDateTime::from_total_nanos(micros as i128 * 1_000).map(Self::from_naive_utc)
    }

    /// Seconds since 1970-01-01T00:00:00Z (rounded down).
    pub fn timestamp(&self) -> i64 {
        self.utc.total_nanos().div_euclid(NANOS_PER_SEC) as i64
    }

    /// Milliseconds since 1970-01-01T00:00:00Z (rounded down).
    pub fn timestamp_millis(&self) -> i64 {
        self.utc.total_nanos().div_euclid(1_000_000) as i64
    }

    /// Microseconds since 1970-01-01T00:00:00Z (rounded down).
    pub fn timestamp_micros(&self) -> i64 {
        self.utc.total_nanos().div_euclid(1_000) as i64
    }

    /// Nanoseconds since 1970-01-01T00:00:00Z, if they fit an `i64` (years
    /// 1677 to 2262).
    pub fn timestamp_nanos_opt(&self) -> Option<i64> {
        i64::try_from(self.utc.total_nanos()).ok()
    }

    pub fn timestamp_subsec_nanos(&self) -> u32 {
        self.utc.nanosecond()
    }

    pub fn timestamp_subsec_millis(&self) -> u32 {
        self.utc.nanosecond() / 1_000_000
    }

    pub fn timestamp_subsec_micros(&self) -> u32 {
        self.utc.nanosecond() / 1_000
    }

    /// The date and time in UTC.
    pub const fn naive_utc(&self) -> NaiveDateTime {
        self.utc
    }

    /// The date in UTC.
    pub const fn date_naive(&self) -> NaiveDate {
        self.utc.date()
    }

    /// The time of day in UTC.
    pub const fn time(&self) -> NaiveTime {
        self.utc.time()
    }

    /// The same instant. There is only UTC; this is here so that code written
    /// for chrono reads the same.
    pub const fn with_timezone(&self, _zone: &Utc) -> DateTime<Utc> {
        *self
    }

    pub fn checked_add_signed(&self, delta: TimeDelta) -> Option<DateTime<Utc>> {
        self.utc.checked_add_signed(delta).map(Self::from_naive_utc)
    }

    pub fn checked_sub_signed(&self, delta: TimeDelta) -> Option<DateTime<Utc>> {
        self.utc.checked_sub_signed(delta).map(Self::from_naive_utc)
    }

    /// The duration from `rhs` to this instant. Takes the other instant by
    /// value or by reference.
    pub fn signed_duration_since(&self, rhs: impl std::borrow::Borrow<DateTime<Utc>>) -> TimeDelta {
        self.utc.signed_duration_since(rhs.borrow().utc)
    }

    /// Format with a `strftime`-style pattern - see `crate::format`.
    pub fn format<'a>(&self, pattern: &'a str) -> DelayedFormat<'a> {
        DelayedFormat::new(
            Some(self.utc.date()),
            Some(self.utc.time()),
            Some(self.timestamp()),
            pattern,
        )
    }

    /// RFC 3339 with an explicit offset: `2026-11-23T10:00:00+00:00` (as in
    /// chrono). `Debug` and serialisation use `Z` instead.
    pub fn to_rfc3339(&self) -> String {
        format!("{}T{}+00:00", self.utc.date(), self.utc.time())
    }

    /// RFC 1123 / 2822, as HTTP uses it: `Mon, 23 Nov 2026 10:00:00 GMT`.
    pub fn to_rfc2822(&self) -> String {
        self.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
    }

    /// Parse an RFC 3339 timestamp, with any offset - see `crate::parse`.
    pub fn parse_from_rfc3339(s: &str) -> Result<DateTime<Utc>, ParseError> {
        parse_rfc3339(s)
    }

    /// Parse an RFC 2822 date - as sent in an HTTP `Date` header - with any
    /// zone - see `crate::parse`.
    pub fn parse_from_rfc2822(s: &str) -> Result<DateTime<Utc>, ParseError> {
        parse_rfc2822(s)
    }
}

impl Default for DateTime<Utc> {
    /// 1970-01-01T00:00:00Z
    fn default() -> Self {
        Self::UNIX_EPOCH
    }
}

impl Datelike for DateTime<Utc> {
    fn year(&self) -> i32 {
        self.utc.year()
    }

    fn month(&self) -> u32 {
        self.utc.month()
    }

    fn day(&self) -> u32 {
        self.utc.day()
    }

    fn ordinal(&self) -> u32 {
        self.utc.ordinal()
    }

    fn weekday(&self) -> Weekday {
        self.utc.weekday()
    }

    fn with_year(&self, year: i32) -> Option<Self> {
        self.utc.with_year(year).map(Self::from_naive_utc)
    }

    fn with_month(&self, month: u32) -> Option<Self> {
        self.utc.with_month(month).map(Self::from_naive_utc)
    }

    fn with_day(&self, day: u32) -> Option<Self> {
        self.utc.with_day(day).map(Self::from_naive_utc)
    }

    fn num_days_from_ce(&self) -> i32 {
        self.utc.num_days_from_ce()
    }
}

impl Timelike for DateTime<Utc> {
    fn hour(&self) -> u32 {
        self.utc.hour()
    }

    fn minute(&self) -> u32 {
        self.utc.minute()
    }

    fn second(&self) -> u32 {
        self.utc.second()
    }

    fn nanosecond(&self) -> u32 {
        self.utc.nanosecond()
    }

    fn with_hour(&self, hour: u32) -> Option<Self> {
        self.utc.with_hour(hour).map(Self::from_naive_utc)
    }

    fn with_minute(&self, minute: u32) -> Option<Self> {
        self.utc.with_minute(minute).map(Self::from_naive_utc)
    }

    fn with_second(&self, second: u32) -> Option<Self> {
        self.utc.with_second(second).map(Self::from_naive_utc)
    }

    fn with_nanosecond(&self, nanosecond: u32) -> Option<Self> {
        self.utc
            .with_nanosecond(nanosecond)
            .map(Self::from_naive_utc)
    }
}

/// **Saturates** at `MIN_UTC` / `MAX_UTC` rather than panicking - an expiry
/// can be computed from a duration a peer sent. Use `checked_add_signed` to
/// detect it.
impl Add<TimeDelta> for DateTime<Utc> {
    type Output = DateTime<Utc>;

    fn add(self, rhs: TimeDelta) -> DateTime<Utc> {
        Self::from_naive_utc(self.utc + rhs)
    }
}

/// Saturates - see `Add`.
impl Sub<TimeDelta> for DateTime<Utc> {
    type Output = DateTime<Utc>;

    fn sub(self, rhs: TimeDelta) -> DateTime<Utc> {
        Self::from_naive_utc(self.utc - rhs)
    }
}

impl AddAssign<TimeDelta> for DateTime<Utc> {
    fn add_assign(&mut self, rhs: TimeDelta) {
        *self = *self + rhs;
    }
}

impl SubAssign<TimeDelta> for DateTime<Utc> {
    fn sub_assign(&mut self, rhs: TimeDelta) {
        *self = *self - rhs;
    }
}

impl Sub<DateTime<Utc>> for DateTime<Utc> {
    type Output = TimeDelta;

    fn sub(self, rhs: DateTime<Utc>) -> TimeDelta {
        self.signed_duration_since(rhs)
    }
}

/// `2026-11-23 10:00:00 UTC`, with a fraction of 3, 6 or 9 digits when there
/// is one (as in chrono).
impl fmt::Display for DateTime<Utc> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} UTC", self.utc)
    }
}

/// RFC 3339 with `Z`: `2026-11-23T10:00:00Z`, with a fraction of 3, 6 or 9
/// digits when there is one. This is also the serialised form (as in chrono).
impl fmt::Debug for DateTime<Utc> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}Z", self.utc)
    }
}

/// RFC 3339, with any offset - see `crate::parse`.
impl FromStr for DateTime<Utc> {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<DateTime<Utc>, ParseError> {
        parse_rfc3339(s)
    }
}

impl From<std::time::SystemTime> for DateTime<Utc> {
    /// Saturates at `MIN_UTC` / `MAX_UTC`.
    fn from(time: std::time::SystemTime) -> Self {
        let total = match time.duration_since(std::time::UNIX_EPOCH) {
            Ok(after) => after.as_nanos() as i128,
            Err(before) => -(before.duration().as_nanos() as i128),
        };

        match NaiveDateTime::from_total_nanos(total) {
            Some(utc) => Self::from_naive_utc(utc),
            None if total < 0 => Self::MIN_UTC,
            None => Self::MAX_UTC,
        }
    }
}
