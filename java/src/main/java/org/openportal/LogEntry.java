// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.time.Instant;

/**
 * A log line captured from an agent's tracing, as it appears in a
 * {@link DiagnosticsReport}.
 */
public record LogEntry(JsonNode json) {

    public Instant timestamp() {
        return Times.fromJson(json.path("timestamp"));
    }

    /** {@code ERROR}, {@code WARN}, {@code INFO}, {@code DEBUG} or {@code TRACE}. */
    public String level() {
        return json.path("level").asText();
    }

    /** The module that logged it. */
    public String target() {
        return json.path("target").asText();
    }

    public String message() {
        return json.path("message").asText();
    }

    @Override
    public String toString() {
        return timestamp() + " " + level() + " " + target() + ": " + message();
    }
}
