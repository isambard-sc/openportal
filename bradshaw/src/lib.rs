// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! # bradshaw
//!
//! UTC dates, times and durations for OpenPortal, on the standard library
//! alone. Named after Bradshaw's railway timetables.
//!
//! OpenPortal used `chrono`, which is no longer maintained. Rather than move
//! its 30,000-odd lines into the tree, or take on another date library, this
//! is the small part of one that OpenPortal uses - so that nothing an agent
//! runs on depends on a date library whose next release nobody is reviewing.
//!
//! - **The API follows chrono's names** (`DateTime<Utc>`, `NaiveDate`,
//!   `TimeDelta`, `Datelike`, `Utc::now()`, ...), so code reads as it did.
//! - **UTC only.** There are no other time zones and no `Local`: an agent's
//!   dates never depend on the time zone of the host it runs on.
//! - **Serialised exactly as chrono serialised** - RFC 3339 timestamps,
//!   `YYYY-MM-DD` dates - checked against vectors recorded from chrono (see
//!   `tests/golden.rs`), so agents of different versions understand each other.
//! - **Arithmetic saturates** instead of panicking, since release builds abort
//!   on a panic and times and durations arrive from peers. The `checked_`
//!   methods report the edge instead.
//! - **The proleptic Gregorian calendar**, from year -262143 to 262142 (as in
//!   chrono), with no leap seconds.
//!
//! Optional features: `pyo3` (Python `datetime` conversions), `stub-gen`
//! (type information for `.pyi` stubs) and `ts-rs` (TypeScript bindings).

mod civil;
mod date;
mod datetime;
mod delta;
pub mod format;
mod naive;
pub mod parse;
pub mod serde;
mod time;

#[cfg(feature = "pyo3")]
mod python;

#[cfg(feature = "ts-rs")]
mod typescript;

pub use date::{Datelike, NaiveDate, Weekday};
pub use datetime::{DateTime, Utc};
pub use delta::{Duration, OutOfRangeError, TimeDelta};
pub use format::DelayedFormat;
pub use naive::NaiveDateTime;
pub use parse::ParseError;
pub use time::{NaiveTime, Timelike};

// used only by the integration tests in `tests/`
#[cfg(test)]
use serde_json as _;
