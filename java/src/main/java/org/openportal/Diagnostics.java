// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.List;
import java.util.Optional;

/**
 * The bridge's answer to {@code POST /diagnostics}.
 *
 * <p>A status word plus, when the agent answered, its
 * {@link DiagnosticsReport}. The field is {@code report} on the wire while the
 * accessor is {@link #detail}, matching {@link Health}.
 */
public record Diagnostics(JsonNode json) {

    public static Diagnostics fromJson(JsonNode node) {
        return new Diagnostics(node == null ? Json.object() : node);
    }

    public String status() {
        return json.path("status").asText();
    }

    public boolean isHealthy() {
        return "ok".equalsIgnoreCase(status());
    }

    /** The report, or empty when the agent did not answer. */
    public Optional<DiagnosticsReport> detail() {
        JsonNode found = json.path("report");

        return found.isObject() ? Optional.of(new DiagnosticsReport(found)) : Optional.empty();
    }

    /** The contained report's log, oldest first. Empty when there is no report. */
    public List<LogEntry> logs() {
        return logs(0, null, null);
    }

    /** As {@link DiagnosticsReport#logs(int, String, String)}. */
    public List<LogEntry> logs(int max, String level, String search) {
        return detail().map(report -> report.logs(max, level, search))
                .orElseGet(java.util.List::of);
    }

    @Override
    public String toString() {
        return Json.write(json);
    }
}
