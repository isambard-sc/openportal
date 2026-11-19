// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.time.LocalDate;
import java.util.Objects;
import java.util.Optional;

/**
 * One entry in an award's spend profile: the controls on spending that apply
 * from a given date - a {@code minimum} ("use it or lose it"), a
 * {@code maximum} (a cap), both, or neither.
 *
 * <p>By convention between the two portals, an entry applies from its
 * {@code from} date up to, but not including, the next entry's, and the last
 * until the award's end date. <b>Each entry describes only its own period</b>:
 * nothing carries over from the entry before, so a control an entry does not
 * set does not apply in its period. An entry that sets neither therefore
 * means "no controls from this date" - see {@link #noControls}.
 *
 * <p>OpenPortal does not interpret any of that - it carries the profile
 * intact, and what it means is for the awarding portal and the site to agree.
 * It refuses only what would make an entry mean two things: an empty
 * allocation as a control, and a minimum and maximum in different units or
 * with the maximum below the minimum. See {@link AwardDetails#spendProfile()}.
 *
 * <p>On the wire, {@code {"from": "2027-01-01", "minimum": "100 NHR",
 * "maximum": "300 NHR"}}, with an unset control left out.
 *
 * @param minimum the least that should be spent in this period, or
 *     {@code null} for no minimum
 * @param maximum the most that may be spent in this period, or {@code null}
 *     for no maximum
 */
public record SpendProfileEntry(LocalDate from, Allocation minimum, Allocation maximum) {

    public SpendProfileEntry {
        if (from == null) {
            throw new IllegalArgumentException("a spend profile entry needs a date");
        }

        checkNotEmpty(from, "minimum", minimum);
        checkNotEmpty(from, "maximum", maximum);

        if (minimum != null && maximum != null) {
            if (!Objects.equals(minimum.units(), maximum.units())) {
                throw new IllegalArgumentException("Invalid spend profile entry from " + from
                        + " - the minimum (" + minimum + ") and maximum (" + maximum
                        + ") must be in the same units");
            }

            if (maximum.size() < minimum.size()) {
                throw new IllegalArgumentException("Invalid spend profile entry from " + from
                        + " - the maximum (" + maximum + ") is less than the minimum ("
                        + minimum + ")");
            }
        }
    }

    private static void checkNotEmpty(LocalDate from, String name, Allocation control) {
        // "none" as a maximum could mean "no cap" or "nothing" - so it is
        // refused, and an unset control is spelled by leaving it out.
        if (control != null && control.isEmpty()) {
            throw new IllegalArgumentException("Invalid spend profile entry from " + from
                    + " - the " + name + " cannot be an empty allocation. Leave it out for no "
                    + name + " in this period");
        }
    }

    public static SpendProfileEntry of(LocalDate from, Allocation minimum, Allocation maximum) {
        return new SpendProfileEntry(from, minimum, maximum);
    }

    /**
     * From the wire spellings, e.g. {@code of("2027-01-01", "100 NHR", null)}.
     * Either control may be {@code null}.
     */
    public static SpendProfileEntry of(String from, String minimum, String maximum) {
        return new SpendProfileEntry(Dates.parse(from),
                minimum == null ? null : Allocation.parse(minimum),
                maximum == null ? null : Allocation.parse(maximum));
    }

    /** A minimum only. */
    public static SpendProfileEntry minimum(String from, String minimum) {
        return of(from, minimum, null);
    }

    /** A maximum only. */
    public static SpendProfileEntry maximum(String from, String maximum) {
        return of(from, null, maximum);
    }

    /** No controls from this date: nothing required, nothing capped. */
    public static SpendProfileEntry noControls(LocalDate from) {
        return new SpendProfileEntry(from, null, null);
    }

    public static SpendProfileEntry noControls(String from) {
        return noControls(Dates.parse(from));
    }

    public Optional<Allocation> minimumSpend() {
        return Optional.ofNullable(minimum);
    }

    public Optional<Allocation> maximumSpend() {
        return Optional.ofNullable(maximum);
    }

    /** True if this entry sets neither control - it ends any controls from its date. */
    public boolean hasNoControls() {
        return minimum == null && maximum == null;
    }

    public JsonNode toJson() {
        ObjectNode node = Json.object().put("from", from.toString());

        if (minimum != null) {
            node.put("minimum", minimum.toString());
        }

        if (maximum != null) {
            node.put("maximum", maximum.toString());
        }

        return node;
    }

    public static SpendProfileEntry fromJson(JsonNode node) {
        if (node == null || !node.isObject()) {
            throw new IllegalArgumentException(
                    "a spend profile entry must be an object with a 'from' date");
        }

        if (!node.hasNonNull("from")) {
            throw new IllegalArgumentException("a spend profile entry needs 'from': " + node);
        }

        // Any other field is ignored, as it is everywhere else in an award, so
        // a field added to entries later does not stop this parsing the rest.
        return of(node.get("from").asText(),
                node.hasNonNull("minimum") ? node.get("minimum").asText() : null,
                node.hasNonNull("maximum") ? node.get("maximum").asText() : null);
    }

    /**
     * Spelled out in words, as the Rust side prints it - so an entry with no
     * controls reads as what it means rather than as a date with something
     * missing: {@code "from 2027-01-01: spend exactly 100 NHR"},
     * {@code "... spend at least 100 NHR and at most 300 NHR"},
     * {@code "... spend at least 100 NHR"}, {@code "... spend at most 300 NHR"},
     * {@code "from 2027-10-01: no spend controls"}.
     */
    @Override
    public String toString() {
        String prefix = "from " + from + ": ";

        if (minimum != null && maximum != null) {
            return minimum.equals(maximum)
                    ? prefix + "spend exactly " + minimum
                    : prefix + "spend at least " + minimum + " and at most " + maximum;
        }

        if (minimum != null) {
            return prefix + "spend at least " + minimum;
        }

        if (maximum != null) {
            return prefix + "spend at most " + maximum;
        }

        return prefix + "no spend controls";
    }
}
