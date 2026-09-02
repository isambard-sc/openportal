// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import java.math.BigDecimal;
import java.util.Locale;

/**
 * Number formatting that agrees with the Rust side's.
 *
 * <p>This exists because {@code Double.toString} and Rust's {@code {}} disagree
 * about the two cases that matter most here. An allocation's size goes on the
 * wire <b>inside a string</b> ({@code "5000 GPUHR"}), so if Java writes
 * {@code "5000.0 GPUHR"} the value survives a round trip but no longer compares
 * equal to what the awarding portal sent - and an allocation is matched by
 * string in more places than it should be.
 */
final class Fmt {

    private Fmt() {}

    /**
     * A {@code double} as Rust's {@code {}} writes it.
     *
     * <p>Two differences from {@code Double.toString}: a value with no
     * fractional part has no {@code ".0"} suffix, and there is never an
     * exponent - Rust's {@code Display} does not switch to scientific notation
     * the way Java does at 10⁷.
     */
    static String number(double value) {
        if (Double.isNaN(value)) {
            return "NaN";
        }

        if (Double.isInfinite(value)) {
            return value > 0 ? "inf" : "-inf";
        }

        if (value == 0.0) {
            // Keeps the sign of negative zero, as Rust does.
            return (1 / value < 0) ? "-0" : "0";
        }

        // Via `Double.toString`, so the digits are the shortest that round-trip
        // - the same choice Rust makes. `stripTrailingZeros` then drops the
        // ".0", and `toPlainString` expands any exponent.
        return new BigDecimal(Double.toString(value)).stripTrailingZeros().toPlainString();
    }

    /** A {@code double} to a fixed number of decimal places, as {@code {:.N}}. */
    static String fixed(double value, int places) {
        return String.format(Locale.ROOT, "%." + places + "f", value);
    }
}
