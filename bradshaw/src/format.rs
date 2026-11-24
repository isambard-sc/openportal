// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! `strftime`-style formatting.
//!
//! | Specifier | Meaning | Example |
//! |---|---|---|
//! | `%Y` | Year, at least four digits, signed outside 0000-9999 | `2026`, `+12026` |
//! | `%C` / `%y` | Year divided by / modulo 100, two digits | `20` / `26` |
//! | `%m` | Month, two digits | `03` |
//! | `%b` / `%h` / `%B` | Month name, short / short / long | `Mar` / `March` |
//! | `%d` / `%e` | Day of the month, zero- / space-padded | `05` / ` 5` |
//! | `%a` / `%A` | Weekday name, short / long | `Thu` / `Thursday` |
//! | `%u` / `%w` | Weekday number, Monday = 1 / Sunday = 0 | `4` |
//! | `%j` | Day of the year, three digits | `064` |
//! | `%F` | `%Y-%m-%d` | `2026-03-05` |
//! | `%H` / `%I` | Hour, 24- / 12-hour, two digits | `14` / `02` |
//! | `%p` | `AM` / `PM` | `PM` |
//! | `%M` / `%S` | Minute / second, two digits | `07` |
//! | `%f` | Nanoseconds, nine digits | `123000000` |
//! | `%.f` | `.` and the fraction in 3, 6 or 9 digits, or nothing | `.123` |
//! | `%.3f` / `%.6f` / `%.9f` | `.` and the fraction in that many digits | `.123000` |
//! | `%T` / `%R` | `%H:%M:%S` / `%H:%M` | `14:07:09` |
//! | `%s` | Seconds since the epoch | `1772719629` |
//! | `%Z` / `%z` / `%:z` | The zone: `UTC` / `+0000` / `+00:00` | |
//! | `%%` / `%n` / `%t` | `%` / newline / tab | |
//!
//! A specifier this does not know, or one asking for something the value does
//! not have (a time, from a date), is written out unchanged rather than
//! failing - so a mistake shows up in the output, and never as a panic.

use std::fmt;

use crate::{Datelike, NaiveDate, NaiveTime, Timelike};

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn month_name(month: u32) -> &'static str {
    MONTHS
        .get((month as usize).wrapping_sub(1))
        .copied()
        .unwrap_or("")
}

/// The month (1..=12) a three-letter name (any case) names.
pub(crate) fn month_from_short_name(name: &str) -> Option<u32> {
    MONTHS
        .iter()
        .position(|month| month.get(..3).is_some_and(|m| m.eq_ignore_ascii_case(name)))
        .map(|index| index as u32 + 1)
}

/// The year as chrono writes it: four digits from 0000 to 9999, and otherwise
/// a sign and at least four digits (`+12026`, `-0001`).
pub(crate) fn write_year(f: &mut impl fmt::Write, year: i64) -> fmt::Result {
    if (0..=9999).contains(&year) {
        write!(f, "{:04}", year)
    } else if year < 0 {
        write!(f, "-{:04}", year.unsigned_abs())
    } else {
        write!(f, "+{:04}", year)
    }
}

/// A fraction of a second as chrono writes it in its default formats: nothing
/// for a whole second, otherwise `.` and the shortest of 3, 6 or 9 digits
/// that is exact.
pub(crate) fn write_fraction(f: &mut impl fmt::Write, nanos: u32) -> fmt::Result {
    if nanos == 0 {
        Ok(())
    } else if nanos.is_multiple_of(1_000_000) {
        write!(f, ".{:03}", nanos / 1_000_000)
    } else if nanos.is_multiple_of(1_000) {
        write!(f, ".{:06}", nanos / 1_000)
    } else {
        write!(f, ".{:09}", nanos)
    }
}

/// A value and a pattern, formatted when displayed - see the module docs.
#[derive(Debug, Clone)]
pub struct DelayedFormat<'a> {
    date: Option<NaiveDate>,
    time: Option<NaiveTime>,
    // present only for a `DateTime<Utc>`, which is what has a zone
    timestamp: Option<i64>,
    pattern: &'a str,
}

impl<'a> DelayedFormat<'a> {
    pub(crate) fn new(
        date: Option<NaiveDate>,
        time: Option<NaiveTime>,
        timestamp: Option<i64>,
        pattern: &'a str,
    ) -> Self {
        Self {
            date,
            time,
            timestamp,
            pattern,
        }
    }

    /// Write one specifier (the text after `%`, e.g. `"Y"` or `".3f"`).
    /// Returns false if it could not be written, so the caller writes it
    /// unchanged.
    fn write_spec(&self, f: &mut fmt::Formatter<'_>, spec: &str) -> Result<bool, fmt::Error> {
        let date = self.date;
        let time = self.time;

        match (spec, date, time) {
            ("Y", Some(d), _) => write_year(f, d.year() as i64)?,
            ("C", Some(d), _) => write!(f, "{:02}", (d.year() as i64).div_euclid(100))?,
            ("y", Some(d), _) => write!(f, "{:02}", (d.year() as i64).rem_euclid(100))?,
            ("m", Some(d), _) => write!(f, "{:02}", d.month())?,
            ("b" | "h", Some(d), _) => f.write_str(month_name(d.month()).get(..3).unwrap_or(""))?,
            ("B", Some(d), _) => f.write_str(month_name(d.month()))?,
            ("d", Some(d), _) => write!(f, "{:02}", d.day())?,
            ("e", Some(d), _) => write!(f, "{:>2}", d.day())?,
            ("a", Some(d), _) => f.write_str(d.weekday().short_name())?,
            ("A", Some(d), _) => f.write_str(d.weekday().long_name())?,
            ("u", Some(d), _) => write!(f, "{}", d.weekday().number_from_monday())?,
            ("w", Some(d), _) => write!(f, "{}", d.weekday().num_days_from_sunday())?,
            ("j", Some(d), _) => write!(f, "{:03}", d.ordinal())?,
            ("F", Some(d), _) => {
                write_year(f, d.year() as i64)?;
                write!(f, "-{:02}-{:02}", d.month(), d.day())?;
            }
            ("H", _, Some(t)) => write!(f, "{:02}", t.hour())?,
            ("I", _, Some(t)) => write!(f, "{:02}", (t.hour() + 11) % 12 + 1)?,
            ("p", _, Some(t)) => f.write_str(if t.hour() < 12 { "AM" } else { "PM" })?,
            ("M", _, Some(t)) => write!(f, "{:02}", t.minute())?,
            ("S", _, Some(t)) => write!(f, "{:02}", t.second())?,
            ("f", _, Some(t)) => write!(f, "{:09}", t.nanosecond())?,
            (".f", _, Some(t)) => write_fraction(f, t.nanosecond())?,
            (".3f", _, Some(t)) => write!(f, ".{:03}", t.nanosecond() / 1_000_000)?,
            (".6f", _, Some(t)) => write!(f, ".{:06}", t.nanosecond() / 1_000)?,
            (".9f", _, Some(t)) => write!(f, ".{:09}", t.nanosecond())?,
            ("T", _, Some(t)) => write!(f, "{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second())?,
            ("R", _, Some(t)) => write!(f, "{:02}:{:02}", t.hour(), t.minute())?,
            ("s" | "Z" | "z" | ":z", _, _) => match (spec, self.timestamp) {
                ("s", Some(timestamp)) => write!(f, "{}", timestamp)?,
                ("Z", Some(_)) => f.write_str("UTC")?,
                ("z", Some(_)) => f.write_str("+0000")?,
                (":z", Some(_)) => f.write_str("+00:00")?,
                _ => return Ok(false),
            },
            ("%", _, _) => f.write_str("%")?,
            ("n", _, _) => f.write_str("\n")?,
            ("t", _, _) => f.write_str("\t")?,
            _ => return Ok(false),
        }

        Ok(true)
    }
}

impl fmt::Display for DelayedFormat<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut rest = self.pattern;

        while let Some(percent) = rest.find('%') {
            let (literal, after) = rest.split_at(percent);
            f.write_str(literal)?;

            // after = "%..." - find the specifier's length
            let spec_text = after.get(1..).unwrap_or("");
            let spec_len = if spec_text.starts_with(".3f")
                || spec_text.starts_with(".6f")
                || spec_text.starts_with(".9f")
            {
                3
            } else if spec_text.starts_with(".f") || spec_text.starts_with(":z") {
                2
            } else {
                spec_text.chars().next().map(char::len_utf8).unwrap_or(0)
            };

            let spec = spec_text.get(..spec_len).unwrap_or("");

            if spec.is_empty() || !self.write_spec(f, spec)? {
                // unknown, or not available for this value: write it unchanged
                f.write_str("%")?;
                f.write_str(spec)?;
            }

            rest = spec_text.get(spec_len..).unwrap_or("");
        }

        f.write_str(rest)
    }
}

#[cfg(test)]
mod tests {
    use crate::{DateTime, NaiveDate};

    #[test]
    fn test_specifiers() {
        let t = DateTime::from_timestamp(1_772_719_629, 123_000_000).unwrap();
        assert_eq!(
            t.format("%Y-%m-%d %H:%M:%S%.f %Z").to_string(),
            "2026-03-05 14:07:09.123 UTC"
        );
        assert_eq!(
            t.format("%a %A %b %B %e %j %u %w %y %C").to_string(),
            "Thu Thursday Mar March  5 064 4 4 26 20"
        );
        assert_eq!(
            t.format("%I %p %T %R %s").to_string(),
            "02 PM 14:07:09 14:07 1772719629"
        );
        assert_eq!(
            t.format("%.3f %.6f %.9f %f").to_string(),
            ".123 .123000 .123000000 123000000"
        );
        assert_eq!(
            t.format("%z %:z %F %% %n%t").to_string(),
            "+0000 +00:00 2026-03-05 % \n\t"
        );
    }

    #[test]
    fn test_unknown_or_unavailable_is_written_unchanged() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 5).unwrap();
        assert_eq!(
            date.format("%Y %H:%M %Q %Z %").to_string(),
            "2026 %H:%M %Q %Z %"
        );
        assert_eq!(date.format("ünï %d €").to_string(), "ünï 05 €");
        assert_eq!(date.format("%ü").to_string(), "%ü");
    }
}
