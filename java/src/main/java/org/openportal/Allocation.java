// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.Locale;
import java.util.Optional;

/**
 * How much resource an award grants, and <b>in which unit</b>.
 *
 * <p>The unit is the part that matters. An award allocates in the <i>awarding
 * portal's</i> unit - {@code "5000 GPUHR"} - and that is the unit every usage
 * report about the award must come back in. A site that accounts in its own unit
 * (node hours, say) converts on the way out, exactly as it remaps identifiers on
 * the way out. Reporting site units against an award denominated in GPU hours is
 * not an error either side can detect; it just produces wrong numbers.
 *
 * <p>Two refusals follow from that, and the {@code site_portal} example makes
 * both:
 *
 * <ul>
 *   <li>An award whose unit the site has no agreed conversion for cannot be
 *       reported on, so it is refused rather than reported with a guessed
 *       factor.
 *   <li>An award with no allocation, or one of zero, is not an award - and the
 *       allocation is also what <i>names</i> the unit, so without it there is
 *       nothing to convert to.
 * </ul>
 *
 * <p>Units are names on numbers: {@code "GPUHR"} works exactly as
 * {@code "credits"} or {@code "dollar_seconds"} would. Six names are
 * canonicalised ({@code NHR}, {@code GPUHR}, {@code CPUHR}, {@code COREHR},
 * {@code GBHR}, {@code BHR}), each from a couple of spellings; anything else is
 * lower-cased and kept as given, so {@code "GPU-hours"} and {@code "gpu_hours"}
 * stay distinct from each other and from {@code GPUHR}. Agree the exact string
 * out of band.
 *
 * <p>On the wire this is one string, {@code "5000 GPUHR"}, or
 * {@code "No allocation"} when empty.
 */
public record Allocation(Double size, String units) implements OpenPortalType {

    private static final String NO_ALLOCATION = "No allocation";

    public Allocation {
        if (size != null) {
            if (!Double.isFinite(size)) {
                // `Double.parseDouble` accepts "NaN" and "Infinity", and a
                // negative test is *false* for NaN, so both used to parse
                // cleanly here and then saturate to the maximum downstream.
                throw new IllegalArgumentException(
                        "Invalid Allocation - size must be a finite number '" + size + "'");
            }

            if (size < 0.0) {
                throw new IllegalArgumentException(
                        "Invalid Allocation - size cannot be negative '" + size + "'");
            }
        }
    }

    /** No allocation at all - what an empty award field deserialises to. */
    public static Allocation empty() {
        return new Allocation(null, null);
    }

    public static Allocation of(double size, String units) {
        if (units == null || units.trim().isEmpty()) {
            throw new IllegalArgumentException("Invalid Allocation - units cannot be empty");
        }

        return new Allocation(size, canonicalize(units));
    }

    /**
     * Parse the {@code "<size> <units>"} form.
     *
     * <p>{@code "none"} and {@code "no allocation"} give an empty allocation.
     * The size and units must be separated by whitespace - {@code "5000GPUHR"}
     * is a parse error, not a size of 5000.
     */
    public static Allocation parse(String value) {
        if (value == null || value.trim().isEmpty()) {
            throw new IllegalArgumentException("Invalid Allocation - cannot be empty");
        }

        String trimmed = value.trim();
        String lower = trimmed.toLowerCase(Locale.ROOT);

        if (lower.equals("none") || lower.equals("no allocation")) {
            return empty();
        }

        String[] parts = trimmed.split("\\s+");

        if (parts.length < 2) {
            throw new IllegalArgumentException(
                    "Invalid Allocation - must contain a size and units '" + trimmed + "'");
        }

        double size;

        try {
            size = Double.parseDouble(parts[0]);
        } catch (NumberFormatException e) {
            throw new IllegalArgumentException(
                    "Invalid Allocation - size must be a number '" + parts[0] + "'");
        }

        StringBuilder units = new StringBuilder(parts[1]);

        for (int i = 2; i < parts.length; i++) {
            units.append(' ').append(parts[i]);
        }

        return of(size, units.toString());
    }

    /**
     * The canonical spelling of a unit name.
     *
     * <p>Only the six known units are folded together. Anything else comes back
     * <b>lower-cased</b> rather than unchanged, which is the surprise: pass
     * {@code "CHR"} and you get {@code "chr"}, not {@code "CHR"} - and
     * {@code "chr"} is then what the other side compares against.
     */
    public static String canonicalize(String units) {
        String canonical = units == null ? "" : units.trim().toLowerCase(Locale.ROOT);

        return switch (canonical) {
            case "node hours", "node hour", "nhr" -> "NHR";
            case "gpu hours", "gpu hour", "gpuhr" -> "GPUHR";
            case "cpu hours", "cpu hour", "cpuhr" -> "CPUHR";
            case "core hours", "core hour", "corehr" -> "COREHR";
            case "gb hours", "gb hour", "gbhr" -> "GBHR";
            case "billing hours", "billing hour", "bhr" -> "BHR";
            default -> canonical;
        };
    }

    public Optional<Double> sizeIfSet() {
        return Optional.ofNullable(size);
    }

    public Optional<String> unitsIfSet() {
        return Optional.ofNullable(units);
    }

    public boolean isEmpty() {
        return size == null;
    }

    public boolean isNodeHours() {
        return "NHR".equals(units);
    }

    public boolean isGpuHours() {
        return "GPUHR".equals(units);
    }

    public boolean isCpuHours() {
        return "CPUHR".equals(units);
    }

    public boolean isCoreHours() {
        return "COREHR".equals(units);
    }

    public boolean isGbHours() {
        return "GBHR".equals(units);
    }

    public boolean isBillingHours() {
        return "BHR".equals(units);
    }

    /**
     * This allocation as node hours on the given node.
     *
     * <p>Raises rather than answering zero when the node cannot express the
     * unit - no GPUs for a GPU-hour allocation, no billing factor for a
     * billing-hour one. A silent zero here is the worst outcome available: an
     * award would be provisioned with nothing.
     */
    public Usage toNodeHours(Node node) {
        if (size != null) {
            if (isNodeHours()) {
                return Usage.fromHours(size);
            }

            if (isCpuHours() || isCoreHours()) {
                return Usage.fromHours(size / requireNonZero(node.cores(),
                        "Node has no cores, cannot convert " + units + " to node hours"));
            }

            if (isGpuHours()) {
                return Usage.fromHours(size / requireNonZero(node.gpus(),
                        "Node has no GPUs, cannot convert GPU hours to node hours"));
            }

            if (isGbHours()) {
                return Usage.fromHours(size / requireNonZero(node.memoryGb(),
                        "Node has no memory, cannot convert GB hours to node hours"));
            }

            if (isBillingHours()) {
                return Usage.fromHours(size / requireNonZero(node.billing(),
                        "Node has no billing factor, cannot convert billing hours to node hours"));
            }
        }

        throw new IllegalStateException("Cannot convert allocation '" + this + "' to node hours.");
    }

    public Usage toCpuHours(Node node) {
        return toNodeHours(node).times(node.cpus());
    }

    public Usage toGpuHours(Node node) {
        return toNodeHours(node).times(node.gpus());
    }

    public Usage toCoreHours(Node node) {
        return toNodeHours(node).times(node.cores());
    }

    public Usage toGbHours(Node node) {
        return toNodeHours(node).times(node.memoryGb());
    }

    public Usage toBillingHours(Node node) {
        return toNodeHours(node).times(node.billing());
    }

    public static Allocation fromNodeHours(Usage usage) {
        return of(usage.hours(), "NHR");
    }

    public static Allocation fromCpuHours(Usage usage, Node node) {
        return of(usage.hours() / node.cpus(), "NHR");
    }

    public static Allocation fromGpuHours(Usage usage, Node node) {
        return of(usage.hours() / node.gpus(), "NHR");
    }

    public static Allocation fromCoreHours(Usage usage, Node node) {
        return of(usage.hours() / node.cores(), "NHR");
    }

    public static Allocation fromGbHours(Usage usage, Node node) {
        return of(usage.hours() / node.memoryGb(), "NHR");
    }

    /** Note the unit: billing hours come back as {@code BHR}, not {@code NHR}. */
    public static Allocation fromBillingHours(Usage usage, Node node) {
        return of(usage.hours() / node.billing(), "BHR");
    }

    @Override
    public String typeName() {
        return "Allocation";
    }

    @Override
    public JsonNode toJson() {
        return Json.text(toString());
    }

    public static Allocation fromJson(JsonNode node) {
        if (node == null || node.isNull()) {
            return empty();
        }

        return parse(node.asText());
    }

    @Override
    public String toString() {
        if (size == null) {
            return NO_ALLOCATION;
        }

        if (units == null) {
            return Fmt.number(size);
        }

        return Fmt.number(size) + " " + units;
    }

    private static double requireNonZero(double value, String message) {
        if (value == 0.0) {
            throw new IllegalStateException(message);
        }

        return value;
    }
}
