// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Parsing: RFC 3339, RFC 2822, and a small `strptime`.
//!
//! **RFC 3339** (`DateTime<Utc>`'s `FromStr`, `parse_from_rfc3339` and
//! deserialisation) accepts `date "T" time offset`:
//!
//! - `T`, `t` or a space between the date and the time (RFC 3339 §5.6 allows
//!   the space);
//! - an optional fraction of any length, truncated to nanoseconds;
//! - `Z`, `z`, or a `+hh:mm` / `-hh:mm` offset, which is applied;
//! - a leap second (`:60`), which is read as the first instant of the next
//!   minute, since there are no leap seconds here;
//! - as an extension, a year outside 0000-9999 written with a sign and at
//!   least four digits (`+12026-…`), because that is how chrono, and
//!   therefore older agents, write one.
//!
//! **RFC 2822** (`parse_from_rfc2822`) is what HTTP `Date` headers use:
//! `[Mon,] 3 Aug 2026 12:00[:00] GMT`, with names in any case, the weekday
//! checked against the date, and a zone of `+hhmm` / `-hhmm`, `GMT`, `UT`,
//! `UTC`, `Z`, or a North American zone (`EST`, `EDT`, `CST`, `CDT`, `MST`,
//! `MDT`, `PST`, `PDT`). Two- and three-digit years are read as RFC 2822
//! §4.3 says (`26` is 2026, `99` is 1999, `126` is 2026).
//!
//! **`strptime`** (`parse_from_str`) knows `%Y`, `%m`, `%d`, `%H`, `%M`,
//! `%S` and `%%`. A space in the pattern matches any amount of whitespace,
//! including none; a number may be preceded by whitespace; and the whole input
//! must be used. `%Y` is up to four digits, or a sign and up to nine.
//! Fields the pattern does not give are zero.

use std::fmt;

use crate::civil::days_in_month;
use crate::date::{MAX_YEAR, MIN_YEAR};
use crate::format::month_from_short_name;
use crate::{
    DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Timelike, Utc, Weekday,
};

/// Why a date or time could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(String);

impl ParseError {
    fn new(what: &str, input: &str) -> ParseError {
        ParseError(format!("{} in '{}'", what, input))
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

/// A position in the input, read a byte at a time. Every read is bounds
/// checked; nothing here can panic.
struct Cursor<'a> {
    input: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(input: &'a str) -> Self {
        Cursor {
            input,
            bytes: input.as_bytes(),
            pos: 0,
        }
    }

    fn error(&self, what: &str) -> ParseError {
        ParseError::new(what, self.input)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn at_end(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, byte: u8, what: &str) -> Result<(), ParseError> {
        if self.eat(byte) {
            Ok(())
        } else {
            Err(self.error(what))
        }
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    /// Between `min` and `max` decimal digits, as a number.
    fn digits(&mut self, min: usize, max: usize, what: &str) -> Result<i64, ParseError> {
        let start = self.pos;
        let mut value: i64 = 0;

        while self.pos - start < max {
            match self.peek() {
                Some(b) if b.is_ascii_digit() => {
                    value = value * 10 + (b - b'0') as i64;
                    self.pos += 1;
                }
                _ => break,
            }
        }

        if self.pos - start < min {
            return Err(self.error(what));
        }

        Ok(value)
    }

    /// Letters, as a string slice.
    fn word(&mut self) -> &'a str {
        let start = self.pos;
        while self.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
            self.pos += 1;
        }
        self.input.get(start..self.pos).unwrap_or("")
    }

    /// A year: four digits, or (as chrono writes years outside 0000-9999) a
    /// sign and four or more.
    fn year(&mut self) -> Result<i64, ParseError> {
        if self.eat(b'+') {
            self.digits(4, 9, "a year")
        } else if self.eat(b'-') {
            Ok(-self.digits(4, 9, "a year")?)
        } else {
            self.digits(4, 4, "a four-digit year")
        }
    }

    /// `yyyy-mm-dd`
    fn full_date(&mut self) -> Result<NaiveDate, ParseError> {
        let year = self.year()?;
        self.expect(b'-', "'-' after the year")?;
        let month = self.digits(2, 2, "a two-digit month")?;
        self.expect(b'-', "'-' after the month")?;
        let day = self.digits(2, 2, "a two-digit day")?;
        make_date(year, month, day).ok_or_else(|| self.error("an invalid date"))
    }

    /// `hh:mm:ss[.fraction]`, returning the time and whether it was a leap
    /// second.
    fn partial_time(&mut self) -> Result<(NaiveTime, bool), ParseError> {
        let hour = self.digits(2, 2, "a two-digit hour")?;
        self.expect(b':', "':' after the hour")?;
        let minute = self.digits(2, 2, "two-digit minutes")?;
        self.expect(b':', "':' after the minutes")?;
        let second = self.digits(2, 2, "two-digit seconds")?;

        let mut nanos: i64 = 0;

        if self.eat(b'.') {
            let start = self.pos;
            let mut scale: i64 = 100_000_000;

            while let Some(b) = self.peek().filter(u8::is_ascii_digit) {
                // digits past the ninth are truncated
                nanos += (b - b'0') as i64 * scale;
                scale /= 10;
                self.pos += 1;
            }

            if self.pos == start {
                return Err(self.error("digits after the '.'"));
            }
        }

        let leap = second == 60;

        let time = NaiveTime::from_hms_nano_opt(
            u32::try_from(hour).unwrap_or(u32::MAX),
            u32::try_from(minute).unwrap_or(u32::MAX),
            u32::try_from(if leap { 59 } else { second }).unwrap_or(u32::MAX),
            u32::try_from(nanos).unwrap_or(u32::MAX),
        )
        .ok_or_else(|| self.error("an invalid time"))?;

        Ok((time, leap))
    }
}

fn make_date(year: i64, month: i64, day: i64) -> Option<NaiveDate> {
    if !(MIN_YEAR..=MAX_YEAR).contains(&year) || !(1..=12).contains(&month) || day < 1 {
        return None;
    }

    let month = u32::try_from(month).ok()?;

    if day > days_in_month(year, month) as i64 {
        return None;
    }

    NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, u32::try_from(day).ok()?)
}

/// A leap second is read as the next instant after second 59 - the first of
/// the next minute.
fn finish_time(datetime: NaiveDateTime, leap: bool) -> NaiveDateTime {
    if leap {
        let second = TimeDelta::seconds(1);
        let start_of_second = datetime
            .checked_sub_signed(TimeDelta::nanoseconds(datetime.time().nanosecond() as i64))
            .unwrap_or(datetime);
        start_of_second + second
    } else {
        datetime
    }
}

/// `yyyy-mm-dd`, the RFC 3339 `full-date` - also how a `NaiveDate` is
/// serialised.
pub(crate) fn parse_date(s: &str) -> Result<NaiveDate, ParseError> {
    let mut cursor = Cursor::new(s);
    let date = cursor.full_date()?;

    if !cursor.at_end() {
        return Err(cursor.error("unexpected text after the date"));
    }

    Ok(date)
}

/// `yyyy-mm-ddThh:mm:ss[.fraction]` - RFC 3339 with no offset, which is how a
/// `NaiveDateTime` is serialised.
pub(crate) fn parse_naive_datetime(s: &str) -> Result<NaiveDateTime, ParseError> {
    let mut cursor = Cursor::new(s);
    let date = cursor.full_date()?;

    if !(cursor.eat(b'T') || cursor.eat(b't') || cursor.eat(b' ')) {
        return Err(cursor.error("'T' between the date and the time"));
    }

    let (time, leap) = cursor.partial_time()?;

    if !cursor.at_end() {
        return Err(cursor.error("unexpected text after the time"));
    }

    Ok(finish_time(date.and_time(time), leap))
}

/// An RFC 3339 timestamp - see the module docs.
pub(crate) fn parse_rfc3339(s: &str) -> Result<DateTime<Utc>, ParseError> {
    let mut cursor = Cursor::new(s);
    let date = cursor.full_date()?;

    if !(cursor.eat(b'T') || cursor.eat(b't') || cursor.eat(b' ')) {
        return Err(cursor.error("'T' between the date and the time"));
    }

    let (time, leap) = cursor.partial_time()?;

    let offset_seconds: i64 = if cursor.eat(b'Z') || cursor.eat(b'z') {
        0
    } else {
        let sign = if cursor.eat(b'+') {
            1
        } else if cursor.eat(b'-') {
            -1
        } else {
            return Err(cursor.error("an offset ('Z', '+hh:mm' or '-hh:mm')"));
        };

        let hours = cursor.digits(2, 2, "a two-digit offset hour")?;
        cursor.expect(b':', "':' in the offset")?;
        let minutes = cursor.digits(2, 2, "two-digit offset minutes")?;

        if hours > 23 || minutes > 59 {
            return Err(cursor.error("an invalid offset"));
        }

        sign * (hours * 3600 + minutes * 60)
    };

    if !cursor.at_end() {
        return Err(cursor.error("unexpected text after the offset"));
    }

    // local time minus the offset is UTC
    finish_time(date.and_time(time), leap)
        .checked_sub_signed(TimeDelta::seconds(offset_seconds))
        .map(|utc| utc.and_utc())
        .ok_or_else(|| cursor.error("a time out of range"))
}

/// An RFC 2822 date - see the module docs.
pub(crate) fn parse_rfc2822(s: &str) -> Result<DateTime<Utc>, ParseError> {
    let mut cursor = Cursor::new(s);
    cursor.skip_whitespace();

    // optional day of the week
    let mut weekday: Option<Weekday> = None;

    if cursor.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
        let name = cursor.word();
        weekday = Some(Weekday::from_short_name(name).ok_or_else(|| cursor.error("a weekday"))?);
        cursor.skip_whitespace();
        cursor.expect(b',', "',' after the weekday")?;
        cursor.skip_whitespace();
    }

    let day = cursor.digits(1, 2, "the day")?;
    cursor.skip_whitespace();

    let month = month_from_short_name(cursor.word()).ok_or_else(|| cursor.error("a month"))?;
    cursor.skip_whitespace();

    let start = cursor.pos;
    let year = cursor.digits(2, 9, "the year")?;
    let year = match cursor.pos - start {
        // RFC 2822 §4.3: two digits are 1950-2049, three are added to 1900
        2 if year < 50 => year + 2000,
        2 | 3 => year + 1900,
        _ => year,
    };

    cursor.skip_whitespace();

    let hour = cursor.digits(2, 2, "a two-digit hour")?;
    cursor.expect(b':', "':' after the hour")?;
    let minute = cursor.digits(2, 2, "two-digit minutes")?;
    let second = if cursor.eat(b':') {
        cursor.digits(2, 2, "two-digit seconds")?
    } else {
        0
    };

    cursor.skip_whitespace();

    let offset_minutes: i64 = match cursor.peek() {
        Some(b'+') | Some(b'-') => {
            let sign = if cursor.eat(b'-') { -1 } else { 1 };
            cursor.eat(b'+');
            let hhmm = cursor.digits(4, 4, "a four-digit zone offset")?;
            let (hours, minutes) = (hhmm / 100, hhmm % 100);
            if minutes > 59 {
                return Err(cursor.error("an invalid zone offset"));
            }
            sign * (hours * 60 + minutes)
        }
        _ => match cursor.word().to_ascii_uppercase().as_str() {
            "GMT" | "UT" | "UTC" | "Z" => 0,
            "EDT" => -4 * 60,
            "EST" | "CDT" => -5 * 60,
            "CST" | "MDT" => -6 * 60,
            "MST" | "PDT" => -7 * 60,
            "PST" => -8 * 60,
            _ => return Err(cursor.error("a zone")),
        },
    };

    cursor.skip_whitespace();

    if !cursor.at_end() {
        return Err(cursor.error("unexpected text after the zone"));
    }

    let date = make_date(year, month as i64, day).ok_or_else(|| cursor.error("an invalid date"))?;

    if weekday.is_some_and(|weekday| weekday != date.weekday()) {
        return Err(cursor.error("a weekday that does not match the date"));
    }

    let leap = second == 60;
    let time = NaiveTime::from_hms_opt(
        u32::try_from(hour).unwrap_or(u32::MAX),
        u32::try_from(minute).unwrap_or(u32::MAX),
        u32::try_from(if leap { 59 } else { second }).unwrap_or(u32::MAX),
    )
    .ok_or_else(|| cursor.error("an invalid time"))?;

    finish_time(date.and_time(time), leap)
        .checked_sub_signed(TimeDelta::minutes(offset_minutes))
        .map(|utc| utc.and_utc())
        .ok_or_else(|| cursor.error("a time out of range"))
}

/// The fields a `strptime` pattern read.
pub(crate) struct Parsed<'a> {
    input: &'a str,
    year: Option<i64>,
    month: Option<i64>,
    day: Option<i64>,
    hour: i64,
    minute: i64,
    second: i64,
}

impl Parsed<'_> {
    pub(crate) fn date(&self) -> Result<NaiveDate, ParseError> {
        match (self.year, self.month, self.day) {
            (Some(year), Some(month), Some(day)) => make_date(year, month, day)
                .ok_or_else(|| ParseError::new("an invalid date", self.input)),
            _ => Err(ParseError::new(
                "a pattern without %Y, %m and %d",
                self.input,
            )),
        }
    }

    pub(crate) fn datetime(&self) -> Result<NaiveDateTime, ParseError> {
        let date = self.date()?;

        let time = NaiveTime::from_hms_opt(
            u32::try_from(self.hour).unwrap_or(u32::MAX),
            u32::try_from(self.minute).unwrap_or(u32::MAX),
            u32::try_from(self.second).unwrap_or(u32::MAX),
        )
        .ok_or_else(|| ParseError::new("an invalid time", self.input))?;

        Ok(date.and_time(time))
    }
}

/// Read `s` with a `strptime`-style `pattern` - see the module docs.
pub(crate) fn parse_with_pattern<'a>(s: &'a str, pattern: &str) -> Result<Parsed<'a>, ParseError> {
    let mut cursor = Cursor::new(s);

    let mut parsed = Parsed {
        input: s,
        year: None,
        month: None,
        day: None,
        hour: 0,
        minute: 0,
        second: 0,
    };

    let mut chars = pattern.chars();

    while let Some(c) = chars.next() {
        if c == '%' {
            let spec = chars
                .next()
                .ok_or_else(|| cursor.error("a pattern ending in '%'"))?;

            if spec == '%' {
                cursor.expect(b'%', "'%'")?;
                continue;
            }

            cursor.skip_whitespace();

            match spec {
                'Y' => {
                    parsed.year = Some(if cursor.eat(b'+') {
                        cursor.digits(1, 9, "a year")?
                    } else if cursor.eat(b'-') {
                        -cursor.digits(1, 9, "a year")?
                    } else {
                        cursor.digits(1, 4, "a year")?
                    })
                }
                'm' => parsed.month = Some(cursor.digits(1, 2, "a month")?),
                'd' => parsed.day = Some(cursor.digits(1, 2, "a day")?),
                'H' => parsed.hour = cursor.digits(1, 2, "an hour")?,
                'M' => parsed.minute = cursor.digits(1, 2, "minutes")?,
                'S' => parsed.second = cursor.digits(1, 2, "seconds")?,
                _ => return Err(cursor.error("an unsupported pattern specifier")),
            }
        } else if c.is_whitespace() {
            cursor.skip_whitespace();
        } else {
            let mut buffer = [0u8; 4];
            for byte in c.encode_utf8(&mut buffer).bytes() {
                cursor.expect(byte, "text matching the pattern")?;
            }
        }
    }

    if !cursor.at_end() {
        return Err(cursor.error("unexpected text after the pattern"));
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rfc3339_offsets_and_leap_seconds() {
        let utc = parse_rfc3339("2026-11-23T10:00:00Z").unwrap();
        assert_eq!(utc.timestamp(), 1_795_428_000);
        assert_eq!(parse_rfc3339("2026-11-23T11:30:00+01:30").unwrap(), utc);
        assert_eq!(
            parse_rfc3339("2026-11-23T10:00:60Z").unwrap().timestamp(),
            1_795_428_060
        );
        assert_eq!(
            parse_rfc3339("2026-12-31T23:59:60.5Z").unwrap(),
            parse_rfc3339("2027-01-01T00:00:00Z").unwrap()
        );
        // not RFC 3339
        for bad in [
            "2026-11-23T10:00:00+0100",
            "2026-11-23T10:00:00",
            "26-11-23T10:00:00Z",
            "2026-11-23T10:00Z",
            "2026-11-23T10:00:00ZZ",
            "2026-11-23T10:00:00+24:00",
            "2026-11-23T10:00:61Z",
            "2026-11-23T10:00:00.Z",
            "２０２６-11-23T10:00:00Z",
        ] {
            assert!(parse_rfc3339(bad).is_err(), "{}", bad);
        }
    }

    #[test]
    fn test_rfc2822() {
        let expected = 1_785_758_400;
        for good in [
            "Mon, 03 Aug 2026 12:00:00 GMT",
            "Mon, 3 Aug 2026 12:00:00 GMT",
            "03 Aug 2026 12:00:00 GMT",
            "Mon, 03 Aug 26 12:00:00 GMT",
            "Mon, 03 Aug 126 12:00:00 GMT",
            "Mon, 03 Aug 2026 13:00:00 +0100",
            "Mon, 03 Aug 2026 12:00:00 -0000",
            "  Mon, 03 Aug 2026 12:00:00 UTC  ",
        ] {
            assert_eq!(
                parse_rfc2822(good).map(|t| t.timestamp()),
                Ok(expected),
                "{}",
                good
            );
        }
        assert_eq!(
            parse_rfc2822("Tue, 03 Aug 1999 12:00:00 GMT")
                .unwrap()
                .year(),
            1999
        );
        for bad in [
            "Mon, 03 Aug 2026 12:00:00 XYZ",
            "Mon, 03 Aug 2026 12:00:00 GMT extra",
            "Mon 03 Aug 2026 12:00:00 GMT",
            "Mon, 03 Aug 2026 12:00:00 +01:00",
        ] {
            assert!(parse_rfc2822(bad).is_err(), "{}", bad);
        }
    }

    #[test]
    fn test_strptime_hour_pattern() {
        // chrono rejected every input to this pattern, because it needs the
        // minutes too; missing fields are zero here
        let hour = NaiveDateTime::parse_from_str("2026-11-23 10", "%Y-%m-%d %H").unwrap();
        assert_eq!(hour.to_string(), "2026-11-23 10:00:00");
        assert!(NaiveDateTime::parse_from_str("2026-11-23 24", "%Y-%m-%d %H").is_err());
        assert!(NaiveDateTime::parse_from_str("2026-11-23 10 ", "%Y-%m-%d %H").is_err());
        assert!(NaiveDate::parse_from_str("2026-11-23", "%Y-%m").is_err());
        assert!(NaiveDate::parse_from_str("2026-11-23", "%Y-%m-%d %Q").is_err());
    }
}
