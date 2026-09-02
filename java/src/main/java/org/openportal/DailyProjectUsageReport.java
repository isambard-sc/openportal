// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;

/**
 * One project's usage on one day, broken down by local user.
 *
 * <p>The leaf of a usage report, and the thing a site portal actually fills in:
 * {@code addUsage("alice", Usage.fromHours(4))} for each local user that
 * consumed something, then {@link #setComplete()} once the day will not change
 * again.
 *
 * <p>Keyed by <b>local</b> username, not by the portal's
 * {@link UserIdentifier} - the translation happens one level up, in
 * {@link ProjectUsageReport}, which carries the mapping. Usage that cannot be
 * attributed to a user goes under {@link #UNATTRIBUTED} ({@code "unknown"}) via
 * {@link #addUnattributedUsage}; it still counts towards the total.
 *
 * <p>The unit is not recorded here. A day's figures are in whatever unit the
 * report they belong to is in - see {@link Allocation}.
 *
 * <p>Held as its own JSON so that a report read from a peer keeps every field
 * this class does not name. The wire type carries about thirty fields - requeue
 * accounting, expansion factors, reservation occupancy - which an agent
 * populates and re-reads; rebuilding one from named fields would silently drop
 * whatever this client had not modelled. {@link #plus} and {@link #times} work
 * from a table of field <i>kinds</i> for the same reason: a field added to the
 * wire later is still summed and scaled correctly.
 */
public final class DailyProjectUsageReport implements OpenPortalType {

    /** The key usage that cannot be attributed to a user is filed under. */
    public static final String UNATTRIBUTED = "unknown";

    /** {@code user → usage}. Remapped when local usernames are rewritten. */
    private static final String[] USAGE_MAPS = {
        "reports", "requeue_reports",
    };

    /** {@code state → usage} - keyed by a Slurm state, so never remapped. */
    private static final String[] STATE_USAGE_MAPS = {
        "requeue_state_usage",
    };

    /** {@code component-or-reservation → user → usage}. Inner keys are users. */
    private static final String[] NESTED_USAGE_MAPS = {
        "components", "requeue_components", "reservation_reports", "reservation_requeue_usage",
    };

    /** {@code user → count}. Remapped with the usage maps. */
    private static final String[] USER_COUNTER_MAPS = {
        "user_job_counts", "user_wait_seconds", "user_expansion_milli", "user_runtime_seconds",
        "user_expansion_jobs", "user_allocated_cpus", "user_allocated_gpus",
        "user_requeue_events", "user_requeue_wait_seconds",
    };

    /** {@code state-or-reservation → count} - not keyed by user. */
    private static final String[] OTHER_COUNTER_MAPS = {
        "requeue_states", "reservation_jobs",
    };

    /** Scalar totals, each shadowing one of the user maps above. */
    private static final String[] COUNTERS = {
        "num_jobs", "total_wait_seconds", "total_expansion_milli", "total_runtime_seconds",
        "num_expansion_jobs", "total_allocated_cpus", "total_allocated_gpus",
        "num_requeue_events", "requeue_wait_seconds",
    };

    /**
     * Which scalar total shadows which map, in the pairs {@link #isConsistent}
     * checks. The invariant is what makes a report auditable: a per-user figure
     * that does not sum to its own total means one of the two was written
     * without the other.
     */
    private static final String[][] SHADOWED = {
        {"user_job_counts", "num_jobs"},
        {"user_wait_seconds", "total_wait_seconds"},
        {"user_expansion_milli", "total_expansion_milli"},
        {"user_runtime_seconds", "total_runtime_seconds"},
        {"user_expansion_jobs", "num_expansion_jobs"},
        {"user_allocated_cpus", "total_allocated_cpus"},
        {"user_allocated_gpus", "total_allocated_gpus"},
        {"user_requeue_events", "num_requeue_events"},
        {"user_requeue_wait_seconds", "requeue_wait_seconds"},
    };

    /** Expansion factors are accumulated in thousandths so that sums are exact. */
    public static final long EXPANSION_SCALE = 1000L;

    private final ObjectNode node;

    /** An empty day. */
    public DailyProjectUsageReport() {
        node = Json.object();
        node.putObject("reports");
        node.put("is_complete", false);
    }

    private DailyProjectUsageReport(ObjectNode node) {
        this.node = node;
    }

    public static DailyProjectUsageReport fromJson(String json) {
        return fromJson(Json.parse(json));
    }

    public static DailyProjectUsageReport fromJson(JsonNode json) {
        if (json == null || json.isNull()) {
            return new DailyProjectUsageReport();
        }

        if (!json.isObject()) {
            throw new IllegalArgumentException("a daily usage report is a JSON object");
        }

        return new DailyProjectUsageReport((ObjectNode) json.deepCopy());
    }

    /** An independent copy. */
    public DailyProjectUsageReport copy() {
        return new DailyProjectUsageReport(node.deepCopy());
    }

    // ---- reading -----------------------------------------------------------

    /** The local users this day has usage for, {@link #UNATTRIBUTED} included. */
    public List<String> localUsers() {
        return keys("reports");
    }

    /** One local user's usage. Zero for a user with none. */
    public Usage usage(String localUser) {
        return Usage.fromJson(map("reports").path(localUser));
    }

    /** Usage that could not be attributed to a user. */
    public Usage unattributedUsage() {
        return usage(UNATTRIBUTED);
    }

    /** Every user's usage summed - the attributed and the unattributed. */
    public Usage totalUsage() {
        return sumUsages("reports");
    }

    /** The resource components this day has a breakdown for ({@code cpu}, {@code gpu}, ...). */
    public List<String> components() {
        return keys("components");
    }

    /**
     * This day as if only one component existed.
     *
     * <p>The component's own per-user figures become the day's
     * {@code reports}, and the counters come across unchanged - a job counted
     * once for the day is still counted once here, because a job is not divided
     * between the components it used.
     */
    public DailyProjectUsageReport getComponent(String component) {
        DailyProjectUsageReport report = copy();
        JsonNode usages = map("components").path(component);

        ObjectNode reports = Json.object();

        if (usages.isObject()) {
            usages.fields().forEachRemaining(
                    entry -> reports.set(entry.getKey(), entry.getValue().deepCopy()));
        }

        report.node.set("reports", reports);
        report.node.remove("components");

        return report;
    }

    public long numJobs() {
        return node.path("num_jobs").asLong();
    }

    public long numJobsForUser(String localUser) {
        return map("user_job_counts").path(localUser).asLong();
    }

    public long totalWaitSeconds() {
        return node.path("total_wait_seconds").asLong();
    }

    public long waitSecondsForUser(String localUser) {
        return map("user_wait_seconds").path(localUser).asLong();
    }

    /** Mean queue wait per job, truncated to whole seconds. Zero for no jobs. */
    public long averageWaitSeconds() {
        long jobs = numJobs();

        return jobs == 0 ? 0 : totalWaitSeconds() / jobs;
    }

    public long averageWaitSecondsForUser(String localUser) {
        long jobs = numJobsForUser(localUser);

        return jobs == 0 ? 0 : waitSecondsForUser(localUser) / jobs;
    }

    /** Usage from attempts superseded by a requeue, which {@link #totalUsage} excludes. */
    public Usage totalRequeueUsage() {
        return sumUsages("requeue_reports");
    }

    /**
     * What the project actually consumed: {@link #totalUsage} plus
     * {@link #totalRequeueUsage}.
     *
     * <p>Slurm keeps one record per <i>attempt</i>, and a requeued job has
     * several. Everything but the last attempt lands in the requeue fields, so
     * {@code totalUsage} alone is the historically reported figure and this is
     * the true one. Which to charge for is a policy question, which is why both
     * are carried.
     */
    public Usage totalUsageIncludingRequeues() {
        return totalUsage().plus(totalRequeueUsage());
    }

    /** Whether the day is closed and will not change again. */
    public boolean isComplete() {
        return node.path("is_complete").asBoolean();
    }

    /**
     * Whether every scalar total equals the sum of the map it shadows.
     *
     * <p>{@code true} for a report whose maps are empty even when the scalars
     * are not: that is what data from an older instance looks like, and it has
     * nothing to check against.
     */
    public boolean isConsistent() {
        for (String[] pair : SHADOWED) {
            JsonNode map = map(pair[0]);

            if (map.isEmpty()) {
                continue;
            }

            if (sumCounters(pair[0]) != node.path(pair[1]).asLong()) {
                return false;
            }
        }

        return true;
    }

    // ---- writing -----------------------------------------------------------

    /** Add to a local user's usage. */
    public DailyProjectUsageReport addUsage(String localUser, Usage usage) {
        ObjectNode reports = mutableMap("reports");
        reports.set(localUser, usage(localUser).plus(usage).toJson());

        return this;
    }

    /** Add usage that belongs to no particular user. */
    public DailyProjectUsageReport addUnattributedUsage(Usage usage) {
        return addUsage(UNATTRIBUTED, usage);
    }

    /** Replace a local user's usage. */
    public DailyProjectUsageReport setUsage(String localUser, Usage usage) {
        mutableMap("reports").set(localUser, usage.toJson());

        return this;
    }

    public DailyProjectUsageReport setUnattributedUsage(Usage usage) {
        return setUsage(UNATTRIBUTED, usage);
    }

    /**
     * Add to one component's share of a user's usage.
     *
     * <p>Independent of {@link #addUsage}: the components are a breakdown the
     * site chooses to publish, and nothing here makes them sum to the total.
     * Report both if you want both.
     */
    public DailyProjectUsageReport addComponentUsage(String component, String localUser, Usage usage) {
        ObjectNode components = mutableMap("components");
        ObjectNode users = component(components, component);
        users.set(localUser, Usage.fromJson(users.path(localUser)).plus(usage).toJson());

        return this;
    }

    public DailyProjectUsageReport addUnattributedComponentUsage(String component, Usage usage) {
        return addComponentUsage(component, UNATTRIBUTED, usage);
    }

    public DailyProjectUsageReport setComponentUsage(String component, String localUser, Usage usage) {
        component(mutableMap("components"), component).set(localUser, usage.toJson());

        return this;
    }

    public DailyProjectUsageReport setUnattributedComponentUsage(String component, Usage usage) {
        return setComponentUsage(component, UNATTRIBUTED, usage);
    }

    /**
     * Add to a user's job count, and to the day's.
     *
     * <p>Both move together, because the scalar total shadows the per-user map -
     * see {@link #isConsistent}. Beyond what the Python module exposes; the
     * field is on the wire either way, and a report that carries usage but no
     * job counts cannot answer "how many jobs".
     */
    public DailyProjectUsageReport addJobs(String localUser, long jobs) {
        return addCounter("user_job_counts", "num_jobs", localUser, jobs);
    }

    /** Add to a user's accumulated queue wait, and to the day's. */
    public DailyProjectUsageReport addWaitSeconds(String localUser, long seconds) {
        return addCounter("user_wait_seconds", "total_wait_seconds", localUser, seconds);
    }

    /** Close the day. */
    public DailyProjectUsageReport setComplete() {
        node.put("is_complete", true);

        return this;
    }

    /** Reopen the day. */
    public DailyProjectUsageReport setIncomplete() {
        node.put("is_complete", false);

        return this;
    }

    // ---- arithmetic --------------------------------------------------------

    /**
     * Two days' figures summed, as a new report.
     *
     * <p>Complete only if both were: a total that includes an open day is
     * itself provisional.
     */
    public DailyProjectUsageReport plus(DailyProjectUsageReport other) {
        DailyProjectUsageReport sum = copy();

        for (String field : USAGE_MAPS) {
            sum.mergeUsages(field, other.map(field));
        }

        for (String field : STATE_USAGE_MAPS) {
            sum.mergeUsages(field, other.map(field));
        }

        for (String field : NESTED_USAGE_MAPS) {
            sum.mergeNestedUsages(field, other.map(field));
        }

        for (String field : USER_COUNTER_MAPS) {
            sum.mergeCounters(field, other.map(field));
        }

        for (String field : OTHER_COUNTER_MAPS) {
            sum.mergeCounters(field, other.map(field));
        }

        for (String field : COUNTERS) {
            long total = Usage.saturatingAdd(
                    sum.node.path(field).asLong(), other.node.path(field).asLong());

            if (total != 0) {
                sum.node.put(field, total);
            }
        }

        sum.node.put("is_complete", isComplete() && other.isComplete());

        return sum;
    }

    /**
     * Every figure scaled, as a new report.
     *
     * <p>Scales the component and requeue breakdowns as well as the totals -
     * leaving them behind is how a scaled report ends up adding two different
     * units together. Each entry is floored independently, and flooring is not
     * distributive over a sum, so a scaled total is not exactly the sum of the
     * scaled parts.
     */
    public DailyProjectUsageReport times(double factor) {
        DailyProjectUsageReport scaled = copy();

        for (String field : USAGE_MAPS) {
            scaled.scaleUsages(field, factor);
        }

        for (String field : STATE_USAGE_MAPS) {
            scaled.scaleUsages(field, factor);
        }

        for (String field : NESTED_USAGE_MAPS) {
            scaled.scaleNestedUsages(field, factor);
        }

        return scaled;
    }

    /** As {@link #times}, dividing. A zero divisor gives zero rather than throwing. */
    public DailyProjectUsageReport dividedBy(double divisor) {
        if (divisor == 0.0) {
            return times(0.0);
        }

        return times(1.0 / divisor);
    }

    /**
     * A copy with local usernames rewritten.
     *
     * <p>Two names mapping onto one are <b>summed</b>, not overwritten - which
     * is the whole point: consolidating two local accounts into one must not
     * lose either one's usage.
     */
    DailyProjectUsageReport remapUsers(Map<String, String> renames) {
        DailyProjectUsageReport remapped = copy();

        for (String field : USAGE_MAPS) {
            remapped.node.set(field, remapUsageMap(remapped.map(field), renames));
        }

        for (String field : NESTED_USAGE_MAPS) {
            JsonNode nested = remapped.map(field);
            ObjectNode result = Json.object();

            nested.fields().forEachRemaining(entry ->
                    result.set(entry.getKey(), remapUsageMap(entry.getValue(), renames)));

            if (!result.isEmpty()) {
                remapped.node.set(field, result);
            }
        }

        for (String field : USER_COUNTER_MAPS) {
            remapped.node.set(field, remapCounterMap(remapped.map(field), renames));
        }

        return remapped;
    }

    // ---- wire form ---------------------------------------------------------

    @Override
    public String typeName() {
        return "DailyProjectUsageReport";
    }

    @Override
    public JsonNode toJson() {
        return node.deepCopy();
    }

    /** Every figure in hours, one line per user. */
    public String inHours() {
        StringBuilder text = new StringBuilder();

        for (String user : localUsers()) {
            text.append(user).append(": ").append(usage(user).inHours()).append('\n');
        }

        return text.toString();
    }

    @Override
    public String toString() {
        StringBuilder text = new StringBuilder();

        for (String user : localUsers()) {
            long jobs = numJobsForUser(user);

            text.append(user).append(": ").append(usage(user));

            if (jobs > 0) {
                text.append(" | ").append(jobs).append(jobs == 1 ? " job" : " jobs")
                        .append(" | Average wait: ")
                        .append(new Usage(averageWaitSecondsForUser(user)));
            }

            text.append('\n');
        }

        return text.toString();
    }

    @Override
    public boolean equals(Object other) {
        return other instanceof DailyProjectUsageReport
                && node.equals(((DailyProjectUsageReport) other).node);
    }

    @Override
    public int hashCode() {
        return node.hashCode();
    }

    // ---- internals ---------------------------------------------------------

    private JsonNode map(String field) {
        JsonNode found = node.path(field);

        return found.isObject() ? found : Json.object();
    }

    private ObjectNode mutableMap(String field) {
        JsonNode found = node.path(field);

        if (found.isObject()) {
            return (ObjectNode) found;
        }

        return node.putObject(field);
    }

    private static ObjectNode component(ObjectNode components, String name) {
        JsonNode found = components.path(name);

        if (found.isObject()) {
            return (ObjectNode) found;
        }

        return components.putObject(name);
    }

    private List<String> keys(String field) {
        List<String> names = new ArrayList<>();
        map(field).fieldNames().forEachRemaining(names::add);
        Collections.sort(names);

        return names;
    }

    private Usage sumUsages(String field) {
        long seconds = 0;

        for (JsonNode value : map(field)) {
            seconds = Usage.saturatingAdd(seconds, Usage.fromJson(value).seconds());
        }

        return new Usage(seconds);
    }

    private long sumCounters(String field) {
        long total = 0;

        for (JsonNode value : map(field)) {
            total = Usage.saturatingAdd(total, value.asLong());
        }

        return total;
    }

    private DailyProjectUsageReport addCounter(
            String mapField, String scalarField, String key, long amount) {
        ObjectNode counters = mutableMap(mapField);
        counters.put(key, Usage.saturatingAdd(counters.path(key).asLong(), amount));
        node.put(scalarField, Usage.saturatingAdd(node.path(scalarField).asLong(), amount));

        return this;
    }

    private void mergeUsages(String field, JsonNode other) {
        if (other.isEmpty()) {
            return;
        }

        ObjectNode target = mutableMap(field);

        other.fields().forEachRemaining(entry -> target.set(entry.getKey(),
                Usage.fromJson(target.path(entry.getKey()))
                        .plus(Usage.fromJson(entry.getValue()))
                        .toJson()));
    }

    private void mergeNestedUsages(String field, JsonNode other) {
        if (other.isEmpty()) {
            return;
        }

        ObjectNode target = mutableMap(field);

        other.fields().forEachRemaining(outer -> {
            ObjectNode inner = component(target, outer.getKey());

            outer.getValue().fields().forEachRemaining(entry -> inner.set(entry.getKey(),
                    Usage.fromJson(inner.path(entry.getKey()))
                            .plus(Usage.fromJson(entry.getValue()))
                            .toJson()));
        });
    }

    private void mergeCounters(String field, JsonNode other) {
        if (other.isEmpty()) {
            return;
        }

        ObjectNode target = mutableMap(field);

        other.fields().forEachRemaining(entry -> target.put(entry.getKey(),
                Usage.saturatingAdd(target.path(entry.getKey()).asLong(),
                        entry.getValue().asLong())));
    }

    private void scaleUsages(String field, double factor) {
        JsonNode found = node.path(field);

        if (!found.isObject()) {
            return;
        }

        ObjectNode target = (ObjectNode) found;
        List<String> names = new ArrayList<>();
        target.fieldNames().forEachRemaining(names::add);

        for (String name : names) {
            target.set(name, Usage.fromJson(target.path(name)).times(factor).toJson());
        }
    }

    private void scaleNestedUsages(String field, double factor) {
        JsonNode found = node.path(field);

        if (!found.isObject()) {
            return;
        }

        List<String> outerNames = new ArrayList<>();
        found.fieldNames().forEachRemaining(outerNames::add);

        for (String outer : outerNames) {
            ObjectNode inner = component((ObjectNode) found, outer);
            List<String> innerNames = new ArrayList<>();
            inner.fieldNames().forEachRemaining(innerNames::add);

            for (String name : innerNames) {
                inner.set(name, Usage.fromJson(inner.path(name)).times(factor).toJson());
            }
        }
    }

    private static ObjectNode remapUsageMap(JsonNode usages, Map<String, String> renames) {
        ObjectNode result = Json.object();

        usages.fields().forEachRemaining(entry -> {
            String user = renames.getOrDefault(entry.getKey(), entry.getKey());

            result.set(user, Usage.fromJson(result.path(user))
                    .plus(Usage.fromJson(entry.getValue()))
                    .toJson());
        });

        return result;
    }

    private static ObjectNode remapCounterMap(JsonNode counters, Map<String, String> renames) {
        ObjectNode result = Json.object();

        counters.fields().forEachRemaining(entry -> {
            String user = renames.getOrDefault(entry.getKey(), entry.getKey());

            result.put(user, Usage.saturatingAdd(
                    result.path(user).asLong(), entry.getValue().asLong()));
        });

        return result;
    }
}
