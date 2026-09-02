// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;

/**
 * An amount of resource consumed, held as whole seconds.
 *
 * <p>Seconds of <i>what</i> is not recorded here, and that is the trap. A usage
 * figure is only meaningful alongside the unit its report is in - a site
 * accounts in its own unit and converts to the award's on the way out (see
 * {@code site-portal-api.md} §4.3). This type is the number; the unit lives on
 * the award.
 *
 * <p>Every operation saturates rather than wrapping or throwing. That is not
 * defensiveness for its own sake: these values arrive from a peer, release
 * builds have {@code overflow-checks} on and {@code panic = "abort"}, so on the
 * Rust side an overflow is a process kill. Subtraction clamps at zero for the
 * same reason - there is no negative usage.
 *
 * <p>On the wire this is an object, {@code {"seconds": 7200}}, not a bare
 * number.
 */
public record Usage(long seconds) implements OpenPortalType {

    private static final double SECONDS_PER_MINUTE = 60.0;
    private static final double SECONDS_PER_HOUR = 3600.0;
    private static final double SECONDS_PER_DAY = 86400.0;
    private static final double SECONDS_PER_WEEK = 604800.0;

    /** A month is 2628000 seconds - 1/12 of the year below, not a calendar month. */
    private static final double SECONDS_PER_MONTH = 2628000.0;

    /** A year is 365 days exactly. */
    private static final double SECONDS_PER_YEAR = 31536000.0;

    public static final Usage ZERO = new Usage(0);

    public Usage {
        if (seconds < 0) {
            throw new IllegalArgumentException("Usage cannot be negative: " + seconds);
        }
    }

    public static Usage fromSeconds(long seconds) {
        return new Usage(seconds);
    }

    public static Usage fromMinutes(double minutes) {
        return fromScaled(minutes, SECONDS_PER_MINUTE);
    }

    public static Usage fromHours(double hours) {
        return fromScaled(hours, SECONDS_PER_HOUR);
    }

    public static Usage fromDays(double days) {
        return fromScaled(days, SECONDS_PER_DAY);
    }

    public static Usage fromWeeks(double weeks) {
        return fromScaled(weeks, SECONDS_PER_WEEK);
    }

    public static Usage fromMonths(double months) {
        return fromScaled(months, SECONDS_PER_MONTH);
    }

    public static Usage fromYears(double years) {
        return fromScaled(years, SECONDS_PER_YEAR);
    }

    /**
     * Parse the {@code "<count> [unit]"} form, as a Slurm limit is written.
     *
     * <p>Units are seconds, minutes, hours or days (and their {@code s}/{@code m}/
     * {@code h}/{@code d} abbreviations); no unit means seconds. Deliberately
     * <i>not</i> the {@link #toString} form, which goes as far as years - this
     * mirrors {@code Usage::parse} on the Rust side, which reads configuration
     * rather than its own output.
     */
    public static Usage parse(String value) {
        if (value == null || value.isBlank()) {
            throw new IllegalArgumentException("Failed to parse a duration from '" + value + "'");
        }

        String[] parts = value.trim().split("\\s+");
        long units = 1;

        if (parts.length > 1) {
            units = switch (parts[1].toLowerCase(java.util.Locale.ROOT)) {
                case "seconds", "second", "s" -> 1L;
                case "minutes", "minute", "m" -> 60L;
                case "hours", "hour", "h" -> 3600L;
                case "days", "day", "d" -> 86400L;
                default -> throw new IllegalArgumentException("Failed to parse '" + value
                        + "'. Units should be seconds, minutes, hours or days");
            };
        }

        long count;

        try {
            count = Long.parseLong(parts[0]);
        } catch (NumberFormatException e) {
            throw new IllegalArgumentException("Failed to parse seconds from '" + value + "'");
        }

        if (count < 0) {
            throw new IllegalArgumentException("Usage cannot be negative: '" + value + "'");
        }

        return new Usage(saturatingMultiply(count, units));
    }

    public boolean isZero() {
        return seconds == 0;
    }

    public double minutes() {
        return seconds / SECONDS_PER_MINUTE;
    }

    public double hours() {
        return seconds / SECONDS_PER_HOUR;
    }

    public double days() {
        return seconds / SECONDS_PER_DAY;
    }

    public double weeks() {
        return seconds / SECONDS_PER_WEEK;
    }

    public double months() {
        return seconds / SECONDS_PER_MONTH;
    }

    public double years() {
        return seconds / SECONDS_PER_YEAR;
    }

    public Usage plus(Usage other) {
        return new Usage(saturatingAdd(seconds, other.seconds));
    }

    /** Subtraction, clamped at zero - there is no negative usage. */
    public Usage minus(Usage other) {
        return new Usage(Math.max(0, seconds - other.seconds));
    }

    public Usage times(double factor) {
        return fromTruncated(seconds * factor);
    }

    /** Division, answering zero for a zero divisor rather than throwing. */
    public Usage dividedBy(double divisor) {
        if (divisor == 0.0) {
            return ZERO;
        }

        return fromTruncated(seconds / divisor);
    }

    /** Always in hours, to three places - {@code "2.500 hours"}. */
    public String inHours() {
        return Fmt.fixed(hours(), 3) + " hours";
    }

    @Override
    public String typeName() {
        return "Usage";
    }

    @Override
    public JsonNode toJson() {
        return Json.object().put("seconds", seconds);
    }

    public static Usage fromJson(JsonNode node) {
        if (node == null || node.isNull()) {
            return ZERO;
        }

        // A bare number is accepted as well as the object form: a hand-written
        // report is the likeliest source of one, and reading it is free.
        if (node.isNumber()) {
            return new Usage(node.asLong());
        }

        return new Usage(node.path("seconds").asLong());
    }

    /**
     * The largest unit the value reaches, to three places.
     *
     * <p>Scales up through seconds, minutes, hours, days, weeks, months and
     * years - so the same value prints differently as it grows, which is fine
     * for a log line and wrong for anything that compares strings. Use
     * {@link #inHours} for a stable rendering, and {@link #seconds} to compare.
     */
    @Override
    public String toString() {
        if (seconds < 60) {
            return seconds + (seconds == 1 ? " second" : " seconds");
        }

        if (minutes() < 60.0) {
            return unit(minutes(), "minute", "minutes");
        }

        if (hours() < 24.0) {
            return unit(hours(), "hour", "hours");
        }

        if (days() < 7.0) {
            return unit(days(), "day", "days");
        }

        if (weeks() < 4.5) {
            return unit(weeks(), "week", "weeks");
        }

        if (months() < 12.0) {
            return unit(months(), "month", "months");
        }

        return unit(years(), "year", "years");
    }

    /** Singular when the value rounds to 1.000 at three places, as Rust does. */
    private static String unit(double value, String singular, String plural) {
        String name = Math.abs(value - 1.0) < 0.0005 ? singular : plural;

        return Fmt.fixed(value, 3) + " " + name;
    }

    /** Negative is treated as zero, matching {@code Usage::from_hours} and friends. */
    private static Usage fromScaled(double amount, double secondsPerUnit) {
        if (amount < 0.0 || Double.isNaN(amount)) {
            return ZERO;
        }

        return fromTruncated(amount * secondsPerUnit);
    }

    /**
     * A {@code double} to whole seconds, truncated then clamped.
     *
     * <p>Truncated rather than rounded, because {@code as u64} in Rust
     * truncates - and clamped because it also saturates rather than wrapping.
     */
    private static Usage fromTruncated(double value) {
        if (Double.isNaN(value) || value <= 0.0) {
            return ZERO;
        }

        if (value >= (double) Long.MAX_VALUE) {
            return new Usage(Long.MAX_VALUE);
        }

        return new Usage((long) value);
    }

    static long saturatingAdd(long a, long b) {
        long sum = a + b;

        // Overflow of two non-negative values shows up as a negative sum.
        return sum < 0 ? Long.MAX_VALUE : sum;
    }

    static long saturatingMultiply(long a, long b) {
        try {
            return Math.multiplyExact(a, b);
        } catch (ArithmeticException e) {
            return Long.MAX_VALUE;
        }
    }
}
