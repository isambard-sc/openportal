// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.time.Instant;
import java.time.LocalDate;
import java.time.ZoneOffset;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * One project's storage quotas and usage, with a history.
 *
 * <p>Storage is a level, not a total: unlike usage, which accumulates, a quota
 * report is a <b>snapshot</b> of how things stand at {@link #generatedAt}. So
 * the top-level fields always hold the most recent snapshot and older ones live
 * in {@code daily_reports}, keyed by UTC date, at most one per day - the newest
 * seen for that day. The current snapshot's own date is never duplicated in
 * there, which is why {@link #dailyReports} has to stitch the two together.
 *
 * <p>Quotas come in two kinds: project-level, keyed by volume name, and
 * per-user, keyed by portal {@link UserIdentifier} and then by volume. The
 * {@code users} map records what this site calls each of those users, for the
 * same reason a usage report carries one.
 */
public final class ProjectStorageReport implements OpenPortalType {

    private final ObjectNode node;

    public ProjectStorageReport(ProjectIdentifier project) {
        node = Json.object();
        node.put("project", project.toString());
        node.put("generated_at", Times.toJson(Instant.now()));
        node.putObject("project_quotas");
        node.putObject("user_quotas");
        node.putObject("users");
    }

    private ProjectStorageReport(ObjectNode node) {
        this.node = node;
    }

    public static ProjectStorageReport fromJson(String json) {
        return fromJson(Json.parse(json));
    }

    public static ProjectStorageReport fromJson(JsonNode json) {
        if (json == null || !json.isObject()) {
            throw new IllegalArgumentException("a project storage report is a JSON object");
        }

        return new ProjectStorageReport((ObjectNode) json.deepCopy());
    }

    public ProjectStorageReport copy() {
        return new ProjectStorageReport(node.deepCopy());
    }

    // ---- reading -----------------------------------------------------------

    public ProjectIdentifier project() {
        return ProjectIdentifier.parse(node.path("project").asText());
    }

    public PortalIdentifier portal() {
        return project().portalIdentifier();
    }

    /** When the top-level snapshot was taken. */
    public Instant generatedAt() {
        return Times.fromJson(node.path("generated_at"));
    }

    /** The UTC date the top-level snapshot belongs to. */
    public LocalDate date() {
        return generatedAt().atZone(ZoneOffset.UTC).toLocalDate();
    }

    /** Project-level quotas, by volume name. */
    public Map<Volume, Quota> projectQuotas() {
        Map<Volume, Quota> quotas = new LinkedHashMap<>();

        map("project_quotas").fields().forEachRemaining(entry ->
                quotas.put(Volume.parse(entry.getKey()), Quota.fromJson(entry.getValue())));

        return quotas;
    }

    /** Per-user quotas, by user and then by volume. */
    public Map<UserIdentifier, Map<Volume, Quota>> userQuotas() {
        Map<UserIdentifier, Map<Volume, Quota>> quotas = new LinkedHashMap<>();

        map("user_quotas").fields().forEachRemaining(user -> {
            Map<Volume, Quota> volumes = new LinkedHashMap<>();

            user.getValue().fields().forEachRemaining(entry ->
                    volumes.put(Volume.parse(entry.getKey()), Quota.fromJson(entry.getValue())));

            quotas.put(UserIdentifier.parse(user.getKey()), volumes);
        });

        return quotas;
    }

    /** The users this report carries quotas or a mapping for. */
    public List<UserIdentifier> users() {
        java.util.LinkedHashSet<UserIdentifier> users = new java.util.LinkedHashSet<>();
        users.addAll(userMapping().keySet());
        users.addAll(userQuotas().keySet());

        return new ArrayList<>(users);
    }

    /** The portal identifier to local username mapping. */
    public Map<UserIdentifier, String> userMapping() {
        Map<UserIdentifier, String> mapping = new LinkedHashMap<>();

        map("users").fields().forEachRemaining(entry ->
                mapping.put(UserIdentifier.parse(entry.getKey()), entry.getValue().asText()));

        return mapping;
    }

    public boolean isEmpty() {
        return map("project_quotas").isEmpty()
                && map("user_quotas").isEmpty()
                && map("daily_reports").isEmpty();
    }

    /**
     * The snapshot for one date, as a report of its own.
     *
     * <p>The top-level snapshot when {@code date} is its date, a historical
     * entry when there is one, and an empty report otherwise - never null.
     */
    public ProjectStorageReport getReport(LocalDate date) {
        if (date.equals(date())) {
            return withoutHistory();
        }

        JsonNode found = map("daily_reports").path(date.toString());

        if (!found.isObject()) {
            return new ProjectStorageReport(project());
        }

        ObjectNode snapshot = (ObjectNode) found.deepCopy();
        snapshot.set("users", map("users").deepCopy());

        return new ProjectStorageReport(snapshot);
    }

    /**
     * Every snapshot, oldest first, the current one included.
     *
     * <p>{@code withUsageOnly} decides whether days with no quota data are
     * included: {@code false} fills the gaps between the first and last
     * snapshot with empty reports, mirroring
     * {@link ProjectUsageReport#dailyReports(boolean)}.
     */
    public List<ProjectStorageReport> dailyReports(boolean withUsageOnly) {
        List<LocalDate> dates = new ArrayList<>();
        map("daily_reports").fieldNames().forEachRemaining(name -> dates.add(Dates.parse(name)));

        if (!dates.contains(date())) {
            dates.add(date());
        }

        Collections.sort(dates);

        List<LocalDate> wanted = (!withUsageOnly && !dates.isEmpty())
                ? new DateRange(dates.get(0), dates.get(dates.size() - 1)).days()
                : dates;

        List<ProjectStorageReport> reports = new ArrayList<>();
        wanted.forEach(date -> reports.add(getReport(date)));

        return reports;
    }

    public List<ProjectStorageReport> dailyReports() {
        return dailyReports(true);
    }

    // ---- writing -----------------------------------------------------------

    /** Set when this snapshot was taken. */
    public ProjectStorageReport setGeneratedAt(Instant when) {
        node.put("generated_at", Times.toJson(when));

        return this;
    }

    /** Set a project-level quota on a volume. */
    public ProjectStorageReport setProjectQuota(Volume volume, Quota quota) {
        mutableMap("project_quotas").set(volume.name(), quota.toJson());

        return this;
    }

    /** Set one user's quota on a volume. */
    public ProjectStorageReport setUserQuota(UserIdentifier user, Volume volume, Quota quota) {
        ObjectNode quotas = mutableMap("user_quotas");
        JsonNode found = quotas.path(user.toString());
        ObjectNode volumes = found.isObject()
                ? (ObjectNode) found
                : quotas.putObject(user.toString());

        volumes.set(volume.name(), quota.toJson());

        return this;
    }

    /** Record what this site calls one of the portal's users. */
    public ProjectStorageReport addMapping(UserMapping mapping) {
        mutableMap("users").put(mapping.user().toString(), mapping.localUser());

        return this;
    }

    public ProjectStorageReport addMappings(Iterable<UserMapping> mappings) {
        mappings.forEach(this::addMapping);

        return this;
    }

    // ---- reshaping ---------------------------------------------------------

    /** A copy naming a different project, its user keys moved with it. */
    public ProjectStorageReport remapProject(ProjectIdentifier newProject) {
        ProjectStorageReport remapped = copy();
        remapped.node.put("project", newProject.toString());
        remapped.node.set("user_quotas", moveUsers(map("user_quotas"), newProject));
        remapped.node.set("users", moveUsers(map("users"), newProject));

        ObjectNode history = Json.object();

        map("daily_reports").fields().forEachRemaining(entry -> {
            ObjectNode snapshot = (ObjectNode) entry.getValue().deepCopy();
            snapshot.put("project", newProject.toString());
            snapshot.set("user_quotas", moveUsers(snapshot.path("user_quotas"), newProject));
            history.set(entry.getKey(), snapshot);
        });

        if (!history.isEmpty()) {
            remapped.node.set("daily_reports", history);
        }

        return remapped;
    }

    /** A copy under a different portal, the project name unchanged. */
    public ProjectStorageReport remapPortal(PortalIdentifier newPortal) {
        return remapProject(new ProjectIdentifier(project().project(), newPortal.portal()));
    }

    /** A copy with the local usernames in the mapping rewritten. */
    public ProjectStorageReport remapUsers(Map<UserIdentifier, String> newMapping) {
        ProjectStorageReport remapped = copy();
        ObjectNode users = remapped.mutableMap("users");

        newMapping.forEach((user, localUser) -> users.put(user.toString(), localUser));

        return remapped;
    }

    /**
     * A copy holding only the snapshots inside {@code range}.
     *
     * <p>The newest surviving snapshot becomes the top-level one, so the result
     * is a report in its own right rather than one with a current snapshot from
     * outside its own range.
     */
    public ProjectStorageReport filter(DateRange range) {
        List<ProjectStorageReport> kept = new ArrayList<>();

        for (ProjectStorageReport snapshot : dailyReports()) {
            if (range.contains(snapshot.date())) {
                kept.add(snapshot);
            }
        }

        if (kept.isEmpty()) {
            return new ProjectStorageReport(project());
        }

        ProjectStorageReport newest = kept.remove(kept.size() - 1).copy();

        for (ProjectStorageReport older : kept) {
            newest.mutableMap("daily_reports")
                    .set(older.date().toString(), older.withoutHistory().node);
        }

        return newest;
    }

    /**
     * Two snapshot histories merged, as a new report.
     *
     * <p>Where both have a snapshot for the same day, the <b>newer</b> wins -
     * these are levels, not amounts, so summing them would be meaningless.
     */
    public ProjectStorageReport plus(ProjectStorageReport other) {
        if (!project().equals(other.project())) {
            throw new IllegalArgumentException(
                    "Cannot combine storage reports for different projects: " + project()
                            + " and " + other.project());
        }

        Map<LocalDate, ProjectStorageReport> byDate = new java.util.TreeMap<>();

        for (ProjectStorageReport snapshot : dailyReports()) {
            byDate.put(snapshot.date(), snapshot);
        }

        for (ProjectStorageReport snapshot : other.dailyReports()) {
            ProjectStorageReport existing = byDate.get(snapshot.date());

            if (existing == null || !snapshot.generatedAt().isBefore(existing.generatedAt())) {
                byDate.put(snapshot.date(), snapshot);
            }
        }

        List<ProjectStorageReport> snapshots = new ArrayList<>(byDate.values());
        ProjectStorageReport combined = snapshots.remove(snapshots.size() - 1).copy();

        for (ProjectStorageReport older : snapshots) {
            combined.mutableMap("daily_reports")
                    .set(older.date().toString(), older.withoutHistory().node);
        }

        other.map("users").fields().forEachRemaining(entry ->
                combined.mutableMap("users").put(entry.getKey(), entry.getValue().asText()));

        return combined;
    }

    /** This report wrapped in a portal-level one. */
    public StorageReport toStorageReport() {
        StorageReport report = new StorageReport(portal());
        report.setReport(this);

        return report;
    }

    public static ProjectStorageReport combine(List<ProjectStorageReport> reports) {
        if (reports.isEmpty()) {
            throw new IllegalArgumentException("No reports to combine");
        }

        ProjectStorageReport combined = reports.get(0).copy();

        for (int i = 1; i < reports.size(); i++) {
            combined = combined.plus(reports.get(i));
        }

        return combined;
    }

    // ---- wire form ---------------------------------------------------------

    @Override
    public String typeName() {
        return "ProjectStorageReport";
    }

    @Override
    public JsonNode toJson() {
        return node.deepCopy();
    }

    @Override
    public String toString() {
        return Json.write(node);
    }

    @Override
    public boolean equals(Object other) {
        return other instanceof ProjectStorageReport
                && node.equals(((ProjectStorageReport) other).node);
    }

    @Override
    public int hashCode() {
        return node.hashCode();
    }

    // ---- internals ---------------------------------------------------------

    /** This snapshot alone, with the history stripped off. */
    private ProjectStorageReport withoutHistory() {
        ObjectNode snapshot = node.deepCopy();
        snapshot.remove("daily_reports");

        return new ProjectStorageReport(snapshot);
    }

    private static ObjectNode moveUsers(JsonNode users, ProjectIdentifier newProject) {
        ObjectNode moved = Json.object();

        users.fields().forEachRemaining(entry -> {
            UserIdentifier user = UserIdentifier.parse(entry.getKey());
            UserIdentifier newUser = new UserIdentifier(
                    user.username(), newProject.project(), newProject.portal());

            moved.set(newUser.toString(), entry.getValue().deepCopy());
        });

        return moved;
    }

    private JsonNode map(String field) {
        JsonNode found = node.path(field);

        return found.isObject() ? found : Json.object();
    }

    private ObjectNode mutableMap(String field) {
        JsonNode found = node.path(field);

        return found.isObject() ? (ObjectNode) found : node.putObject(field);
    }
}
