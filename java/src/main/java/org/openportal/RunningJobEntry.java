// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.time.Instant;

/**
 * A job in flight right now, deduplicated with a count.
 *
 * <p>{@link #runningForSeconds} against the expiry is the useful reading: one
 * that has been running longer than the answer budget is about to expire.
 */
public record RunningJobEntry(JsonNode json) {

    public String destination() {
        return json.path("destination").asText();
    }

    public String instruction() {
        return json.path("instruction").asText();
    }

    public Instant startedAt() {
        return Times.fromJson(json.path("started_at"));
    }

    public long count() {
        return json.path("count").asLong();
    }

    public long runningForSeconds() {
        return json.path("running_for_seconds").asLong();
    }

    @Override
    public String toString() {
        return destination() + " " + instruction() + " (×" + count() + ") for "
                + runningForSeconds() + "s";
    }
}
