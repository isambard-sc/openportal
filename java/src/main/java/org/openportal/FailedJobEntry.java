// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.time.Instant;

/**
 * A failure an agent saw, deduplicated by destination and instruction.
 *
 * <p>{@link #count} is how many times this same failure happened, and
 * {@link #firstSeen}/{@link #lastSeen} bound the window - so one entry can
 * stand for a great many failures. A high count with a recent
 * {@code lastSeen} is something still going wrong.
 */
public record FailedJobEntry(JsonNode json) {

    public String destination() {
        return json.path("destination").asText();
    }

    public String instruction() {
        return json.path("instruction").asText();
    }

    public String errorMessage() {
        return json.path("error_message").asText();
    }

    public long count() {
        return json.path("count").asLong();
    }

    public Instant firstSeen() {
        return Times.fromJson(json.path("first_seen"));
    }

    public Instant lastSeen() {
        return Times.fromJson(json.path("last_seen"));
    }

    /** The typed error this failure decodes to. */
    public OpenPortalError error() {
        return OpenPortalError.decode(errorMessage());
    }

    @Override
    public String toString() {
        return destination() + " " + instruction() + " (×" + count() + "): " + errorMessage();
    }
}
