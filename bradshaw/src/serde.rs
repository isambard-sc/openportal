// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Serialisation, in the same forms chrono uses, so the wire format is
//! unchanged:
//!
//! | Type | Form |
//! |---|---|
//! | `DateTime<Utc>` | `"2026-11-23T10:00:00Z"` (RFC 3339), with a fraction of 3, 6 or 9 digits when there is one; any RFC 3339 offset is accepted |
//! | `NaiveDate` | `"2026-11-23"` |
//! | `NaiveDateTime` | `"2026-11-23T10:00:00"` |
//! | `DateTime<Utc>` with `#[serde(with = "bradshaw::serde::ts_seconds")]` | `1795428000`, whole seconds since the epoch |

use std::fmt;

use ::serde::de::{self, Visitor};
use ::serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{DateTime, NaiveDate, NaiveDateTime, Utc};

/// A visitor for a string that `parse` turns into a `T`.
struct StrVisitor<T> {
    expecting: &'static str,
    parse: fn(&str) -> Result<T, crate::ParseError>,
}

impl<T> Visitor<'_> for StrVisitor<T> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.expecting)
    }

    fn visit_str<E: de::Error>(self, s: &str) -> Result<T, E> {
        (self.parse)(s).map_err(E::custom)
    }
}

impl Serialize for DateTime<Utc> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&format_args!("{:?}", self))
    }
}

impl<'de> Deserialize<'de> for DateTime<Utc> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(StrVisitor {
            expecting: "an RFC 3339 timestamp, such as \"2026-11-23T10:00:00Z\"",
            parse: crate::parse::parse_rfc3339,
        })
    }
}

impl Serialize for NaiveDate {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for NaiveDate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(StrVisitor {
            expecting: "a date, such as \"2026-11-23\"",
            parse: crate::parse::parse_date,
        })
    }
}

impl Serialize for NaiveDateTime {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&format_args!("{:?}", self))
    }
}

impl<'de> Deserialize<'de> for NaiveDateTime {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(StrVisitor {
            expecting: "a date and time, such as \"2026-11-23T10:00:00\"",
            parse: crate::parse::parse_naive_datetime,
        })
    }
}

/// A `DateTime<Utc>` as whole seconds since 1970-01-01T00:00:00Z, for
/// `#[serde(with = "bradshaw::serde::ts_seconds")]`. Any fraction of a second
/// is dropped when serialising.
pub mod ts_seconds {
    use super::*;

    pub fn serialize<S: Serializer>(
        time: &DateTime<Utc>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(time.timestamp())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<DateTime<Utc>, D::Error> {
        deserializer.deserialize_i64(SecondsVisitor)
    }

    struct SecondsVisitor;

    impl Visitor<'_> for SecondsVisitor {
        type Value = DateTime<Utc>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("whole seconds since 1970-01-01T00:00:00Z")
        }

        fn visit_i64<E: de::Error>(self, secs: i64) -> Result<DateTime<Utc>, E> {
            DateTime::from_timestamp(secs, 0)
                .ok_or_else(|| E::custom(format!("timestamp {} is out of range", secs)))
        }

        fn visit_u64<E: de::Error>(self, secs: u64) -> Result<DateTime<Utc>, E> {
            i64::try_from(secs)
                .ok()
                .and_then(|secs| DateTime::from_timestamp(secs, 0))
                .ok_or_else(|| E::custom(format!("timestamp {} is out of range", secs)))
        }
    }
}
