// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! bradshaw against chrono.
//!
//! `golden.tsv` was recorded from chrono 0.4.45 - the version OpenPortal used
//! before bradshaw - by the program in `golden_generator.rs.txt`. Each line is
//! a value and everything chrono produced from it: its serialised forms, its
//! `Display` and `Debug`, every `format` pattern OpenPortal uses, its calendar
//! fields, and what chrono made of a string it was asked to parse.
//!
//! chrono is not a dependency, not even of this test: `cargo audit` reads the
//! whole dependency graph, so the vectors are recorded once and kept here.
//! To re-record them, build the generator against chrono in a scratch crate.
//!
//! The few deliberate differences are listed, with the reason, in
//! `INTENDED_DIFFERENCES`.

// The exemptions `clippy.toml` gives unit tests, which do not reach the helper
// functions of an integration test: a malformed vector should fail loudly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
// An integration test is a crate of its own, given the package's whole
// dependency list - including the optional integrations it does not use.
#![allow(unused_crate_dependencies)]

use bradshaw::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeDelta, Timelike, Utc};

const GOLDEN: &str = include_str!("golden.tsv");

const PATTERNS: &[&str] = &[
    "%H:%M:%S UTC",
    "%Y-%m",
    "%Y-%m-%d %H",
    "%Y-%m-%d %H:%M UTC",
    "%Y-%m-%d %H:%M:%S UTC",
    "%Y-%m-%d %H:%M:%S",
    "%Y-%m-%d",
    "%Y-%m-%dT%H:%M:%S",
    "%a, %d %b %Y %H:%M:%S GMT",
];

/// (kind, input, what bradshaw gives, why it differs from chrono)
const INTENDED_DIFFERENCES: &[(&str, &str, &str, &str)] = &[
    (
        "P3339",
        "2026-11-23T10:00:00+0100",
        "ERR",
        "not RFC 3339, which requires the colon in the offset; chrono was lenient",
    ),
    (
        "P3339",
        "2026-11-23T10:00:60Z",
        "1795428060 0",
        "there are no leap seconds, so :60 is the start of the next minute; chrono kept it as second 59 plus a billion nanoseconds",
    ),
    (
        "PH",
        "2026-11-23 10",
        "2026-11-23 10:00:00",
        "chrono could not parse \"%Y-%m-%d %H\" at all (it needs the minutes), so Hour::parse - and so deserialising an Hour - always failed",
    ),
    ("PH", "2026-11-23 7", "2026-11-23 07:00:00", "as above"),
    ("PH", "2026-11-23  10", "2026-11-23 10:00:00", "as above"),
];

fn expected<'a>(kind: &str, input: &str, chrono: &'a str) -> &'a str {
    INTENDED_DIFFERENCES
        .iter()
        .find(|(k, i, _, _)| *k == kind && *i == input)
        .map(|(_, _, ours, _)| *ours)
        .unwrap_or(chrono)
}

fn lines(kind: &str) -> Vec<Vec<&'static str>> {
    let lines: Vec<Vec<&str>> = GOLDEN
        .lines()
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .filter(|fields| fields[0] == kind)
        .map(|fields| fields[1..].to_vec())
        .collect();
    assert!(!lines.is_empty(), "no {} lines", kind);
    lines
}

fn pair(field: &str) -> (i64, i64) {
    let mut parts = field.split(' ');
    (
        parts.next().unwrap().parse().unwrap(),
        parts.next().unwrap().parse().unwrap(),
    )
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Secs {
    #[serde(with = "bradshaw::serde::ts_seconds")]
    t: DateTime<Utc>,
}

#[test]
fn test_timestamps() {
    let lines = lines("T");
    assert!(lines.len() > 400);

    for f in lines {
        let (secs, nanos) = pair(f[0]);
        let t = DateTime::from_timestamp(secs, nanos as u32).unwrap();
        let context = f[0];

        assert_eq!(
            serde_json::to_string(&t).unwrap(),
            f[1],
            "serde {}",
            context
        );
        assert_eq!(
            serde_json::from_str::<DateTime<Utc>>(f[1]).unwrap(),
            t,
            "{}",
            context
        );
        assert_eq!(t.to_string(), f[2], "Display {}", context);
        assert_eq!(format!("{:?}", t), f[3], "Debug {}", context);
        assert_eq!(t.to_rfc3339(), f[4], "to_rfc3339 {}", context);
        assert_eq!(f[4].parse::<DateTime<Utc>>().unwrap(), t, "{}", context);
        assert_eq!(
            serde_json::to_string(&Secs { t }).unwrap(),
            f[5],
            "ts_seconds {}",
            context
        );
        assert_eq!(
            serde_json::from_str::<Secs>(f[5]).unwrap().t.timestamp(),
            secs,
            "{}",
            context
        );
        assert_eq!(
            format!(
                "{} {} {} {} {} {} {} {}",
                t.year(),
                t.month(),
                t.day(),
                t.hour(),
                t.minute(),
                t.second(),
                t.weekday(),
                t.timestamp_millis()
            ),
            f[6],
            "fields {}",
            context
        );
        assert_eq!(t.timestamp(), secs, "{}", context);
        assert_eq!(t.timestamp_subsec_nanos() as i64, nanos, "{}", context);
        assert_eq!(t.date_naive().to_string(), f[7], "date_naive {}", context);
        assert_eq!(
            format!("{:?}", t.naive_utc()),
            f[8],
            "naive Debug {}",
            context
        );
        assert_eq!(t.naive_utc().to_string(), f[9], "naive Display {}", context);
        assert_eq!(
            serde_json::to_string(&t.naive_utc()).unwrap(),
            f[10],
            "{}",
            context
        );
        assert_eq!(
            serde_json::from_str::<NaiveDateTime>(f[10]).unwrap(),
            t.naive_utc(),
            "{}",
            context
        );

        for (i, pattern) in PATTERNS.iter().enumerate() {
            assert_eq!(
                t.format(pattern).to_string(),
                f[11 + i],
                "{} {}",
                pattern,
                context
            );
        }

        // the HTTP date form reads back to the same second
        assert_eq!(
            DateTime::parse_from_rfc2822(f[19]).map(|p| p.timestamp()),
            Ok(secs),
            "{}",
            f[19]
        );
    }
}

#[test]
fn test_dates() {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();

    for f in lines("D") {
        let days: i64 = f[0].parse().unwrap();
        let date = epoch + TimeDelta::days(days);
        let context = f[0];

        assert_eq!(date.to_string(), f[1], "Display {}", context);
        assert_eq!(format!("{:?}", date), f[2], "Debug {}", context);
        assert_eq!(
            serde_json::to_string(&date).unwrap(),
            f[3],
            "serde {}",
            context
        );
        assert_eq!(
            serde_json::from_str::<NaiveDate>(f[3]).unwrap(),
            date,
            "{}",
            context
        );
        assert_eq!(
            format!(
                "{} {} {} {} {} {}",
                date.year(),
                date.month(),
                date.day(),
                date.weekday(),
                date.ordinal(),
                date.weekday().num_days_from_monday()
            ),
            f[4],
            "fields {}",
            context
        );
        assert_eq!(
            NaiveDate::from_ymd_opt(date.year(), date.month(), date.day()),
            Some(date),
            "{}",
            context
        );
        assert_eq!(
            date.succ_opt()
                .map(|d| d.to_string())
                .unwrap_or("NONE".into()),
            f[5]
        );
        assert_eq!(
            date.pred_opt()
                .map(|d| d.to_string())
                .unwrap_or("NONE".into()),
            f[6]
        );
        assert_eq!(date.format("%Y-%m").to_string(), f[7], "{}", context);
        assert_eq!(
            date.and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp()
                .to_string(),
            f[8],
            "{}",
            context
        );
        assert_eq!(date.signed_duration_since(epoch), TimeDelta::days(days));
    }
}

#[test]
fn test_durations() {
    for f in lines("U") {
        let (s, n) = pair(f[0]);
        let d = TimeDelta::seconds(s) + TimeDelta::nanoseconds(n);
        let context = f[0];

        assert_eq!(d.to_string(), f[1], "Display {}", context);
        assert_eq!(
            format!(
                "{} {} {} {} {}",
                d.num_days(),
                d.num_hours(),
                d.num_minutes(),
                d.num_seconds(),
                d.num_milliseconds()
            ),
            f[2],
            "{}",
            context
        );
        assert_eq!(
            d.to_std()
                .map(|x| format!("{} {}", x.as_secs(), x.subsec_nanos()))
                .unwrap_or("ERR".into()),
            f[3],
            "{}",
            context
        );
    }
}

fn check_parses(kind: &str, parse: impl Fn(&str) -> String) {
    for f in lines(kind) {
        let (input, chrono) = (f[0], f[1]);
        assert_eq!(
            parse(input),
            expected(kind, input, chrono),
            "{} {:?}",
            kind,
            input
        );
    }
}

#[test]
fn test_rfc3339_parsing() {
    check_parses("P3339", |s| {
        serde_json::from_str::<DateTime<Utc>>(&format!("\"{}\"", s))
            .map(|t| format!("{} {}", t.timestamp(), t.timestamp_subsec_nanos()))
            .unwrap_or("ERR".into())
    });
}

#[test]
fn test_rfc2822_parsing() {
    check_parses("P2822", |s| {
        DateTime::parse_from_rfc2822(s)
            .map(|t| t.timestamp().to_string())
            .unwrap_or("ERR".into())
    });
}

#[test]
fn test_pattern_parsing() {
    check_parses("PD", |s| {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(|d| d.to_string())
            .unwrap_or("ERR".into())
    });
    check_parses("PH", |s| {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H")
            .map(|d| d.to_string())
            .unwrap_or("ERR".into())
    });
}
