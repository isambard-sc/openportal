// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.time.Instant;
import java.time.LocalDate;
import java.time.format.DateTimeParseException;

/**
 * Reading and writing the two time forms that appear on the wire.
 *
 * <p>A date is {@code "2026-09-01"}. A timestamp is RFC 3339 with a {@code Z}
 * suffix and however many fractional digits the sender had -
 * {@code "2026-09-02T08:28:06.602549301Z"} carries nanoseconds, because it came
 * from a Rust {@code DateTime<Utc>}. {@link Instant} holds nanosecond precision
 * too, so nothing is lost; a formatter with a fixed number of fractional digits
 * would lose or invent them, which is why this uses {@code Instant}'s own
 * parsing rather than a pattern.
 */
final class Times {

    private Times() {}

    static String toJson(Instant when) {
        // `Instant.toString` is ISO-8601 with a `Z` and the minimum number of
        // fractional digits - the same choice chrono makes.
        return when.toString();
    }

    static Instant fromJson(JsonNode node) {
        if (node == null || node.isNull()) {
            return Instant.EPOCH;
        }

        String text = node.asText();

        try {
            return Instant.parse(text);
        } catch (DateTimeParseException e) {
            // A timestamp with an offset rather than `Z` - not what the agents
            // send, but valid RFC 3339 and free to accept.
            try {
                return java.time.OffsetDateTime.parse(text).toInstant();
            } catch (DateTimeParseException nested) {
                throw new IllegalArgumentException("Invalid timestamp '" + text + "'");
            }
        }
    }

    static String dateToJson(LocalDate date) {
        return date.toString();
    }

    static LocalDate dateFromJson(JsonNode node) {
        return Dates.parse(node.asText());
    }
}
