// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import java.time.LocalDate;
import java.time.format.DateTimeParseException;

/**
 * Parsing a wire date, with the bounds the wire form is held to.
 *
 * <p>{@code %Y} in a date pattern accepts a signed, unbounded digit count on
 * both sides, so without a bound the whole representable range parses - and a
 * date range's span is what decides how many days a report iterates over. The
 * two limits here are the same ones the Rust side applies.
 */
final class Dates {

    static final int MIN_YEAR = 1970;
    static final int MAX_YEAR = 2200;

    private Dates() {}

    static LocalDate parse(String value) {
        if (value == null || value.trim().isEmpty()) {
            throw new IllegalArgumentException("Invalid Date - cannot be empty");
        }

        LocalDate date;

        try {
            date = LocalDate.parse(value.trim());
        } catch (DateTimeParseException e) {
            throw new IllegalArgumentException(
                    "Invalid Date - date cannot be parsed from '" + value + "'");
        }

        if (date.getYear() < MIN_YEAR || date.getYear() > MAX_YEAR) {
            throw new IllegalArgumentException("Invalid Date - year " + date.getYear()
                    + " is outside the supported range " + MIN_YEAR + "-" + MAX_YEAR
                    + " '" + value + "'");
        }

        return date;
    }
}
