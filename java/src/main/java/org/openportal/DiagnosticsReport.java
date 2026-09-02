// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Locale;

/**
 * An agent's troubleshooting report: what failed, what was slow, what expired,
 * what is running, and the recent log.
 *
 * <p>Every list is capped and deduplicated by the agent - the hundred most
 * recent of each - so a report is a sample rather than a complete history.
 */
public record DiagnosticsReport(JsonNode json) {

    public String agentName() {
        return json.path("agent_name").asText();
    }

    public Instant generatedAt() {
        return Times.fromJson(json.path("generated_at"));
    }

    public List<FailedJobEntry> failedJobs() {
        List<FailedJobEntry> entries = new ArrayList<>();
        json.path("failed_jobs").forEach(entry -> entries.add(new FailedJobEntry(entry)));

        return entries;
    }

    public List<SlowJobEntry> slowestJobs() {
        List<SlowJobEntry> entries = new ArrayList<>();
        json.path("slowest_jobs").forEach(entry -> entries.add(new SlowJobEntry(entry)));

        return entries;
    }

    public List<ExpiredJobEntry> expiredJobs() {
        List<ExpiredJobEntry> entries = new ArrayList<>();
        json.path("expired_jobs").forEach(entry -> entries.add(new ExpiredJobEntry(entry)));

        return entries;
    }

    public List<RunningJobEntry> runningJobs() {
        List<RunningJobEntry> entries = new ArrayList<>();
        json.path("running_jobs").forEach(entry -> entries.add(new RunningJobEntry(entry)));

        return entries;
    }

    public List<String> warnings() {
        List<String> warnings = new ArrayList<>();
        json.path("warnings").forEach(entry -> warnings.add(entry.asText()));

        return warnings;
    }

    public NotificationStatistics notificationStatistics() {
        return new NotificationStatistics(json.path("notification_statistics"));
    }

    /** Every log entry, <b>oldest first</b> - the wire order is the other way round. */
    public List<LogEntry> logs() {
        return logs(0, null, null);
    }

    /**
     * The log, filtered.
     *
     * <p>{@code max} of {@code 0} means all. {@code level} is a minimum -
     * {@code "WARN"} gives warnings and errors, and the {@code "WARN+"} spelling
     * is accepted too. {@code search} is a case-insensitive substring of the
     * message. Oldest first.
     */
    public List<LogEntry> logs(int max, String level, String search) {
        List<LogEntry> entries = new ArrayList<>();

        // `recent_logs` on the wire, most recent first.
        json.path("recent_logs").forEach(entry -> entries.add(new LogEntry(entry)));
        Collections.reverse(entries);

        int minimum = level == null ? Integer.MIN_VALUE : severity(level);
        List<LogEntry> kept = new ArrayList<>();

        for (LogEntry entry : entries) {
            if (severity(entry.level()) < minimum) {
                continue;
            }

            if (search != null && !entry.message().toLowerCase(Locale.ROOT)
                    .contains(search.toLowerCase(Locale.ROOT))) {
                continue;
            }

            kept.add(entry);
        }

        // The newest `max`, still oldest first.
        if (max > 0 && kept.size() > max) {
            return new ArrayList<>(kept.subList(kept.size() - max, kept.size()));
        }

        return kept;
    }

    @Override
    public String toString() {
        return Json.write(json);
    }

    private static int severity(String level) {
        String name = level == null ? "" : level.trim().toUpperCase(Locale.ROOT);

        // "WARN+" and "WARN" mean the same thing here - the filter is already a
        // minimum, and the suffix is what the Python module accepts.
        if (name.endsWith("+")) {
            name = name.substring(0, name.length() - 1);
        }

        return switch (name) {
            case "TRACE" -> 0;
            case "DEBUG" -> 1;
            case "INFO" -> 2;
            case "WARN", "WARNING" -> 3;
            case "ERROR" -> 4;
            default -> Integer.MIN_VALUE;
        };
    }
}
