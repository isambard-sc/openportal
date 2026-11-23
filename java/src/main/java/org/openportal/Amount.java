// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * A {@link Usage} together with the unit it is in, so it can be printed and
 * read as what it is: {@code "12.340000 GBP"}, {@code "1.500 GPUHR"}.
 *
 * <p>{@link #toString} shows the unit's full resolution, exactly when that is a
 * power of ten; {@link #format(int)} rounds to as many places as you ask for.
 */
public record Amount(Usage usage, UsageUnit unit) {

    public Amount {
        if (usage == null || unit == null) {
            throw new IllegalArgumentException("an amount needs a usage and a unit");
        }
    }

    /** The amount as a number of the unit. */
    public double value() {
        return unit.amount(usage);
    }

    /** {@code "12.34 GBP"} for {@code format(2)}. */
    public String format(int decimals) {
        return Fmt.fixed(value(), decimals) + " " + unit.name();
    }

    @Override
    public String toString() {
        int decimals = unit.defaultDecimals();
        long perUnit = unit.countsPerUnit();

        // Exact when the resolution is a power of ten: whole units and the
        // remainder, with no floating point in between.
        if (perUnit == (long) Math.pow(10, decimals)) {
            long counts = usage.counts();
            long whole = counts / perUnit;

            if (decimals == 0) {
                return whole + " " + unit.name();
            }

            String fraction = String.format("%0" + decimals + "d", counts % perUnit);

            return whole + "." + fraction + " " + unit.name();
        }

        return format(decimals);
    }
}
