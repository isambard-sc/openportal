// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! The proleptic Gregorian calendar, as days since 1970-01-01.
//!
//! These are Howard Hinnant's `days_from_civil` and `civil_from_days`
//! (<https://howardhinnant.github.io/date_algorithms.html>): exact for every
//! date, with no tables and no loops. They work in 400-year eras, the period
//! after which the Gregorian calendar repeats, and count years from March so
//! that the leap day falls at the end of the year.
//!
//! Callers keep years within `NaiveDate::MIN`..=`NaiveDate::MAX` (about
//! ±262,000 years), so none of the arithmetic here can overflow an `i64`.

/// Days since 1970-01-01 of `year`-`month`-`day`. The caller has validated
/// that `month` is 1..=12 and `day` is within it.
pub(crate) const fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_from_march = ((month + 9) % 12) as i64;
    let day_of_year = (153 * month_from_march + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The `(year, month, day)` that is `days` after 1970-01-01.
pub(crate) const fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_from_march + 2) / 5 + 1) as u32;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    } as u32;
    let year = year_of_era + era * 400;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

pub(crate) const fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The number of days in `month` (1..=12) of `year`; 0 for an invalid month.
pub(crate) const fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_trip_every_day_for_two_eras() {
        // 800 years either side of the epoch, every single day
        let start = days_from_civil(1170, 1, 1);
        let end = days_from_civil(2770, 12, 31);

        let mut expected = (1170_i64, 1_u32, 1_u32);

        for days in start..=end {
            assert_eq!(civil_from_days(days), expected, "day {}", days);
            assert_eq!(
                days_from_civil(expected.0, expected.1, expected.2),
                days,
                "{:?}",
                expected
            );

            // the next day, counted by hand
            let (y, m, d) = expected;
            expected = if d < days_in_month(y, m) {
                (y, m, d + 1)
            } else if m < 12 {
                (y, m + 1, 1)
            } else {
                (y + 1, 1, 1)
            };
        }
    }

    #[test]
    fn test_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(1, 1, 1), -719_162);
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert!(is_leap_year(2000) && is_leap_year(2024) && is_leap_year(-4));
        assert!(!is_leap_year(1900) && !is_leap_year(2100) && !is_leap_year(2026));
    }
}
