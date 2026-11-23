// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.math.BigDecimal;
import java.math.BigInteger;
import java.math.RoundingMode;
import java.util.Locale;
import java.util.Set;

/**
 * What one count of a {@link Usage} means: {@code countsPerUnit} counts make
 * one {@code name}.
 *
 * <p>A usage is a bare count. {@code UsageUnit.parse("GBP")} makes a count a
 * micro-pound; {@code UsageUnit.parse("NHR")} makes it a node-second, which is
 * what usage has always meant on an HPC site. A {@link ProjectUsageReport}
 * carries its unit, so the awarding portal reads its figures as what they are.
 *
 * <p>On the wire, {@code {"name": "GBP", "counts_per_unit": 1000000}}. Mirrors
 * {@code UsageUnit} in Rust; see {@code docs/plans/usage-units-design.md}.
 */
public record UsageUnit(String name, long countsPerUnit) implements OpenPortalType {

    /** A count of a time unit is a unit-second. */
    public static final long COUNTS_PER_HOUR = 3600;

    /** A count of a currency is a micro-unit. */
    public static final long COUNTS_PER_CURRENCY_UNIT = 1_000_000;

    private static final int MAX_NAME_LENGTH = 64;

    private static final Set<String> TIME_UNITS =
            Set.of("NHR", "GPUHR", "CPUHR", "COREHR", "GBHR", "MBHR", "BHR");

    /**
     * Every current ISO 4217 alphabetic code, less {@code XTS} (testing) and
     * {@code XXX} (no currency). {@code $} is deliberately not accepted - it
     * names a dozen currencies.
     */
    private static final Set<String> CURRENCY_CODES = Set.of(
            "AED", "AFN", "ALL", "AMD", "ANG", "AOA", "ARS", "AUD", "AWG", "AZN", "BAM", "BBD",
            "BDT", "BGN", "BHD", "BIF", "BMD", "BND", "BOB", "BOV", "BRL", "BSD", "BTN", "BWP",
            "BYN", "BZD", "CAD", "CDF", "CHE", "CHF", "CHW", "CLF", "CLP", "CNY", "COP", "COU",
            "CRC", "CUC", "CUP", "CVE", "CZK", "DJF", "DKK", "DOP", "DZD", "EGP", "ERN", "ETB",
            "EUR", "FJD", "FKP", "GBP", "GEL", "GHS", "GIP", "GMD", "GNF", "GTQ", "GYD", "HKD",
            "HNL", "HTG", "HUF", "IDR", "ILS", "INR", "IQD", "IRR", "ISK", "JMD", "JOD", "JPY",
            "KES", "KGS", "KHR", "KMF", "KPW", "KRW", "KWD", "KYD", "KZT", "LAK", "LBP", "LKR",
            "LRD", "LSL", "LYD", "MAD", "MDL", "MGA", "MKD", "MMK", "MNT", "MOP", "MRU", "MUR",
            "MVR", "MWK", "MXN", "MXV", "MYR", "MZN", "NAD", "NGN", "NIO", "NOK", "NPR", "NZD",
            "OMR", "PAB", "PEN", "PGK", "PHP", "PKR", "PLN", "PYG", "QAR", "RON", "RSD", "RUB",
            "RWF", "SAR", "SBD", "SCR", "SDG", "SEK", "SGD", "SHP", "SLE", "SLL", "SOS", "SRD",
            "SSP", "STN", "SVC", "SYP", "SZL", "THB", "TJS", "TMT", "TND", "TOP", "TRY", "TTD",
            "TWD", "TZS", "UAH", "UGX", "USD", "USN", "UYI", "UYU", "UYW", "UZS", "VED", "VES",
            "VND", "VUV", "WST", "XAF", "XAG", "XAU", "XBA", "XBB", "XBC", "XBD", "XCD", "XCG",
            "XDR", "XOF", "XPD", "XPF", "XPT", "XSU", "XUA", "YER", "ZAR", "ZMW", "ZWG", "ZWL");

    public UsageUnit {
        if (name == null || name.trim().isEmpty()) {
            throw new IllegalArgumentException("Invalid usage unit - the name cannot be empty");
        }

        name = Allocation.canonicalize(name);

        if (name.codePointCount(0, name.length()) > MAX_NAME_LENGTH) {
            throw new IllegalArgumentException("Invalid usage unit - the name is longer than "
                    + MAX_NAME_LENGTH + " characters '" + name + "'");
        }

        if (name.chars().anyMatch(Character::isISOControl)) {
            throw new IllegalArgumentException(
                    "Invalid usage unit - the name contains a control character");
        }

        if (countsPerUnit < 1) {
            throw new IllegalArgumentException("Invalid usage unit '" + name
                    + "' - counts_per_unit must be at least 1");
        }
    }

    /** Any unit, at any resolution: {@code UsageUnit.of("CREDITS", 100)}. */
    public static UsageUnit of(String name, long countsPerUnit) {
        return new UsageUnit(name, countsPerUnit);
    }

    /**
     * A well-known unit at its default resolution: a time unit ({@code NHR},
     * {@code GPUHR}, ... in any spelling {@link Allocation} accepts) at 3600
     * counts per hour, or a currency (an ISO 4217 code, or {@code £} /
     * {@code €}) at 1000000 counts per unit. Anything else is refused: use
     * {@link #of} for a unit of your own, so its resolution is stated.
     */
    public static UsageUnit parse(String name) {
        String code = currencyCode(name);

        if (code != null) {
            return new UsageUnit(code, COUNTS_PER_CURRENCY_UNIT);
        }

        if (name != null && TIME_UNITS.contains(Allocation.canonicalize(name))) {
            return new UsageUnit(name, COUNTS_PER_HOUR);
        }

        throw new IllegalArgumentException("Unknown usage unit '" + name + "'. Use a time "
                + "unit (NHR, GPUHR, CPUHR, COREHR, GBHR, MBHR, BHR), an ISO 4217 currency "
                + "code (e.g. GBP, USD, EUR), or give the counts per unit explicitly");
    }

    /** The ISO 4217 code a name spells - {@code "gbp"}, {@code "£"} - or null. */
    static String currencyCode(String name) {
        if (name == null) {
            return null;
        }

        String trimmed = name.trim();

        if (trimmed.equals("£")) {
            return "GBP";
        }

        if (trimmed.equals("€")) {
            return "EUR";
        }

        String upper = trimmed.toUpperCase(Locale.ROOT);

        return CURRENCY_CODES.contains(upper) ? upper : null;
    }

    public boolean isTime() {
        return TIME_UNITS.contains(name);
    }

    public boolean isCurrency() {
        return CURRENCY_CODES.contains(name);
    }

    /**
     * An amount of this unit as a {@link Usage}, rounded to the nearest count
     * (ties to even). Negative, NaN, infinite or too-large amounts are refused.
     * For money prefer {@link #usage(BigDecimal)}, which is exact.
     */
    public Usage usage(double amount) {
        if (Double.isNaN(amount) || Double.isInfinite(amount)) {
            throw new IllegalArgumentException("Invalid amount of " + name
                    + " - it must be a finite number, not '" + amount + "'");
        }

        if (amount < 0.0) {
            throw new IllegalArgumentException(
                    "Invalid amount of " + name + " - it cannot be negative (" + amount + ")");
        }

        double counts = Math.rint(amount * countsPerUnit);

        if (counts >= (double) Long.MAX_VALUE) {
            throw new IllegalArgumentException(
                    "Invalid amount of " + name + " - " + amount + " is too large to count");
        }

        return Usage.fromCounts((long) counts);
    }

    /**
     * An exact amount of this unit as a {@link Usage}: {@code 12.34} GBP is
     * exactly 12340000 counts. An amount finer than one count is refused
     * rather than rounded.
     */
    public Usage usage(BigDecimal amount) {
        if (amount.signum() < 0) {
            throw new IllegalArgumentException(
                    "Invalid amount of " + name + " '" + amount + "' - it cannot be negative");
        }

        BigDecimal counts = amount.multiply(BigDecimal.valueOf(countsPerUnit));

        BigInteger exact;

        try {
            exact = counts.setScale(0, RoundingMode.UNNECESSARY).toBigIntegerExact();
        } catch (ArithmeticException e) {
            throw new IllegalArgumentException("Invalid amount of " + name + " '" + amount
                    + "' - it is finer than one count (" + countsPerUnit + " per " + name + ")");
        }

        if (exact.bitLength() > 63) {
            throw new IllegalArgumentException(
                    "Invalid amount of " + name + " '" + amount + "' - it is too large to count");
        }

        return Usage.fromCounts(exact.longValueExact());
    }

    /** As {@link #usage(BigDecimal)}, from its text: {@code "12.34"}. */
    public Usage usageFromDecimal(String amount) {
        BigDecimal parsed;

        try {
            parsed = new BigDecimal(amount.trim());
        } catch (NumberFormatException e) {
            throw new IllegalArgumentException("Invalid amount of " + name + " '" + amount
                    + "' - it must be a decimal number, such as '12.34'");
        }

        return usage(parsed);
    }

    /** The amount of this unit a {@link Usage} holds. */
    public double amount(Usage usage) {
        return (double) usage.counts() / countsPerUnit;
    }

    /**
     * How many decimal places to print by default: exactly the resolution when
     * it is a power of ten (6 for a currency), otherwise 3.
     */
    public int defaultDecimals() {
        long perUnit = countsPerUnit;
        int decimals = 0;

        while (perUnit > 1 && perUnit % 10 == 0) {
            perUnit /= 10;
            decimals++;
        }

        return perUnit == 1 ? decimals : 3;
    }

    @Override
    public String typeName() {
        return "UsageUnit";
    }

    @Override
    public JsonNode toJson() {
        return Json.object().put("name", name).put("counts_per_unit", countsPerUnit);
    }

    public static UsageUnit fromJson(JsonNode node) {
        if (node == null || !node.isObject()) {
            throw new IllegalArgumentException("a usage unit is a JSON object");
        }

        return new UsageUnit(node.path("name").asText(""),
                node.path("counts_per_unit").asLong(0));
    }

    @Override
    public String toString() {
        return name + " (" + countsPerUnit + " per unit)";
    }
}
