// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;

/**
 * One project's usage over a span of days.
 *
 * <p>This is what a site portal builds to answer {@code get_usage_report}: a
 * {@link DailyProjectUsageReport} per day, plus the mapping from the portal's
 * {@link UserIdentifier}s to the site's own local usernames. The mapping is the
 * part that is easy to leave out and expensive to leave out - the daily reports
 * are keyed by local username, and without a mapping the awarding portal cannot
 * attribute a single figure to a person it knows about. Whatever it cannot map
 * shows up in {@link #unmappedUsers} and {@link #unmappedUsage}.
 *
 * <p>Two conventions matter when filling one in:
 *
 * <ul>
 *   <li><b>The day is the site's, and it is a convention.</b> Attribute a job
 *       to a day and stick to the same rule; an allocator reading two sites
 *       cannot tell that one bills by start and the other by end.
 *   <li><b>The unit is the award's, not yours.</b> Convert on the way out - see
 *       {@link Allocation}.
 * </ul>
 *
 * <p>{@link #isComplete} is a promise, not a status: a day marked complete will
 * not be asked for again, so mark one only once it will not change.
 */
public final class ProjectUsageReport implements OpenPortalType {

    private final ObjectNode node;

    public ProjectUsageReport(ProjectIdentifier project) {
        node = Json.object();
        node.put("project", project.toString());
        node.putObject("reports");

        // Written even when empty: release 0.92.0 cannot read a report without
        // `users`, so omitting it is not a smaller payload but an unreadable one.
        node.putObject("users");
    }

    private ProjectUsageReport(ObjectNode node) {
        this.node = node;
    }

    public static ProjectUsageReport fromJson(String json) {
        return fromJson(Json.parse(json));
    }

    public static ProjectUsageReport fromJson(JsonNode json) {
        if (json == null || !json.isObject()) {
            throw new IllegalArgumentException("a project usage report is a JSON object");
        }

        return new ProjectUsageReport((ObjectNode) json.deepCopy());
    }

    public ProjectUsageReport copy() {
        return new ProjectUsageReport(node.deepCopy());
    }

    // ---- reading -----------------------------------------------------------

    public ProjectIdentifier project() {
        return ProjectIdentifier.parse(node.path("project").asText());
    }

    public PortalIdentifier portal() {
        return project().portalIdentifier();
    }

    /** The days this report carries a report for, oldest first. */
    public List<LocalDate> dates() {
        List<LocalDate> dates = new ArrayList<>();
        reports().fieldNames().forEachRemaining(name -> dates.add(Dates.parse(name)));
        Collections.sort(dates);

        return dates;
    }

    /** One day's report. An empty one for a day with nothing recorded. */
    public DailyProjectUsageReport getReport(LocalDate date) {
        JsonNode found = reports().path(date.toString());

        return found.isObject()
                ? DailyProjectUsageReport.fromJson(found)
                : new DailyProjectUsageReport();
    }

    /**
     * Every day's report.
     *
     * <p>{@code withUsageOnly} decides whether days with nothing recorded are
     * included: {@code false} fills in the gaps between the first and last day
     * with empty reports, which is what a caller drawing a chart wants.
     */
    public List<DailyProjectUsageReport> dailyReports(boolean withUsageOnly) {
        List<LocalDate> dates = dates();

        if (withUsageOnly || dates.isEmpty()) {
            List<DailyProjectUsageReport> daily = new ArrayList<>();
            dates.forEach(date -> daily.add(getReport(date)));

            return daily;
        }

        List<DailyProjectUsageReport> daily = new ArrayList<>();

        for (LocalDate date : new DateRange(dates.get(0), dates.get(dates.size() - 1)).days()) {
            daily.add(getReport(date));
        }

        return daily;
    }

    public List<DailyProjectUsageReport> dailyReports() {
        return dailyReports(true);
    }

    /** The resource components any day in this report breaks usage down by. */
    public List<String> components() {
        java.util.SortedSet<String> names = new java.util.TreeSet<>();
        dailyReports().forEach(daily -> names.addAll(daily.components()));

        return new ArrayList<>(names);
    }

    /** This report as if only one component existed. */
    public ProjectUsageReport getComponent(String component) {
        ProjectUsageReport report = new ProjectUsageReport(project());
        report.node.set("users", userMapNode().deepCopy());

        for (LocalDate date : dates()) {
            report.setReport(date, getReport(date).getComponent(component));
        }

        return report;
    }

    /** The portal identifier to local username mapping this report carries. */
    public Map<UserIdentifier, String> userMapping() {
        Map<UserIdentifier, String> mapping = new LinkedHashMap<>();

        userMapNode().fields().forEachRemaining(entry ->
                mapping.put(UserIdentifier.parse(entry.getKey()), entry.getValue().asText()));

        return mapping;
    }

    /** The users this report can attribute usage to. */
    public List<UserIdentifier> users() {
        return new ArrayList<>(userMapping().keySet());
    }

    /**
     * Local usernames that appear in the daily reports with no mapping.
     *
     * <p>Their usage is real and counted in {@link #totalUsage}, but the
     * awarding portal cannot say whose it is. {@link DailyProjectUsageReport#UNATTRIBUTED}
     * is included - it is unmapped by construction.
     */
    public List<String> unmappedUsers() {
        java.util.SortedSet<String> mapped = new java.util.TreeSet<>(userMapping().values());
        java.util.SortedSet<String> unmapped = new java.util.TreeSet<>();

        for (DailyProjectUsageReport daily : dailyReports()) {
            for (String user : daily.localUsers()) {
                if (!mapped.contains(user)) {
                    unmapped.add(user);
                }
            }
        }

        return new ArrayList<>(unmapped);
    }

    /** One user's usage across every day, found through the mapping. */
    public Usage usage(UserIdentifier user) {
        String localUser = userMapping().get(user);

        if (localUser == null) {
            return Usage.ZERO;
        }

        return localUsage(localUser);
    }

    /** One local username's usage across every day. */
    public Usage localUsage(String localUser) {
        long seconds = 0;

        for (DailyProjectUsageReport daily : dailyReports()) {
            seconds = Usage.saturatingAdd(seconds, daily.usage(localUser).seconds());
        }

        return new Usage(seconds);
    }

    /** The usage no mapped user accounts for. */
    public Usage unmappedUsage() {
        long seconds = 0;

        for (String user : unmappedUsers()) {
            seconds = Usage.saturatingAdd(seconds, localUsage(user).seconds());
        }

        return new Usage(seconds);
    }

    public Usage totalUsage() {
        long seconds = 0;

        for (DailyProjectUsageReport daily : dailyReports()) {
            seconds = Usage.saturatingAdd(seconds, daily.totalUsage().seconds());
        }

        return new Usage(seconds);
    }

    public long numJobs() {
        long jobs = 0;

        for (DailyProjectUsageReport daily : dailyReports()) {
            jobs = Usage.saturatingAdd(jobs, daily.numJobs());
        }

        return jobs;
    }

    public long totalWaitSeconds() {
        long waits = 0;

        for (DailyProjectUsageReport daily : dailyReports()) {
            waits = Usage.saturatingAdd(waits, daily.totalWaitSeconds());
        }

        return waits;
    }

    public long averageWaitSeconds() {
        long jobs = numJobs();

        return jobs == 0 ? 0 : totalWaitSeconds() / jobs;
    }

    /** Whether every day in the report is closed. An empty report is complete. */
    public boolean isComplete() {
        for (DailyProjectUsageReport daily : dailyReports()) {
            if (!daily.isComplete()) {
                return false;
            }
        }

        return true;
    }

    // ---- writing -----------------------------------------------------------

    /** Replace one day's report. */
    public ProjectUsageReport setReport(LocalDate date, DailyProjectUsageReport report) {
        mutableReports().set(date.toString(), report.toJson());

        return this;
    }

    /** Add to one day's report, summing with whatever is already there. */
    public ProjectUsageReport addReport(LocalDate date, DailyProjectUsageReport report) {
        return setReport(date, getReport(date).plus(report));
    }

    /** Record what this site calls one of the portal's users. */
    public ProjectUsageReport addMapping(UserMapping mapping) {
        mutableUserMap().put(mapping.user().toString(), mapping.localUser());

        return this;
    }

    public ProjectUsageReport addMappings(Iterable<UserMapping> mappings) {
        mappings.forEach(this::addMapping);

        return this;
    }

    /** Close every day in the report. */
    public ProjectUsageReport setComplete() {
        for (LocalDate date : dates()) {
            setDayComplete(date);
        }

        return this;
    }

    /** Close one day. */
    public ProjectUsageReport setDayComplete(LocalDate date) {
        return setReport(date, getReport(date).setComplete());
    }

    /** Rename the project this report is about. */
    public ProjectUsageReport setProject(ProjectIdentifier project) {
        node.put("project", project.toString());

        return this;
    }

    // ---- reshaping ---------------------------------------------------------

    /**
     * A copy naming a different project.
     *
     * <p>What a portal does on the way out: the site's own project identifier
     * is replaced by the one the awarding portal knows, and the per-user
     * mapping keys are re-pointed at the new project too, since a
     * {@link UserIdentifier} names a user <i>within</i> a project.
     */
    public ProjectUsageReport remapProject(ProjectIdentifier newProject) {
        ProjectUsageReport remapped = copy();
        remapped.setProject(newProject);

        ObjectNode users = Json.object();

        userMapNode().fields().forEachRemaining(entry -> {
            UserIdentifier user = UserIdentifier.parse(entry.getKey());
            UserIdentifier moved = new UserIdentifier(
                    user.username(), newProject.project(), newProject.portal());

            users.put(moved.toString(), entry.getValue().asText());
        });

        remapped.node.set("users", users);

        return remapped;
    }

    /** A copy under a different portal, the project name unchanged. */
    public ProjectUsageReport remapPortal(PortalIdentifier newPortal) {
        return remapProject(new ProjectIdentifier(project().project(), newPortal.portal()));
    }

    /**
     * A copy with the local usernames rewritten.
     *
     * <p>The new mapping is {@code UserIdentifier → local username}; every day's
     * figures move with it, and two names collapsing into one are summed rather
     * than one overwriting the other.
     */
    public ProjectUsageReport remapUsers(Map<UserIdentifier, String> newMapping) {
        Map<String, String> renames = new TreeMap<>();
        Map<UserIdentifier, String> existing = userMapping();

        newMapping.forEach((user, localUser) -> {
            String was = existing.get(user);

            if (was != null && !was.equals(localUser)) {
                renames.put(was, localUser);
            }
        });

        ProjectUsageReport remapped = new ProjectUsageReport(project());

        for (LocalDate date : dates()) {
            remapped.setReport(date, getReport(date).remapUsers(renames));
        }

        ObjectNode users = remapped.mutableUserMap();
        newMapping.forEach((user, localUser) -> users.put(user.toString(), localUser));

        return remapped;
    }

    /** A copy holding only the days inside {@code range}. */
    public ProjectUsageReport filter(DateRange range) {
        ProjectUsageReport filtered = new ProjectUsageReport(project());
        filtered.node.set("users", userMapNode().deepCopy());

        for (LocalDate date : dates()) {
            if (range.contains(date)) {
                filtered.setReport(date, getReport(date));
            }
        }

        return filtered;
    }

    /** Two reports for the same project, summed day by day. */
    public ProjectUsageReport plus(ProjectUsageReport other) {
        if (!project().equals(other.project())) {
            throw new IllegalArgumentException(
                    "Cannot combine reports for different projects: " + project()
                            + " and " + other.project());
        }

        ProjectUsageReport sum = copy();

        for (LocalDate date : other.dates()) {
            sum.addReport(date, other.getReport(date));
        }

        other.userMapNode().fields().forEachRemaining(entry ->
                sum.mutableUserMap().put(entry.getKey(), entry.getValue().asText()));

        return sum;
    }

    /** Every day's figures scaled. */
    public ProjectUsageReport times(double factor) {
        ProjectUsageReport scaled = copy();

        for (LocalDate date : dates()) {
            scaled.setReport(date, getReport(date).times(factor));
        }

        return scaled;
    }

    public ProjectUsageReport dividedBy(double divisor) {
        if (divisor == 0.0) {
            return times(0.0);
        }

        return times(1.0 / divisor);
    }

    /** This report wrapped in a portal-level one. */
    public UsageReport toUsageReport() {
        UsageReport report = new UsageReport(portal());
        report.setReport(this);

        return report;
    }

    public static ProjectUsageReport combine(List<ProjectUsageReport> reports) {
        if (reports.isEmpty()) {
            throw new IllegalArgumentException("No reports to combine");
        }

        ProjectUsageReport combined = reports.get(0).copy();

        for (int i = 1; i < reports.size(); i++) {
            combined = combined.plus(reports.get(i));
        }

        return combined;
    }

    // ---- wire form ---------------------------------------------------------

    @Override
    public String typeName() {
        return "ProjectUsageReport";
    }

    @Override
    public JsonNode toJson() {
        return node.deepCopy();
    }

    public String inHours() {
        StringBuilder text = new StringBuilder();

        for (LocalDate date : dates()) {
            text.append(date).append(":\n").append(getReport(date).inHours());
        }

        return text.toString();
    }

    @Override
    public String toString() {
        return Json.write(node);
    }

    @Override
    public boolean equals(Object other) {
        return other instanceof ProjectUsageReport
                && node.equals(((ProjectUsageReport) other).node);
    }

    @Override
    public int hashCode() {
        return node.hashCode();
    }

    // ---- internals ---------------------------------------------------------

    private JsonNode reports() {
        JsonNode found = node.path("reports");

        return found.isObject() ? found : Json.object();
    }

    private ObjectNode mutableReports() {
        JsonNode found = node.path("reports");

        return found.isObject() ? (ObjectNode) found : node.putObject("reports");
    }

    private JsonNode userMapNode() {
        JsonNode found = node.path("users");

        return found.isObject() ? found : Json.object();
    }

    private ObjectNode mutableUserMap() {
        JsonNode found = node.path("users");

        return found.isObject() ? (ObjectNode) found : node.putObject("users");
    }
}
