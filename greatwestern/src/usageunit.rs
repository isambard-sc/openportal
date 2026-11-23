// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! What a count of [`Usage`] means.
//!
//! A [`Usage`] is a bare integer count. On the wire it is still written as
//! `{"seconds": n}`, because renaming the field would break every existing
//! reader, but nothing about it has to be time: a [`UsageUnit`] says what one
//! count is. `{"name": "GBP", "counts_per_unit": 1000000}` makes a count one
//! micro-pound; `{"name": "NHR", "counts_per_unit": 3600}` makes it one
//! node-second, which is what usage has always meant on an HPC site.
//!
//! Accounting stays in integers: an amount is turned into counts once, rounded
//! to the nearest count, and summed exactly from then on. See
//! `docs/plans/usage-units-design.md`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use templemeads::named::NamedType;
use templemeads::Error;

use crate::grammar::Allocation;
use crate::usagereport::Usage;

/// The longest unit name accepted, in characters.
const MAX_NAME_LEN: usize = 64;

/// The default resolution of a time unit: a count is a unit-second, as usage
/// has always been reported.
pub const COUNTS_PER_HOUR: u64 = 3600;

/// The default resolution of a currency: a count is a micro-unit.
pub const COUNTS_PER_CURRENCY_UNIT: u64 = 1_000_000;

/// The time units OpenPortal knows, as `Allocation` canonicalises them. Each
/// is a resource-hour, so its default resolution is [`COUNTS_PER_HOUR`].
const TIME_UNITS: &[&str] = &["NHR", "GPUHR", "CPUHR", "COREHR", "GBHR", "MBHR", "BHR"];

/// Every current ISO 4217 alphabetic currency code, less `XTS` (reserved for
/// testing) and `XXX` (no currency). A currency is named by its code, so that
/// `"GBP"` and `"gbp"` are one unit and `"Pounds"` is an error rather than a
/// guess. `$` is deliberately not accepted - it names a dozen currencies.
const CURRENCY_CODES: &[&str] = &[
    "AED", "AFN", "ALL", "AMD", "ANG", "AOA", "ARS", "AUD", "AWG", "AZN", "BAM", "BBD", "BDT",
    "BGN", "BHD", "BIF", "BMD", "BND", "BOB", "BOV", "BRL", "BSD", "BTN", "BWP", "BYN", "BZD",
    "CAD", "CDF", "CHE", "CHF", "CHW", "CLF", "CLP", "CNY", "COP", "COU", "CRC", "CUC", "CUP",
    "CVE", "CZK", "DJF", "DKK", "DOP", "DZD", "EGP", "ERN", "ETB", "EUR", "FJD", "FKP", "GBP",
    "GEL", "GHS", "GIP", "GMD", "GNF", "GTQ", "GYD", "HKD", "HNL", "HTG", "HUF", "IDR", "ILS",
    "INR", "IQD", "IRR", "ISK", "JMD", "JOD", "JPY", "KES", "KGS", "KHR", "KMF", "KPW", "KRW",
    "KWD", "KYD", "KZT", "LAK", "LBP", "LKR", "LRD", "LSL", "LYD", "MAD", "MDL", "MGA", "MKD",
    "MMK", "MNT", "MOP", "MRU", "MUR", "MVR", "MWK", "MXN", "MXV", "MYR", "MZN", "NAD", "NGN",
    "NIO", "NOK", "NPR", "NZD", "OMR", "PAB", "PEN", "PGK", "PHP", "PKR", "PLN", "PYG", "QAR",
    "RON", "RSD", "RUB", "RWF", "SAR", "SBD", "SCR", "SDG", "SEK", "SGD", "SHP", "SLE", "SLL",
    "SOS", "SRD", "SSP", "STN", "SVC", "SYP", "SZL", "THB", "TJS", "TMT", "TND", "TOP", "TRY",
    "TTD", "TWD", "TZS", "UAH", "UGX", "USD", "USN", "UYI", "UYU", "UYW", "UZS", "VED", "VES",
    "VND", "VUV", "WST", "XAF", "XAG", "XAU", "XBA", "XBB", "XBC", "XBD", "XCD", "XCG", "XDR",
    "XOF", "XPD", "XPF", "XPT", "XSU", "XUA", "YER", "ZAR", "ZMW", "ZWG", "ZWL",
];

/// The currency a symbol names, for the two symbols that name only one.
fn currency_for_symbol(symbol: &str) -> Option<&'static str> {
    match symbol {
        "£" => Some("GBP"),
        "€" => Some("EUR"),
        _ => None,
    }
}

/// The ISO 4217 code `name` spells, if it spells one: `"gbp"`, `"GBP"` and
/// `"£"` all give `"GBP"`.
pub(crate) fn currency_code(name: &str) -> Option<&'static str> {
    let name = name.trim();

    if let Some(code) = currency_for_symbol(name) {
        return Some(code);
    }

    let upper = name.to_ascii_uppercase();

    CURRENCY_CODES.iter().copied().find(|code| *code == upper)
}

/// True if `name`, canonicalised, is one of the time units OpenPortal knows.
pub fn is_time_unit(name: &str) -> bool {
    let canonical = Allocation::canonicalize(name);
    TIME_UNITS.contains(&canonical.as_str())
}

/// True if `name` is an ISO 4217 currency code or `£` / `€`.
pub fn is_currency(name: &str) -> bool {
    currency_code(name).is_some()
}

///
/// What one count of a [`Usage`] means: `counts_per_unit` counts make one
/// `name`.
///
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(try_from = "UncheckedUsageUnit")]
#[ts(export)]
pub struct UsageUnit {
    /// The unit's name, canonicalised as `Allocation` units are: `"NHR"`,
    /// `"GPUHR"`, `"GBP"`, or a name two portals have agreed
    name: String,

    /// How many counts make one of the unit. 3600 for a resource-hour (a count
    /// is a resource-second), 1000000 for a currency (a count is a micro-unit)
    counts_per_unit: u64,
}

#[derive(Deserialize)]
struct UncheckedUsageUnit {
    name: String,
    counts_per_unit: u64,
}

impl TryFrom<UncheckedUsageUnit> for UsageUnit {
    type Error = Error;

    fn try_from(unit: UncheckedUsageUnit) -> Result<Self, Self::Error> {
        UsageUnit::new(&unit.name, unit.counts_per_unit)
    }
}

impl NamedType for UsageUnit {
    fn type_name() -> String {
        "UsageUnit".to_string()
    }
}

impl std::fmt::Display for UsageUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({} per unit)", self.name, self.counts_per_unit)
    }
}

impl UsageUnit {
    ///
    /// A well-known unit, at its default resolution:
    ///
    /// * a time unit (`NHR`, `GPUHR`, `CPUHR`, `COREHR`, `GBHR`, `MBHR`, `BHR`,
    ///   in any spelling `Allocation` accepts) at 3600 counts per hour;
    /// * a currency (an ISO 4217 code, or `£` / `€`) at 1000000 counts per
    ///   unit.
    ///
    /// Anything else is an error: use [`UsageUnit::new`] for a unit two
    /// portals have agreed, so that its resolution is stated rather than
    /// guessed.
    ///
    pub fn parse(name: &str) -> Result<Self, Error> {
        if let Some(code) = currency_code(name) {
            return Self::new(code, COUNTS_PER_CURRENCY_UNIT);
        }

        if is_time_unit(name) {
            return Self::new(name, COUNTS_PER_HOUR);
        }

        Err(Error::Parse(format!(
            "Unknown usage unit '{}'. Use a time unit (NHR, GPUHR, CPUHR, COREHR, GBHR, \
             MBHR, BHR), an ISO 4217 currency code (e.g. GBP, USD, EUR), or give the \
             counts per unit explicitly for a unit of your own",
            name
        )))
    }

    ///
    /// Any unit, at any resolution: `UsageUnit::new("CREDITS", 100)`. The name
    /// is canonicalised as `Allocation` units are, so it can be checked against
    /// an award's allocation.
    ///
    pub fn new(name: &str, counts_per_unit: u64) -> Result<Self, Error> {
        let name = Allocation::canonicalize(name);

        if name.is_empty() {
            return Err(Error::Parse(
                "Invalid usage unit - the name cannot be empty".to_string(),
            ));
        }

        if name.chars().count() > MAX_NAME_LEN {
            return Err(Error::Parse(format!(
                "Invalid usage unit - the name is longer than {} characters '{}'",
                MAX_NAME_LEN, name
            )));
        }

        if name.chars().any(|c| c.is_control()) {
            return Err(Error::Parse(format!(
                "Invalid usage unit - the name contains a control character '{}'",
                name.escape_debug()
            )));
        }

        if counts_per_unit == 0 {
            return Err(Error::Parse(format!(
                "Invalid usage unit '{}' - counts_per_unit must be at least 1",
                name
            )));
        }

        Ok(Self {
            name,
            counts_per_unit,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn counts_per_unit(&self) -> u64 {
        self.counts_per_unit
    }

    /// True if this is one of the time units OpenPortal knows.
    pub fn is_time(&self) -> bool {
        TIME_UNITS.contains(&self.name.as_str())
    }

    /// True if this is a currency.
    pub fn is_currency(&self) -> bool {
        CURRENCY_CODES.contains(&self.name.as_str())
    }

    ///
    /// An amount of this unit as a [`Usage`], rounded to the nearest count
    /// (ties to even, so that summing many conversions carries no bias).
    ///
    /// A negative, `NaN` or infinite amount, or one too large to count, is an
    /// error - never a saturated or truncated figure. For an exact amount, such
    /// as money, prefer [`UsageUnit::usage_from_decimal`].
    ///
    pub fn usage(&self, amount: f64) -> Result<Usage, Error> {
        if !amount.is_finite() {
            return Err(Error::Parse(format!(
                "Invalid amount of {} - it must be a finite number, not '{}'",
                self.name, amount
            )));
        }

        if amount < 0.0 {
            return Err(Error::Parse(format!(
                "Invalid amount of {} - it cannot be negative ({})",
                self.name, amount
            )));
        }

        let counts = (amount * self.counts_per_unit as f64).round_ties_even();

        // `u64::MAX as f64` rounds up to 2^64, so anything at or above it does
        // not fit - and a cast would quietly saturate rather than say so.
        if counts >= u64::MAX as f64 {
            return Err(Error::Parse(format!(
                "Invalid amount of {} - {} is too large to count",
                self.name, amount
            )));
        }

        Ok(Usage::from_counts(counts as u64))
    }

    ///
    /// An exact decimal amount of this unit as a [`Usage`], with no floating
    /// point: `"12.34"` of GBP is exactly 12340000 counts.
    ///
    /// The amount is plain digits with an optional decimal point - no sign, no
    /// exponent. An amount finer than the unit resolves (`"0.0000001"` of GBP)
    /// is an error rather than a rounded figure, since the point of this route
    /// is that nothing is rounded.
    ///
    pub fn usage_from_decimal(&self, amount: &str) -> Result<Usage, Error> {
        let text = amount.trim();

        if text.starts_with('-') {
            return Err(Error::Parse(format!(
                "Invalid amount of {} '{}' - it cannot be negative",
                self.name, amount
            )));
        }

        let (whole, fraction) = match text.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (text, ""),
        };

        if (whole.is_empty() && fraction.is_empty())
            || !whole.chars().all(|c| c.is_ascii_digit())
            || !fraction.chars().all(|c| c.is_ascii_digit())
        {
            return Err(Error::Parse(format!(
                "Invalid amount of {} '{}' - it must be digits with an optional decimal \
                 point, such as '12.34'",
                self.name, amount
            )));
        }

        // Trailing zeros carry no value, and dropping them keeps the
        // arithmetic below inside u128 however many the caller wrote.
        let fraction = fraction.trim_end_matches('0');
        let whole = whole.trim_start_matches('0');

        let too_large = || {
            Error::Parse(format!(
                "Invalid amount of {} '{}' - it is too large to count",
                self.name, amount
            ))
        };

        if whole.len() > 20 || fraction.len() > 30 {
            return Err(too_large());
        }

        let whole_value: u128 = if whole.is_empty() {
            0
        } else {
            whole.parse::<u128>().map_err(|_| too_large())?
        };

        let fraction_value: u128 = if fraction.is_empty() {
            0
        } else {
            fraction.parse::<u128>().map_err(|_| too_large())?
        };

        let per_unit = u128::from(self.counts_per_unit);
        let scale = 10u128
            .checked_pow(fraction.len() as u32)
            .ok_or_else(too_large)?;

        let fraction_counts = fraction_value.checked_mul(per_unit).ok_or_else(too_large)?;

        if fraction_counts % scale != 0 {
            return Err(Error::Parse(format!(
                "Invalid amount of {} '{}' - it is finer than one count ({} per {})",
                self.name, amount, self.counts_per_unit, self.name
            )));
        }

        let counts = whole_value
            .checked_mul(per_unit)
            .and_then(|c| c.checked_add(fraction_counts / scale))
            .ok_or_else(too_large)?;

        let counts = u64::try_from(counts).map_err(|_| too_large())?;

        Ok(Usage::from_counts(counts))
    }

    /// The amount of this unit a [`Usage`] holds, for display and comparison.
    pub fn amount(&self, usage: Usage) -> f64 {
        usage.counts() as f64 / self.counts_per_unit as f64
    }

    /// The default number of decimal places to print an amount with: exactly
    /// the unit's resolution when that is a power of ten (6 for a currency),
    /// otherwise 3.
    pub fn default_decimals(&self) -> usize {
        let mut per_unit = self.counts_per_unit;
        let mut decimals = 0;

        while per_unit > 1 && per_unit.is_multiple_of(10) {
            per_unit /= 10;
            decimals += 1;
        }

        match per_unit {
            1 => decimals,
            _ => 3,
        }
    }
}

///
/// A [`Usage`] together with the unit it is in, so that it can be printed and
/// read as what it is: `12.340000 GBP`, `1.500 GPUHR`.
///
/// Printing honours a precision - `format!("{:.2}", amount)` gives
/// `12.34 GBP` - and without one shows the unit's full resolution, computed
/// exactly from the counts when that resolution is a power of ten.
///
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Amount {
    usage: Usage,
    unit: UsageUnit,
}

impl Amount {
    pub fn new(usage: Usage, unit: &UsageUnit) -> Self {
        Self {
            usage,
            unit: unit.clone(),
        }
    }

    pub fn usage(&self) -> Usage {
        self.usage
    }

    pub fn unit(&self) -> &UsageUnit {
        &self.unit
    }

    /// The amount as a number of the unit.
    pub fn value(&self) -> f64 {
        self.unit.amount(self.usage)
    }
}

impl std::fmt::Display for Amount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(precision) = f.precision() {
            return write!(f, "{:.*} {}", precision, self.value(), self.unit.name());
        }

        let decimals = self.unit.default_decimals();
        let per_unit = self.unit.counts_per_unit();

        // Exact when the resolution is a power of ten: whole units and the
        // remainder, with no float in between.
        if 10u64.checked_pow(decimals as u32) == Some(per_unit) {
            let counts = self.usage.counts();
            let whole = counts / per_unit;

            return match decimals {
                0 => write!(f, "{} {}", whole, self.unit.name()),
                _ => write!(
                    f,
                    "{}.{:0width$} {}",
                    whole,
                    counts % per_unit,
                    self.unit.name(),
                    width = decimals
                ),
            };
        }

        write!(f, "{:.*} {}", decimals, self.value(), self.unit.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_well_known_units_parse_at_their_default_resolution() {
        #[allow(clippy::unwrap_used)]
        {
            for (name, canonical, per_unit) in [
                ("GBP", "GBP", 1_000_000),
                ("gbp", "GBP", 1_000_000),
                ("£", "GBP", 1_000_000),
                ("€", "EUR", 1_000_000),
                ("usd", "USD", 1_000_000),
                ("JPY", "JPY", 1_000_000),
                ("NHR", "NHR", 3600),
                ("node hours", "NHR", 3600),
                ("GPU hours", "GPUHR", 3600),
                ("cpuhr", "CPUHR", 3600),
                ("MBHR", "MBHR", 3600),
                ("billing hours", "BHR", 3600),
            ] {
                let unit = UsageUnit::parse(name).unwrap();
                assert_eq!(unit.name(), canonical, "{name}");
                assert_eq!(unit.counts_per_unit(), per_unit, "{name}");
            }

            assert!(UsageUnit::parse("GBP").unwrap().is_currency());
            assert!(UsageUnit::parse("GPUHR").unwrap().is_time());
            assert!(!UsageUnit::parse("GPUHR").unwrap().is_currency());
        }
    }

    #[test]
    fn test_unknown_or_ambiguous_units_are_refused_by_parse() {
        for name in ["$", "dollars", "pounds", "credits", "", "XXX", "XTS"] {
            assert!(UsageUnit::parse(name).is_err(), "accepted {name:?}");
        }
    }

    #[test]
    fn test_any_unit_can_be_given_its_own_resolution() {
        #[allow(clippy::unwrap_used)]
        {
            let credits = UsageUnit::new("CREDITS", 100).unwrap();
            assert_eq!(credits.counts_per_unit(), 100);

            let minutes = UsageUnit::new("node hours", 60).unwrap();
            assert_eq!(minutes.name(), "NHR");
            assert_eq!(minutes.counts_per_unit(), 60);

            assert!(UsageUnit::new("GBP", 0).is_err());
            assert!(UsageUnit::new("", 1).is_err());
            assert!(UsageUnit::new(&"x".repeat(65), 1).is_err());
            assert!(UsageUnit::new("bad\nname", 1).is_err());
        }
    }

    #[test]
    fn test_amounts_round_to_the_nearest_count() {
        #[allow(clippy::unwrap_used)]
        {
            let gbp = UsageUnit::parse("GBP").unwrap();
            assert_eq!(gbp.usage(3.2).unwrap().counts(), 3_200_000);
            assert_eq!(gbp.usage(0.1 + 0.2).unwrap().counts(), 300_000);
            assert_eq!(gbp.usage(0.0000005).unwrap().counts(), 0); // tie to even
            assert_eq!(gbp.usage(0.0000015).unwrap().counts(), 2); // tie to even

            let nhr = UsageUnit::parse("NHR").unwrap();
            assert_eq!(nhr.usage(1.5).unwrap().counts(), 5400);

            for bad in [-1.0, f64::NAN, f64::INFINITY, 1e30] {
                assert!(gbp.usage(bad).is_err(), "accepted {bad}");
            }
        }
    }

    #[test]
    fn test_decimal_amounts_are_exact() {
        #[allow(clippy::unwrap_used)]
        {
            let gbp = UsageUnit::parse("GBP").unwrap();
            for (text, counts) in [
                ("12.34", 12_340_000),
                ("3", 3_000_000),
                ("3.", 3_000_000),
                (".5", 500_000),
                ("0.000001", 1),
                ("0012.3400000000000000000000", 12_340_000),
                ("18446744073709.551615", u64::MAX),
            ] {
                assert_eq!(
                    gbp.usage_from_decimal(text).unwrap().counts(),
                    counts,
                    "{text}"
                );
            }

            for bad in [
                "",
                ".",
                "-1",
                "+1",
                "1e3",
                "1,000",
                "abc",
                "0.0000001",
                "18446744073709.551616",
                "99999999999999999999999",
            ] {
                assert!(gbp.usage_from_decimal(bad).is_err(), "accepted {bad:?}");
            }

            // A resolution that is not a power of ten still converts exactly
            // where the fraction resolves to whole counts.
            let nhr = UsageUnit::parse("NHR").unwrap();
            assert_eq!(nhr.usage_from_decimal("1.5").unwrap().counts(), 5400);
            assert_eq!(nhr.usage_from_decimal("0.25").unwrap().counts(), 900);
            assert!(nhr.usage_from_decimal("0.0001").is_err());
        }
    }

    #[test]
    fn test_amounts_print_in_their_unit() {
        #[allow(clippy::unwrap_used)]
        {
            let gbp = UsageUnit::parse("GBP").unwrap();
            let amount = Amount::new(gbp.usage_from_decimal("12.34").unwrap(), &gbp);

            assert_eq!(amount.to_string(), "12.340000 GBP");
            assert_eq!(format!("{:.2}", amount), "12.34 GBP");
            assert_eq!(format!("{:.0}", amount), "12 GBP");
            assert!((amount.value() - 12.34).abs() < 1e-9);

            let gpu = UsageUnit::parse("GPUHR").unwrap();
            let hours = Amount::new(gpu.usage(1.5).unwrap(), &gpu);
            assert_eq!(hours.to_string(), "1.500 GPUHR");
            assert_eq!(format!("{:.1}", hours), "1.5 GPUHR");

            let whole = UsageUnit::new("TOKENS", 1).unwrap();
            assert_eq!(
                Amount::new(Usage::from_counts(42), &whole).to_string(),
                "42 tokens"
            );
        }
    }

    #[test]
    fn test_a_unit_round_trips_and_is_checked_on_the_wire() {
        #[allow(clippy::unwrap_used)]
        {
            let gbp = UsageUnit::parse("GBP").unwrap();
            let json = serde_json::to_string(&gbp).unwrap();
            assert_eq!(json, r#"{"name":"GBP","counts_per_unit":1000000}"#);
            assert_eq!(serde_json::from_str::<UsageUnit>(&json).unwrap(), gbp);

            // Canonicalised on the way in, and checked.
            let read: UsageUnit =
                serde_json::from_str(r#"{"name": "gpu hours", "counts_per_unit": 3600}"#).unwrap();
            assert_eq!(read.name(), "GPUHR");
            assert!(
                serde_json::from_str::<UsageUnit>(r#"{"name": "GBP", "counts_per_unit": 0}"#)
                    .is_err()
            );
        }
    }
}
