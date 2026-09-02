// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.time.Instant;

/** One of the slowest jobs an agent completed - it succeeded, it just took a while. */
public record SlowJobEntry(JsonNode json) {

    public String destination() {
        return json.path("destination").asText();
    }

    public String instruction() {
        return json.path("instruction").asText();
    }

    public double durationMs() {
        return json.path("duration_ms").asDouble();
    }

    public Instant completedAt() {
        return Times.fromJson(json.path("completed_at"));
    }

    @Override
    public String toString() {
        return destination() + " " + instruction() + ": " + Fmt.fixed(durationMs(), 1) + " ms";
    }
}
