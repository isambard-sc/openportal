// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! A signed duration.

use std::fmt;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

const NANOS_PER_SEC: i32 = 1_000_000_000;
const MAX_SECS: i64 = i64::MAX / 1000;
const MAX_NANOS: i32 = (i64::MAX % 1000) as i32 * 1_000_000;

/// A signed duration, to the nanosecond, of up to `i64::MAX` milliseconds
/// either way (about 292 million years).
///
/// Unlike a `std::time::Duration` it can be negative, so it is what
/// subtracting one time from another gives.
///
/// **Arithmetic saturates.** A constructor or operator whose result would be
/// out of range gives `TimeDelta::MIN` or `TimeDelta::MAX` rather than
/// panicking - release builds abort on a panic, and a duration can come from a
/// peer. Use the `try_` constructors and `checked_` methods to detect it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TimeDelta {
    // Whole seconds, and the nanoseconds (always 0..1e9) after them - so -1.5s
    // is (-2, 500_000_000). Ordering by the fields orders by the duration.
    secs: i64,
    nanos: i32,
}

/// The name chrono also uses for `TimeDelta`.
pub type Duration = TimeDelta;

/// A `TimeDelta` could not be converted to or from a `std::time::Duration`:
/// it was negative, or too long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfRangeError;

impl fmt::Display for OutOfRangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("duration out of range")
    }
}

impl std::error::Error for OutOfRangeError {}

impl TimeDelta {
    /// The longest negative duration: `-i64::MAX` milliseconds.
    pub const MIN: TimeDelta = TimeDelta {
        secs: -MAX_SECS - 1,
        nanos: NANOS_PER_SEC - MAX_NANOS,
    };

    /// The longest duration: `i64::MAX` milliseconds.
    pub const MAX: TimeDelta = TimeDelta {
        secs: MAX_SECS,
        nanos: MAX_NANOS,
    };

    /// A duration from whole seconds and nanoseconds (which may be negative,
    /// or more than a second). `None` if it is out of range.
    pub const fn new(secs: i64, nanos: i32) -> Option<TimeDelta> {
        let total = secs as i128 * NANOS_PER_SEC as i128 + nanos as i128;
        Self::from_total_nanos(total)
    }

    const fn from_total_nanos(total: i128) -> Option<TimeDelta> {
        let secs = total.div_euclid(NANOS_PER_SEC as i128);
        let nanos = total.rem_euclid(NANOS_PER_SEC as i128) as i32;

        let delta = TimeDelta {
            secs: secs as i64,
            nanos,
        };

        // compare as nanoseconds, which is exact
        if total < Self::MIN.total_nanos() || total > Self::MAX.total_nanos() {
            None
        } else {
            Some(delta)
        }
    }

    /// A duration of `total` nanoseconds, saturating at `MIN` / `MAX`.
    pub(crate) const fn saturating(total: i128) -> TimeDelta {
        match Self::from_total_nanos(total) {
            Some(delta) => delta,
            None if total < 0 => Self::MIN,
            None => Self::MAX,
        }
    }

    /// The whole duration in nanoseconds, which an `i128` holds exactly.
    pub(crate) const fn total_nanos(&self) -> i128 {
        self.secs as i128 * NANOS_PER_SEC as i128 + self.nanos as i128
    }

    pub const fn zero() -> TimeDelta {
        TimeDelta { secs: 0, nanos: 0 }
    }

    pub const fn is_zero(&self) -> bool {
        self.secs == 0 && self.nanos == 0
    }

    pub const fn weeks(weeks: i64) -> TimeDelta {
        Self::saturating(weeks as i128 * 7 * 86_400 * NANOS_PER_SEC as i128)
    }

    pub const fn days(days: i64) -> TimeDelta {
        Self::saturating(days as i128 * 86_400 * NANOS_PER_SEC as i128)
    }

    pub const fn hours(hours: i64) -> TimeDelta {
        Self::saturating(hours as i128 * 3_600 * NANOS_PER_SEC as i128)
    }

    pub const fn minutes(minutes: i64) -> TimeDelta {
        Self::saturating(minutes as i128 * 60 * NANOS_PER_SEC as i128)
    }

    pub const fn seconds(seconds: i64) -> TimeDelta {
        Self::saturating(seconds as i128 * NANOS_PER_SEC as i128)
    }

    pub const fn milliseconds(milliseconds: i64) -> TimeDelta {
        Self::saturating(milliseconds as i128 * 1_000_000)
    }

    pub const fn microseconds(microseconds: i64) -> TimeDelta {
        Self::saturating(microseconds as i128 * 1_000)
    }

    pub const fn nanoseconds(nanoseconds: i64) -> TimeDelta {
        Self::saturating(nanoseconds as i128)
    }

    pub const fn try_days(days: i64) -> Option<TimeDelta> {
        Self::from_total_nanos(days as i128 * 86_400 * NANOS_PER_SEC as i128)
    }

    pub const fn try_hours(hours: i64) -> Option<TimeDelta> {
        Self::from_total_nanos(hours as i128 * 3_600 * NANOS_PER_SEC as i128)
    }

    pub const fn try_minutes(minutes: i64) -> Option<TimeDelta> {
        Self::from_total_nanos(minutes as i128 * 60 * NANOS_PER_SEC as i128)
    }

    pub const fn try_seconds(seconds: i64) -> Option<TimeDelta> {
        Self::from_total_nanos(seconds as i128 * NANOS_PER_SEC as i128)
    }

    pub const fn try_milliseconds(milliseconds: i64) -> Option<TimeDelta> {
        Self::from_total_nanos(milliseconds as i128 * 1_000_000)
    }

    /// Whole weeks, truncated towards zero.
    pub const fn num_weeks(&self) -> i64 {
        self.num_days() / 7
    }

    /// Whole days, truncated towards zero.
    pub const fn num_days(&self) -> i64 {
        self.num_seconds() / 86_400
    }

    /// Whole hours, truncated towards zero.
    pub const fn num_hours(&self) -> i64 {
        self.num_seconds() / 3_600
    }

    /// Whole minutes, truncated towards zero.
    pub const fn num_minutes(&self) -> i64 {
        self.num_seconds() / 60
    }

    /// Whole seconds, truncated towards zero.
    pub const fn num_seconds(&self) -> i64 {
        if self.secs < 0 && self.nanos > 0 {
            self.secs + 1
        } else {
            self.secs
        }
    }

    /// The nanoseconds after `num_seconds()`, with the duration's sign.
    pub const fn subsec_nanos(&self) -> i32 {
        if self.secs < 0 && self.nanos > 0 {
            self.nanos - NANOS_PER_SEC
        } else {
            self.nanos
        }
    }

    /// Whole milliseconds, truncated towards zero. Always in range.
    pub const fn num_milliseconds(&self) -> i64 {
        (self.total_nanos() / 1_000_000) as i64
    }

    /// Whole microseconds, truncated towards zero, if they fit an `i64`.
    pub const fn num_microseconds(&self) -> Option<i64> {
        let micros = self.total_nanos() / 1_000;
        if micros > i64::MAX as i128 || micros < i64::MIN as i128 {
            None
        } else {
            Some(micros as i64)
        }
    }

    /// Nanoseconds, if they fit an `i64` (about ±292 years).
    pub const fn num_nanoseconds(&self) -> Option<i64> {
        let nanos = self.total_nanos();
        if nanos > i64::MAX as i128 || nanos < i64::MIN as i128 {
            None
        } else {
            Some(nanos as i64)
        }
    }

    /// The duration in seconds, as a float.
    pub fn as_seconds_f64(&self) -> f64 {
        self.secs as f64 + self.nanos as f64 / NANOS_PER_SEC as f64
    }

    pub fn abs(&self) -> TimeDelta {
        if self.secs < 0 {
            -*self
        } else {
            *self
        }
    }

    pub fn checked_add(&self, rhs: &TimeDelta) -> Option<TimeDelta> {
        Self::from_total_nanos(self.total_nanos() + rhs.total_nanos())
    }

    pub fn checked_sub(&self, rhs: &TimeDelta) -> Option<TimeDelta> {
        Self::from_total_nanos(self.total_nanos() - rhs.total_nanos())
    }

    /// The `std::time::Duration` of the same length, or an error if this is
    /// negative.
    pub fn to_std(&self) -> Result<std::time::Duration, OutOfRangeError> {
        if self.secs < 0 {
            return Err(OutOfRangeError);
        }

        Ok(std::time::Duration::new(
            self.secs.unsigned_abs(),
            self.nanos.unsigned_abs(),
        ))
    }

    /// The `TimeDelta` of the same length as a `std::time::Duration`, or an
    /// error if it is longer than `TimeDelta::MAX`.
    pub fn from_std(duration: std::time::Duration) -> Result<TimeDelta, OutOfRangeError> {
        Self::from_total_nanos(duration.as_nanos() as i128).ok_or(OutOfRangeError)
    }
}

impl Neg for TimeDelta {
    type Output = TimeDelta;

    fn neg(self) -> TimeDelta {
        // MIN and MAX are symmetric, so this is always in range
        Self::saturating(-self.total_nanos())
    }
}

impl Add for TimeDelta {
    type Output = TimeDelta;

    fn add(self, rhs: TimeDelta) -> TimeDelta {
        Self::saturating(self.total_nanos() + rhs.total_nanos())
    }
}

impl Sub for TimeDelta {
    type Output = TimeDelta;

    fn sub(self, rhs: TimeDelta) -> TimeDelta {
        Self::saturating(self.total_nanos() - rhs.total_nanos())
    }
}

impl AddAssign for TimeDelta {
    fn add_assign(&mut self, rhs: TimeDelta) {
        *self = *self + rhs;
    }
}

impl SubAssign for TimeDelta {
    fn sub_assign(&mut self, rhs: TimeDelta) {
        *self = *self - rhs;
    }
}

impl Mul<i32> for TimeDelta {
    type Output = TimeDelta;

    fn mul(self, rhs: i32) -> TimeDelta {
        Self::saturating(self.total_nanos().saturating_mul(rhs as i128))
    }
}

impl Div<i32> for TimeDelta {
    type Output = TimeDelta;

    /// Division by zero gives zero rather than panicking.
    fn div(self, rhs: i32) -> TimeDelta {
        match self.total_nanos().checked_div(rhs as i128) {
            Some(total) => Self::saturating(total),
            None => TimeDelta::zero(),
        }
    }
}

impl std::iter::Sum for TimeDelta {
    fn sum<I: Iterator<Item = TimeDelta>>(iter: I) -> TimeDelta {
        iter.fold(TimeDelta::zero(), |a, b| a + b)
    }
}

impl<'a> std::iter::Sum<&'a TimeDelta> for TimeDelta {
    fn sum<I: Iterator<Item = &'a TimeDelta>>(iter: I) -> TimeDelta {
        iter.fold(TimeDelta::zero(), |a, b| a + *b)
    }
}

/// ISO 8601, as chrono writes it: `PT3600S`, `-PT1.5S`, and `P0D` for zero.
impl fmt::Display for TimeDelta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (abs, sign) = if self.secs < 0 {
            (-*self, "-")
        } else {
            (*self, "")
        };

        write!(f, "{}P", sign)?;

        if abs.is_zero() {
            return f.write_str("0D");
        }

        write!(f, "T{}", abs.secs)?;

        if abs.nanos > 0 {
            let mut digits = 9usize;
            let mut fraction = abs.nanos;
            while fraction % 10 == 0 {
                fraction /= 10;
                digits -= 1;
            }
            write!(f, ".{:0width$}", fraction, width = digits)?;
        }

        f.write_str("S")
    }
}

impl fmt::Debug for TimeDelta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_saturates_instead_of_panicking() {
        assert_eq!(TimeDelta::seconds(i64::MAX), TimeDelta::MAX);
        assert_eq!(TimeDelta::seconds(i64::MIN), TimeDelta::MIN);
        assert_eq!(TimeDelta::days(i64::MAX), TimeDelta::MAX);
        assert_eq!(TimeDelta::MAX + TimeDelta::seconds(1), TimeDelta::MAX);
        assert_eq!(TimeDelta::MIN - TimeDelta::seconds(1), TimeDelta::MIN);
        assert_eq!(-TimeDelta::MIN, TimeDelta::MAX);
        assert_eq!(TimeDelta::MAX * 2, TimeDelta::MAX);
        assert_eq!(TimeDelta::seconds(10) / 0, TimeDelta::zero());
        assert_eq!(TimeDelta::try_seconds(i64::MAX), None);
        assert_eq!(TimeDelta::MAX.num_milliseconds(), i64::MAX);
        assert_eq!(TimeDelta::MIN.num_milliseconds(), -i64::MAX);
    }

    #[test]
    fn test_truncates_towards_zero() {
        let d = TimeDelta::milliseconds(-1500);
        assert_eq!(d.num_seconds(), -1);
        assert_eq!(d.subsec_nanos(), -500_000_000);
        assert_eq!(d.num_milliseconds(), -1500);
        assert_eq!(TimeDelta::seconds(-90_061).num_days(), -1);
        assert_eq!(TimeDelta::seconds(-90_061).num_hours(), -25);
    }

    #[test]
    fn test_std_conversions() {
        assert_eq!(
            TimeDelta::milliseconds(1500).to_std(),
            Ok(std::time::Duration::from_millis(1500))
        );
        assert_eq!(TimeDelta::seconds(-1).to_std(), Err(OutOfRangeError));
        assert_eq!(
            TimeDelta::from_std(std::time::Duration::from_secs(7)),
            Ok(TimeDelta::seconds(7))
        );
        assert_eq!(
            TimeDelta::from_std(std::time::Duration::from_secs(u64::MAX)),
            Err(OutOfRangeError)
        );
    }
}
