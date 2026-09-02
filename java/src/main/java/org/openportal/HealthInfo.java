// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.time.Instant;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * One agent's health, and its peers' underneath it.
 *
 * <p>Nested: an agent reports its own counters and, in {@link #peers}, whatever
 * its downstream agents reported to it. So the whole network's state arrives as
 * one tree from whichever agent you asked.
 *
 * <p>{@link #connected} is the field to check first - the counters of an agent
 * that is not connected are the last ones anybody saw, not the current ones.
 */
public record HealthInfo(JsonNode json) {

    public String name() {
        return json.path("name").asText();
    }

    public AgentType agentType() {
        return AgentType.parse(json.path("agent_type").asText());
    }

    /** Whether this agent is reachable. Check before trusting the counters. */
    public boolean connected() {
        return json.path("connected").asBoolean();
    }

    public long activeJobs() {
        return json.path("active_jobs").asLong();
    }

    public long pendingJobs() {
        return json.path("pending_jobs").asLong();
    }

    public long runningJobs() {
        return json.path("running_jobs").asLong();
    }

    public long completedJobs() {
        return json.path("completed_jobs").asLong();
    }

    public long successfulJobs() {
        return json.path("successful_jobs").asLong();
    }

    public long expiredJobs() {
        return json.path("expired_jobs").asLong();
    }

    /** Jobs that failed, <b>not</b> counting the ones that expired. */
    public long erroredJobs() {
        return json.path("errored_jobs").asLong();
    }

    public long duplicateJobs() {
        return json.path("duplicate_jobs").asLong();
    }

    /** Jobs passing through intermediate agents. */
    public long inflightJobs() {
        return json.path("inflight_jobs").asLong();
    }

    /** Jobs waiting for a connection rather than for work. */
    public long queuedJobs() {
        return json.path("queued_jobs").asLong();
    }

    public long workerCount() {
        return json.path("worker_count").asLong();
    }

    public long memoryBytes() {
        return json.path("memory_bytes").asLong();
    }

    public double cpuPercent() {
        return json.path("cpu_percent").asDouble();
    }

    public long systemMemoryTotal() {
        return json.path("system_memory_total").asLong();
    }

    public long systemCpus() {
        return json.path("system_cpus").asLong();
    }

    public double jobTimeMinMs() {
        return json.path("job_time_min_ms").asDouble();
    }

    public double jobTimeMaxMs() {
        return json.path("job_time_max_ms").asDouble();
    }

    public double jobTimeMeanMs() {
        return json.path("job_time_mean_ms").asDouble();
    }

    public double jobTimeMedianMs() {
        return json.path("job_time_median_ms").asDouble();
    }

    public long jobTimeCount() {
        return json.path("job_time_count").asLong();
    }

    public long totalCompleted() {
        return json.path("total_completed").asLong();
    }

    public long totalFailed() {
        return json.path("total_failed").asLong();
    }

    public long totalExpired() {
        return json.path("total_expired").asLong();
    }

    /** Jobs that took over a second. */
    public long totalSlow() {
        return json.path("total_slow").asLong();
    }

    public Instant startTime() {
        return Times.fromJson(json.path("start_time"));
    }

    /** The agent's own clock. Worth comparing against yours - the bridge signs by time. */
    public Instant currentTime() {
        return Times.fromJson(json.path("current_time"));
    }

    public long uptimeSeconds() {
        return json.path("uptime_seconds").asLong();
    }

    public String engine() {
        return json.path("engine").asText();
    }

    public String version() {
        return json.path("version").asText();
    }

    public Instant lastUpdated() {
        return Times.fromJson(json.path("last_updated"));
    }

    /** The agents downstream of this one, by name. */
    public Map<String, HealthInfo> peers() {
        Map<String, HealthInfo> peers = new LinkedHashMap<>();
        JsonNode found = json.path("peers");

        if (found.isObject()) {
            found.fields().forEachRemaining(entry ->
                    peers.put(entry.getKey(), new HealthInfo(entry.getValue())));
        }

        return peers;
    }

    public List<String> peerNames() {
        return new java.util.ArrayList<>(peers().keySet());
    }

    /** One peer by name, or empty. */
    public java.util.Optional<HealthInfo> peer(String name) {
        return java.util.Optional.ofNullable(peers().get(name));
    }

    @Override
    public String toString() {
        return Json.write(json);
    }
}
