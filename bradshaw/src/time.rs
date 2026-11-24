// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Times of day.

use std::fmt;

use crate::format::{write_fraction, DelayedFormat};

/// The parts of a time of day: implemented by `NaiveTime`, `NaiveDateTime`
/// and `DateTime`.
pub trait Timelike: Sized {
    /// 0 to 23.
    fn hour(&self) -> u32;

    /// 0 to 59.
    fn minute(&self) -> u32;

    /// 0 to 59.
    fn second(&self) -> u32;

    /// 0 to 999,999,999.
    fn nanosecond(&self) -> u32;

    fn with_hour(&self, hour: u32) -> Option<Self>;

    fn with_minute(&self, minute: u32) -> Option<Self>;

    fn with_second(&self, second: u32) -> Option<Self>;

    fn with_nanosecond(&self, nanosecond: u32) -> Option<Self>;

    fn num_seconds_from_midnight(&self) -> u32 {
        self.hour() * 3600 + self.minute() * 60 + self.second()
    }
}

/// A time of day, to the nanosecond, with no date or time zone.
///
/// There are no leap seconds: a time is always 00:00:00 to 23:59:59.999999999.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct NaiveTime {
    secs: u32,
    nanos: u32,
}

impl NaiveTime {
    /// 00:00:00
    pub const MIN: NaiveTime = NaiveTime { secs: 0, nanos: 0 };

    pub const fn from_hms_opt(hour: u32, minute: u32, second: u32) -> Option<NaiveTime> {
        Self::from_hms_nano_opt(hour, minute, second, 0)
    }

    pub const fn from_hms_milli_opt(
        hour: u32,
        minute: u32,
        second: u32,
        milli: u32,
    ) -> Option<NaiveTime> {
        if milli >= 1000 {
            return None;
        }
        Self::from_hms_nano_opt(hour, minute, second, milli * 1_000_000)
    }

    pub const fn from_hms_micro_opt(
        hour: u32,
        minute: u32,
        second: u32,
        micro: u32,
    ) -> Option<NaiveTime> {
        if micro >= 1_000_000 {
            return None;
        }
        Self::from_hms_nano_opt(hour, minute, second, micro * 1000)
    }

    pub const fn from_hms_nano_opt(
        hour: u32,
        minute: u32,
        second: u32,
        nano: u32,
    ) -> Option<NaiveTime> {
        if hour >= 24 || minute >= 60 || second >= 60 || nano >= 1_000_000_000 {
            return None;
        }

        Some(NaiveTime {
            secs: hour * 3600 + minute * 60 + second,
            nanos: nano,
        })
    }

    /// The time `secs` seconds and `nano` nanoseconds after midnight.
    pub const fn from_num_seconds_from_midnight_opt(secs: u32, nano: u32) -> Option<NaiveTime> {
        if secs >= 86_400 || nano >= 1_000_000_000 {
            return None;
        }

        Some(NaiveTime { secs, nanos: nano })
    }

    pub(crate) const fn seconds_from_midnight(&self) -> u32 {
        self.secs
    }

    pub fn format<'a>(&self, pattern: &'a str) -> DelayedFormat<'a> {
        DelayedFormat::new(None, Some(*self), None, pattern)
    }
}

impl Timelike for NaiveTime {
    fn hour(&self) -> u32 {
        self.secs / 3600
    }

    fn minute(&self) -> u32 {
        self.secs / 60 % 60
    }

    fn second(&self) -> u32 {
        self.secs % 60
    }

    fn nanosecond(&self) -> u32 {
        self.nanos
    }

    fn with_hour(&self, hour: u32) -> Option<NaiveTime> {
        Self::from_hms_nano_opt(hour, self.minute(), self.second(), self.nanos)
    }

    fn with_minute(&self, minute: u32) -> Option<NaiveTime> {
        Self::from_hms_nano_opt(self.hour(), minute, self.second(), self.nanos)
    }

    fn with_second(&self, second: u32) -> Option<NaiveTime> {
        Self::from_hms_nano_opt(self.hour(), self.minute(), second, self.nanos)
    }

    fn with_nanosecond(&self, nanosecond: u32) -> Option<NaiveTime> {
        Self::from_hms_nano_opt(self.hour(), self.minute(), self.second(), nanosecond)
    }

    fn num_seconds_from_midnight(&self) -> u32 {
        self.secs
    }
}

/// `10:00:00`, with a fraction of 3, 6 or 9 digits when there is one.
impl fmt::Display for NaiveTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02}:{:02}:{:02}",
            self.hour(),
            self.minute(),
            self.second()
        )?;
        write_fraction(f, self.nanos)
    }
}

impl fmt::Debug for NaiveTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
