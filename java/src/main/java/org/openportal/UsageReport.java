// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Every project's usage, for one portal.
 *
 * <p>The top of the usage report tree, and the type {@code get_usage_report}
 * returns. A {@link ProjectUsageReport} per project, keyed by project
 * identifier.
 *
 * <p>Every project's identifier must end in this report's own portal. That is
 * enforced here rather than left to the caller: a report whose keys disagree
 * with its {@code portal} field is one a receiver that trusts the keys will
 * mis-attribute, and the same invariant is checked on the Rust side when a
 * report arrives from the wire.
 */
public final class UsageReport implements OpenPortalType {

    private final ObjectNode node;

    public UsageReport(PortalIdentifier portal) {
        node = Json.object();
        node.put("portal", portal.portal());
        node.putObject("reports");
    }

    private UsageReport(ObjectNode node) {
        this.node = node;
    }

    public static UsageReport fromJson(String json) {
        return fromJson(Json.parse(json));
    }

    public static UsageReport fromJson(JsonNode json) {
        if (json == null || !json.isObject()) {
            throw new IllegalArgumentException("a usage report is a JSON object");
        }

        UsageReport report = new UsageReport((ObjectNode) json.deepCopy());

        // Checked on the way in, not only on the way out: a wire-supplied
        // report can carry keys that disagree with its own portal field.
        for (ProjectIdentifier project : report.projects()) {
            report.requireOurPortal(project);
        }

        return report;
    }

    public UsageReport copy() {
        return new UsageReport(node.deepCopy());
    }

    // ---- reading -----------------------------------------------------------

    public PortalIdentifier portal() {
        return new PortalIdentifier(node.path("portal").asText());
    }

    /** The projects this report covers. */
    public List<ProjectIdentifier> projects() {
        List<String> names = new ArrayList<>();
        reports().fieldNames().forEachRemaining(names::add);
        Collections.sort(names);

        List<ProjectIdentifier> projects = new ArrayList<>();
        names.forEach(name -> projects.add(ProjectIdentifier.parse(name)));

        return projects;
    }

    /** One project's report. An empty one for a project not covered. */
    public ProjectUsageReport getReport(ProjectIdentifier project) {
        JsonNode found = reports().path(project.toString());

        return found.isObject()
                ? ProjectUsageReport.fromJson(found)
                : new ProjectUsageReport(project);
    }

    public boolean isEmpty() {
        return reports().isEmpty();
    }

    /** Every project's user mapping, flattened into one. */
    public Map<UserIdentifier, String> userMapping() {
        Map<UserIdentifier, String> mapping = new LinkedHashMap<>();

        for (ProjectIdentifier project : projects()) {
            mapping.putAll(getReport(project).userMapping());
        }

        return mapping;
    }

    /** The resource components any project in this report breaks usage down by. */
    public List<String> components() {
        java.util.SortedSet<String> names = new java.util.TreeSet<>();
        projects().forEach(project -> names.addAll(getReport(project).components()));

        return new ArrayList<>(names);
    }

    public Usage totalUsage() {
        long seconds = 0;

        for (ProjectIdentifier project : projects()) {
            seconds = Usage.saturatingAdd(seconds, getReport(project).totalUsage().seconds());
        }

        return new Usage(seconds);
    }

    // ---- writing -----------------------------------------------------------

    /** Add or replace one project's report. */
    public UsageReport setReport(ProjectUsageReport report) {
        requireOurPortal(report.project());
        mutableReports().set(report.project().toString(), report.toJson());

        return this;
    }

    /** Add to one project's report, summing with whatever is already there. */
    public UsageReport addReport(ProjectUsageReport report) {
        requireOurPortal(report.project());

        return setReport(getReport(report.project()).plus(report));
    }

    // ---- reshaping ---------------------------------------------------------

    /** This report as if only one component existed. */
    public UsageReport getComponent(String component) {
        UsageReport report = new UsageReport(portal());

        for (ProjectIdentifier project : projects()) {
            report.setReport(getReport(project).getComponent(component));
        }

        return report;
    }

    /**
     * A copy under a different portal.
     *
     * <p>Every project key moves too - they have to, since a project identifier
     * ends in its portal's name.
     */
    public UsageReport remapPortal(PortalIdentifier newPortal) {
        UsageReport remapped = new UsageReport(newPortal);

        for (ProjectIdentifier project : projects()) {
            remapped.setReport(getReport(project).remapPortal(newPortal));
        }

        return remapped;
    }

    /** A copy in which one project is renamed. Does nothing if it is not here. */
    public UsageReport remapProject(ProjectIdentifier oldProject, ProjectIdentifier newProject) {
        if (!reports().has(oldProject.toString())) {
            return copy();
        }

        requireOurPortal(newProject);

        UsageReport remapped = copy();
        remapped.mutableReports().remove(oldProject.toString());
        remapped.setReport(getReport(oldProject).remapProject(newProject));

        return remapped;
    }

    /** A copy with local usernames rewritten across every project. */
    public UsageReport remapUsers(Map<UserIdentifier, String> newMapping) {
        UsageReport remapped = new UsageReport(portal());

        for (ProjectIdentifier project : projects()) {
            remapped.setReport(getReport(project).remapUsers(newMapping));
        }

        return remapped;
    }

    /** A copy holding only the days inside {@code range}. */
    public UsageReport filter(DateRange range) {
        UsageReport filtered = new UsageReport(portal());

        for (ProjectIdentifier project : projects()) {
            filtered.setReport(getReport(project).filter(range));
        }

        return filtered;
    }

    /** Two reports for the same portal, summed project by project. */
    public UsageReport plus(UsageReport other) {
        if (!portal().equals(other.portal())) {
            throw new IllegalArgumentException(
                    "Cannot combine reports from incompatible portals: " + portal()
                            + " and " + other.portal());
        }

        UsageReport sum = copy();

        for (ProjectIdentifier project : other.projects()) {
            sum.addReport(other.getReport(project));
        }

        return sum;
    }

    /** Every project's figures scaled. */
    public UsageReport times(double factor) {
        UsageReport scaled = new UsageReport(portal());

        for (ProjectIdentifier project : projects()) {
            scaled.setReport(getReport(project).times(factor));
        }

        return scaled;
    }

    public UsageReport dividedBy(double divisor) {
        if (divisor == 0.0) {
            return times(0.0);
        }

        return times(1.0 / divisor);
    }

    public static UsageReport combine(List<UsageReport> reports) {
        if (reports.isEmpty()) {
            throw new IllegalArgumentException("No reports to combine");
        }

        UsageReport combined = reports.get(0).copy();

        for (int i = 1; i < reports.size(); i++) {
            combined = combined.plus(reports.get(i));
        }

        return combined;
    }

    // ---- wire form ---------------------------------------------------------

    @Override
    public String typeName() {
        return "UsageReport";
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
        return other instanceof UsageReport && node.equals(((UsageReport) other).node);
    }

    @Override
    public int hashCode() {
        return node.hashCode();
    }

    // ---- internals ---------------------------------------------------------

    private void requireOurPortal(ProjectIdentifier project) {
        String ours = node.path("portal").asText();

        if (!project.portal().equals(ours)) {
            throw new IllegalArgumentException("Project '" + project
                    + "' does not belong to portal '" + ours + "'");
        }
    }

    private JsonNode reports() {
        JsonNode found = node.path("reports");

        return found.isObject() ? found : Json.object();
    }

    private ObjectNode mutableReports() {
        JsonNode found = node.path("reports");

        return found.isObject() ? (ObjectNode) found : node.putObject("reports");
    }
}
