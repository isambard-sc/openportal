// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.Locale;

/**
 * A quantity of storage in bytes.
 *
 * <p>The units are <b>binary</b> throughout - a KB is 1024 bytes, a GB is
 * 1024³ - despite the decimal-looking names. That is what the Rust side does,
 * and a site that reports 1000-based figures under the same names is
 * understating every quota by 7% at TB scale.
 *
 * <p>On the wire this is a human-readable <b>string</b>, {@code "2.00 TB"} -
 * which means the round trip is lossy: the string keeps two decimal places, so
 * a byte count that is not a round number of its own display unit does not come
 * back the same. Compare {@link #bytes}, never the strings. A bare number is
 * accepted when reading, and taken as bytes.
 */
public record StorageSize(long bytes) implements OpenPortalType {

    public static final long KB = 1024L;
    public static final long MB = 1024L * KB;
    public static final long GB = 1024L * MB;
    public static final long TB = 1024L * GB;
    public static final long PB = 1024L * TB;

    public static final StorageSize ZERO = new StorageSize(0);

    public StorageSize {
        if (bytes < 0) {
            throw new IllegalArgumentException("storage size cannot be negative: " + bytes);
        }
    }

    public static StorageSize fromBytes(long bytes) {
        return new StorageSize(bytes);
    }

    public static StorageSize fromKilobytes(double kb) {
        return fromScaled(kb, KB);
    }

    public static StorageSize fromMegabytes(double mb) {
        return fromScaled(mb, MB);
    }

    public static StorageSize fromGigabytes(double gb) {
        return fromScaled(gb, GB);
    }

    public static StorageSize fromTerabytes(double tb) {
        return fromScaled(tb, TB);
    }

    public static StorageSize fromPetabytes(double pb) {
        return fromScaled(pb, PB);
    }

    /**
     * Parse {@code "2TB"}, {@code "2 TB"}, {@code "500 gigabytes"} or a bare
     * byte count.
     *
     * <p>Case-insensitive, and whitespace between the number and the unit is
     * optional. The unit names are the {@code B}/{@code KB}/.../{@code PB}
     * abbreviations and their {@code bytes}/{@code kilobytes}/... spellings.
     *
     * <p>A unit is <b>required</b>: {@code "100"} is rejected, not read as 100
     * bytes. Only the JSON reader accepts a bare number, and only because a
     * number arriving there is unambiguous.
     */
    public static StorageSize parse(String value) {
        if (value == null || value.trim().isEmpty()) {
            throw new IllegalArgumentException("Failed to parse a storage size from '" + value + "'");
        }

        String upper = value.trim().toUpperCase(Locale.ROOT);
        StringBuilder digits = new StringBuilder();
        StringBuilder unit = new StringBuilder();

        // Partitioned character by character rather than split on whitespace,
        // which is how the Rust side does it - and is why "2TB" and "2 TB" are
        // both accepted, and why "1.5.2GB" fails as a number rather than as a
        // unit.
        for (int i = 0; i < upper.length(); i++) {
            char c = upper.charAt(i);

            if ((c >= '0' && c <= '9') || c == '.') {
                digits.append(c);
            } else {
                unit.append(c);
            }
        }

        double number;

        try {
            number = Double.parseDouble(digits.toString());
        } catch (NumberFormatException e) {
            throw new IllegalArgumentException(
                    "Failed to parse '" + digits + "' as a number");
        }

        String name = unit.toString().trim();

        double multiplier = switch (name) {
            case "B", "BYTES" -> 1.0;
            case "KB", "KILOBYTES" -> (double) KB;
            case "MB", "MEGABYTES" -> (double) MB;
            case "GB", "GIGABYTES" -> (double) GB;
            case "TB", "TERABYTES" -> (double) TB;
            case "PB", "PETABYTES" -> (double) PB;
            default -> throw new IllegalArgumentException("Unknown unit '" + name + "'");
        };

        return fromScaled(number, (long) multiplier);
    }

    public double kilobytes() {
        return bytes / (double) KB;
    }

    public double megabytes() {
        return bytes / (double) MB;
    }

    public double gigabytes() {
        return bytes / (double) GB;
    }

    public double terabytes() {
        return bytes / (double) TB;
    }

    public double petabytes() {
        return bytes / (double) PB;
    }

    public boolean isZero() {
        return bytes == 0;
    }

    public StorageSize plus(StorageSize other) {
        return new StorageSize(Usage.saturatingAdd(bytes, other.bytes));
    }

    /** Subtraction, clamped at zero. */
    public StorageSize minus(StorageSize other) {
        return new StorageSize(Math.max(0, bytes - other.bytes));
    }

    public StorageSize times(long factor) {
        return new StorageSize(Usage.saturatingMultiply(bytes, Math.max(0, factor)));
    }

    /** Division, answering zero for a zero divisor rather than throwing. */
    public StorageSize dividedBy(long divisor) {
        if (divisor <= 0) {
            return ZERO;
        }

        return new StorageSize(bytes / divisor);
    }

    @Override
    public String typeName() {
        return "StorageSize";
    }

    @Override
    public JsonNode toJson() {
        return Json.text(toString());
    }

    public static StorageSize fromJson(JsonNode node) {
        if (node == null || node.isNull()) {
            return ZERO;
        }

        if (node.isNumber()) {
            return new StorageSize(node.asLong());
        }

        return parse(node.asText());
    }

    /**
     * The largest unit the value reaches, to two places.
     *
     * <p>Note the boundaries are inclusive of the lower unit: exactly 1024
     * bytes prints as {@code "1024 B"}, and 1025 as {@code "1.00 KB"}.
     */
    @Override
    public String toString() {
        if (bytes <= KB) {
            return bytes + " B";
        }

        if (bytes <= MB) {
            return Fmt.fixed(kilobytes(), 2) + " KB";
        }

        if (bytes <= GB) {
            return Fmt.fixed(megabytes(), 2) + " MB";
        }

        if (bytes <= TB) {
            return Fmt.fixed(gigabytes(), 2) + " GB";
        }

        if (bytes <= PB) {
            return Fmt.fixed(terabytes(), 2) + " TB";
        }

        return Fmt.fixed(petabytes(), 2) + " PB";
    }

    private static StorageSize fromScaled(double amount, long multiplier) {
        double value = amount * multiplier;

        if (Double.isNaN(value) || value <= 0.0) {
            return ZERO;
        }

        if (value >= (double) Long.MAX_VALUE) {
            return new StorageSize(Long.MAX_VALUE);
        }

        return new StorageSize((long) value);
    }
}
