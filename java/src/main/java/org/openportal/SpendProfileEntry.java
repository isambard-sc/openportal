// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.time.LocalDate;

/**
 * One entry in an award's spend profile: the least that should be spent from
 * a given date.
 *
 * <p>By convention between the two portals, an entry applies from its
 * {@code from} date up to, but not including, the next entry's, and the last
 * until the award's end date. The minimums are floors ("use it or lose it")
 * and need not add up to the allocation. OpenPortal does not interpret any of
 * that - it carries the profile intact, and what it means is for the awarding
 * portal and the site to agree. See {@link AwardDetails#spendProfile()}.
 *
 * <p>On the wire, {@code {"from": "2027-01-01", "minimum": "100 NHR"}}.
 */
public record SpendProfileEntry(LocalDate from, Allocation minimum) {

    public SpendProfileEntry {
        if (from == null) {
            throw new IllegalArgumentException("a spend profile entry needs a date");
        }

        if (minimum == null) {
            throw new IllegalArgumentException("a spend profile entry needs a minimum");
        }
    }

    public static SpendProfileEntry of(LocalDate from, Allocation minimum) {
        return new SpendProfileEntry(from, minimum);
    }

    /** From the wire spellings, e.g. {@code of("2027-01-01", "100 NHR")}. */
    public static SpendProfileEntry of(String from, String minimum) {
        return new SpendProfileEntry(Dates.parse(from), Allocation.parse(minimum));
    }

    public JsonNode toJson() {
        return Json.object()
                .put("from", from.toString())
                .put("minimum", minimum.toString());
    }

    public static SpendProfileEntry fromJson(JsonNode node) {
        if (node == null || !node.isObject()) {
            throw new IllegalArgumentException(
                    "a spend profile entry must be an object with 'from' and 'minimum'");
        }

        if (!node.hasNonNull("from") || !node.hasNonNull("minimum")) {
            throw new IllegalArgumentException(
                    "a spend profile entry needs both 'from' and 'minimum': " + node);
        }

        return of(node.get("from").asText(), node.get("minimum").asText());
    }

    /** {@code "2027-01-01: 100 NHR"}, as the Rust side prints it. */
    @Override
    public String toString() {
        return from + ": " + minimum;
    }
}
