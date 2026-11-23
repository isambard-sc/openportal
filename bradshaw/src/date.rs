// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Calendar dates and weekdays.

use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};
use std::str::FromStr;

use crate::civil::{civil_from_days, days_from_civil, days_in_month};
use crate::format::{write_year, DelayedFormat};
use crate::parse::{parse_date, ParseError};
use crate::{NaiveDateTime, NaiveTime, TimeDelta};

/// The earliest year a `NaiveDate` can hold (as in chrono).
pub(crate) const MIN_YEAR: i64 = -262_143;

/// The latest year a `NaiveDate` can hold (as in chrono).
pub(crate) const MAX_YEAR: i64 = 262_142;

pub(crate) const MIN_DAYS: i64 = days_from_civil(MIN_YEAR, 1, 1);
pub(crate) const MAX_DAYS: i64 = days_from_civil(MAX_YEAR, 12, 31);

/// A day of the week.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Weekday {
    const ALL: [Weekday; 7] = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ];

    /// 0 for Monday, up to 6 for Sunday.
    pub const fn num_days_from_monday(&self) -> u32 {
        *self as u32
    }

    /// 0 for Sunday, up to 6 for Saturday.
    pub const fn num_days_from_sunday(&self) -> u32 {
        (*self as u32 + 1) % 7
    }

    /// 1 for Monday, up to 7 for Sunday.
    pub const fn number_from_monday(&self) -> u32 {
        *self as u32 + 1
    }

    fn from_days_from_monday(n: u32) -> Weekday {
        match Self::ALL.get((n % 7) as usize) {
            Some(day) => *day,
            None => Weekday::Mon,
        }
    }

    pub fn succ(&self) -> Weekday {
        Self::from_days_from_monday(self.num_days_from_monday() + 1)
    }

    pub fn pred(&self) -> Weekday {
        Self::from_days_from_monday(self.num_days_from_monday() + 6)
    }

    pub(crate) fn short_name(&self) -> &'static str {
        match self {
            Weekday::Mon => "Mon",
            Weekday::Tue => "Tue",
            Weekday::Wed => "Wed",
            Weekday::Thu => "Thu",
            Weekday::Fri => "Fri",
            Weekday::Sat => "Sat",
            Weekday::Sun => "Sun",
        }
    }

    pub(crate) fn long_name(&self) -> &'static str {
        match self {
            Weekday::Mon => "Monday",
            Weekday::Tue => "Tuesday",
            Weekday::Wed => "Wednesday",
            Weekday::Thu => "Thursday",
            Weekday::Fri => "Friday",
            Weekday::Sat => "Saturday",
            Weekday::Sun => "Sunday",
        }
    }

    /// The weekday a short name (any case) names - `"mon"`, `"Tue"`.
    pub(crate) fn from_short_name(name: &str) -> Option<Weekday> {
        Self::ALL
            .iter()
            .find(|day| day.short_name().eq_ignore_ascii_case(name))
            .copied()
    }
}

impl fmt::Display for Weekday {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.short_name())
    }
}

/// The parts of a date: implemented by `NaiveDate`, `NaiveDateTime` and
/// `DateTime`.
pub trait Datelike: Sized {
    fn year(&self) -> i32;

    /// 1 to 12.
    fn month(&self) -> u32;

    /// 1 to 31.
    fn day(&self) -> u32;

    /// 1 to 366: the day of the year.
    fn ordinal(&self) -> u32;

    fn weekday(&self) -> Weekday;

    /// The same date in another year; `None` if it does not exist there (29
    /// February) or is out of range.
    fn with_year(&self, year: i32) -> Option<Self>;

    /// The same day in another month; `None` if it does not exist there.
    fn with_month(&self, month: u32) -> Option<Self>;

    /// Another day of the same month; `None` if it does not exist.
    fn with_day(&self, day: u32) -> Option<Self>;

    /// 0 to 11.
    fn month0(&self) -> u32 {
        self.month().saturating_sub(1)
    }

    /// 0 to 30.
    fn day0(&self) -> u32 {
        self.day().saturating_sub(1)
    }

    /// 0 to 365.
    fn ordinal0(&self) -> u32 {
        self.ordinal().saturating_sub(1)
    }

    /// Days since 0001-01-01, counting that day as 1 (as in chrono).
    fn num_days_from_ce(&self) -> i32;
}

/// A date in the proleptic Gregorian calendar, with no time or time zone.
///
/// Years from -262143 to 262142, as in chrono. Ordering is chronological.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NaiveDate {
    // days since 1970-01-01
    days: i32,
}

impl NaiveDate {
    /// -262143-01-01
    pub const MIN: NaiveDate = NaiveDate {
        days: MIN_DAYS as i32,
    };

    /// 262142-12-31
    pub const MAX: NaiveDate = NaiveDate {
        days: MAX_DAYS as i32,
    };

    /// 1970-01-01
    pub(crate) const EPOCH: NaiveDate = NaiveDate { days: 0 };

    /// The date `days` after 1970-01-01, if it is in range.
    pub(crate) const fn from_days_since_epoch(days: i64) -> Option<NaiveDate> {
        if days < MIN_DAYS || days > MAX_DAYS {
            None
        } else {
            Some(NaiveDate { days: days as i32 })
        }
    }

    /// How many days after 1970-01-01 this is.
    pub(crate) const fn days_since_epoch(&self) -> i64 {
        self.days as i64
    }

    fn ymd(&self) -> (i64, u32, u32) {
        civil_from_days(self.days as i64)
    }

    /// The date, if `month` and `day` are valid and it is in range.
    pub fn from_ymd_opt(year: i32, month: u32, day: u32) -> Option<NaiveDate> {
        let year = year as i64;

        if !(MIN_YEAR..=MAX_YEAR).contains(&year)
            || !(1..=12).contains(&month)
            || day < 1
            || day > days_in_month(year, month)
        {
            return None;
        }

        Self::from_days_since_epoch(days_from_civil(year, month, day))
    }

    /// The next day, if it is in range.
    pub fn succ_opt(&self) -> Option<NaiveDate> {
        Self::from_days_since_epoch(self.days as i64 + 1)
    }

    /// The previous day, if it is in range.
    pub fn pred_opt(&self) -> Option<NaiveDate> {
        Self::from_days_since_epoch(self.days as i64 - 1)
    }

    pub fn and_time(&self, time: NaiveTime) -> NaiveDateTime {
        NaiveDateTime::new(*self, time)
    }

    pub fn and_hms_opt(&self, hour: u32, minute: u32, second: u32) -> Option<NaiveDateTime> {
        NaiveTime::from_hms_opt(hour, minute, second).map(|time| self.and_time(time))
    }

    pub fn and_hms_milli_opt(
        &self,
        hour: u32,
        minute: u32,
        second: u32,
        milli: u32,
    ) -> Option<NaiveDateTime> {
        NaiveTime::from_hms_milli_opt(hour, minute, second, milli).map(|time| self.and_time(time))
    }

    pub fn and_hms_micro_opt(
        &self,
        hour: u32,
        minute: u32,
        second: u32,
        micro: u32,
    ) -> Option<NaiveDateTime> {
        NaiveTime::from_hms_micro_opt(hour, minute, second, micro).map(|time| self.and_time(time))
    }

    pub fn and_hms_nano_opt(
        &self,
        hour: u32,
        minute: u32,
        second: u32,
        nano: u32,
    ) -> Option<NaiveDateTime> {
        NaiveTime::from_hms_nano_opt(hour, minute, second, nano).map(|time| self.and_time(time))
    }

    /// This date moved by the whole days in `delta` (any part-day is
    /// ignored, as in chrono), if the result is in range.
    pub fn checked_add_signed(&self, delta: TimeDelta) -> Option<NaiveDate> {
        Self::from_days_since_epoch((self.days as i64).checked_add(delta.num_days())?)
    }

    /// This date moved back by the whole days in `delta`, if in range.
    pub fn checked_sub_signed(&self, delta: TimeDelta) -> Option<NaiveDate> {
        Self::from_days_since_epoch((self.days as i64).checked_sub(delta.num_days())?)
    }

    /// The whole days from `rhs` to this date.
    pub fn signed_duration_since(&self, rhs: NaiveDate) -> TimeDelta {
        TimeDelta::days(self.days as i64 - rhs.days as i64)
    }

    /// Format with a `strftime`-style pattern - see `crate::format`.
    pub fn format<'a>(&self, pattern: &'a str) -> DelayedFormat<'a> {
        DelayedFormat::new(Some(*self), None, None, pattern)
    }

    /// Parse with a `strptime`-style pattern of `%Y`, `%m` and `%d` - see
    /// `crate::parse`.
    pub fn parse_from_str(s: &str, pattern: &str) -> Result<NaiveDate, ParseError> {
        crate::parse::parse_with_pattern(s, pattern)?.date()
    }

    fn clamp(days: i64) -> NaiveDate {
        NaiveDate {
            days: days.clamp(MIN_DAYS, MAX_DAYS) as i32,
        }
    }
}

impl Default for NaiveDate {
    /// 1970-01-01
    fn default() -> Self {
        Self::EPOCH
    }
}

impl Datelike for NaiveDate {
    fn year(&self) -> i32 {
        self.ymd().0 as i32
    }

    fn month(&self) -> u32 {
        self.ymd().1
    }

    fn day(&self) -> u32 {
        self.ymd().2
    }

    fn ordinal(&self) -> u32 {
        let year = self.ymd().0;
        (self.days as i64 - days_from_civil(year, 1, 1) + 1) as u32
    }

    fn weekday(&self) -> Weekday {
        // 1970-01-01 was a Thursday
        Weekday::from_days_from_monday((self.days as i64 + 3).rem_euclid(7) as u32)
    }

    fn with_year(&self, year: i32) -> Option<NaiveDate> {
        let (_, month, day) = self.ymd();
        Self::from_ymd_opt(year, month, day)
    }

    fn with_month(&self, month: u32) -> Option<NaiveDate> {
        let (year, _, day) = self.ymd();
        Self::from_ymd_opt(year as i32, month, day)
    }

    fn with_day(&self, day: u32) -> Option<NaiveDate> {
        let (year, month, _) = self.ymd();
        Self::from_ymd_opt(year as i32, month, day)
    }

    fn num_days_from_ce(&self) -> i32 {
        (self.days as i64 - days_from_civil(1, 1, 1) + 1) as i32
    }
}

/// Adds the whole days in the delta. **Saturates** at `NaiveDate::MIN` /
/// `MAX` rather than panicking; use `checked_add_signed` to detect that.
impl Add<TimeDelta> for NaiveDate {
    type Output = NaiveDate;

    fn add(self, rhs: TimeDelta) -> NaiveDate {
        Self::clamp((self.days as i64).saturating_add(rhs.num_days()))
    }
}

/// Subtracts the whole days in the delta, saturating - see `Add`.
impl Sub<TimeDelta> for NaiveDate {
    type Output = NaiveDate;

    fn sub(self, rhs: TimeDelta) -> NaiveDate {
        Self::clamp((self.days as i64).saturating_sub(rhs.num_days()))
    }
}

impl AddAssign<TimeDelta> for NaiveDate {
    fn add_assign(&mut self, rhs: TimeDelta) {
        *self = *self + rhs;
    }
}

impl SubAssign<TimeDelta> for NaiveDate {
    fn sub_assign(&mut self, rhs: TimeDelta) {
        *self = *self - rhs;
    }
}

impl Sub<NaiveDate> for NaiveDate {
    type Output = TimeDelta;

    fn sub(self, rhs: NaiveDate) -> TimeDelta {
        self.signed_duration_since(rhs)
    }
}

/// `2026-11-23`: the year has at least four digits, and a sign outside
/// 0000 to 9999 (`+12026-01-01`, `-0001-01-01`), as in chrono.
impl fmt::Display for NaiveDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (year, month, day) = self.ymd();
        write_year(f, year)?;
        write!(f, "-{:02}-{:02}", month, day)
    }
}

impl fmt::Debug for NaiveDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// `2026-11-23` (the RFC 3339 `full-date`), or a signed year as written by
/// `Display`.
impl FromStr for NaiveDate {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<NaiveDate, ParseError> {
        parse_date(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_ymd_opt_validates() {
        assert!(NaiveDate::from_ymd_opt(2024, 2, 29).is_some());
        assert!(NaiveDate::from_ymd_opt(2026, 2, 29).is_none());
        assert!(NaiveDate::from_ymd_opt(2026, 13, 1).is_none());
        assert!(NaiveDate::from_ymd_opt(2026, 4, 31).is_none());
        assert!(NaiveDate::from_ymd_opt(2026, 1, 0).is_none());
        assert!(NaiveDate::from_ymd_opt(262_143, 1, 1).is_none());
        assert_eq!(
            NaiveDate::from_ymd_opt(262_142, 12, 31),
            Some(NaiveDate::MAX)
        );
        assert_eq!(
            NaiveDate::from_ymd_opt(-262_143, 1, 1),
            Some(NaiveDate::MIN)
        );
    }

    #[test]
    fn test_saturates_at_the_ends() {
        assert_eq!(NaiveDate::MAX + TimeDelta::days(1), NaiveDate::MAX);
        assert_eq!(NaiveDate::MIN - TimeDelta::days(1), NaiveDate::MIN);
        assert_eq!(NaiveDate::MAX.succ_opt(), None);
        assert_eq!(NaiveDate::MIN.pred_opt(), None);
        assert_eq!(NaiveDate::MAX.checked_add_signed(TimeDelta::days(1)), None);
    }

    #[test]
    fn test_weekday() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
        assert_eq!(date.weekday(), Weekday::Mon);
        assert_eq!(Weekday::Sun.succ(), Weekday::Mon);
        assert_eq!(Weekday::Mon.pred(), Weekday::Sun);
        assert_eq!(Weekday::Sun.num_days_from_sunday(), 0);
        assert_eq!(Weekday::Sun.number_from_monday(), 7);
    }
}
