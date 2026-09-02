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
 * Every project's storage, for one portal.
 *
 * <p>The type {@code get_storage_report} returns: a
 * {@link ProjectStorageReport} per project, keyed by project identifier. As
 * with {@link UsageReport}, every key must end in this report's own portal, and
 * that is checked when one is read from the wire rather than trusted.
 */
public final class StorageReport implements OpenPortalType {

    private final ObjectNode node;

    public StorageReport(PortalIdentifier portal) {
        node = Json.object();
        node.put("portal", portal.portal());
        node.putObject("reports");
    }

    private StorageReport(ObjectNode node) {
        this.node = node;
    }

    public static StorageReport fromJson(String json) {
        return fromJson(Json.parse(json));
    }

    public static StorageReport fromJson(JsonNode json) {
        if (json == null || !json.isObject()) {
            throw new IllegalArgumentException("a storage report is a JSON object");
        }

        StorageReport report = new StorageReport((ObjectNode) json.deepCopy());

        for (ProjectIdentifier project : report.projects()) {
            report.requireOurPortal(project);
        }

        return report;
    }

    public StorageReport copy() {
        return new StorageReport(node.deepCopy());
    }

    // ---- reading -----------------------------------------------------------

    public PortalIdentifier portal() {
        return new PortalIdentifier(node.path("portal").asText());
    }

    public List<ProjectIdentifier> projects() {
        List<String> names = new ArrayList<>();
        reports().fieldNames().forEachRemaining(names::add);
        Collections.sort(names);

        List<ProjectIdentifier> projects = new ArrayList<>();
        names.forEach(name -> projects.add(ProjectIdentifier.parse(name)));

        return projects;
    }

    /** One project's report. An empty one for a project not covered. */
    public ProjectStorageReport getReport(ProjectIdentifier project) {
        JsonNode found = reports().path(project.toString());

        return found.isObject()
                ? ProjectStorageReport.fromJson(found)
                : new ProjectStorageReport(project);
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

    // ---- writing -----------------------------------------------------------

    public StorageReport setReport(ProjectStorageReport report) {
        requireOurPortal(report.project());
        mutableReports().set(report.project().toString(), report.toJson());

        return this;
    }

    /** Add to one project's report, merging snapshot histories. */
    public StorageReport addReport(ProjectStorageReport report) {
        requireOurPortal(report.project());

        if (!reports().has(report.project().toString())) {
            return setReport(report);
        }

        return setReport(getReport(report.project()).plus(report));
    }

    // ---- reshaping ---------------------------------------------------------

    public StorageReport remapPortal(PortalIdentifier newPortal) {
        StorageReport remapped = new StorageReport(newPortal);

        for (ProjectIdentifier project : projects()) {
            remapped.setReport(getReport(project).remapPortal(newPortal));
        }

        return remapped;
    }

    public StorageReport remapProject(ProjectIdentifier oldProject, ProjectIdentifier newProject) {
        if (!reports().has(oldProject.toString())) {
            return copy();
        }

        requireOurPortal(newProject);

        StorageReport remapped = copy();
        remapped.mutableReports().remove(oldProject.toString());
        remapped.setReport(getReport(oldProject).remapProject(newProject));

        return remapped;
    }

    public StorageReport remapUsers(Map<UserIdentifier, String> newMapping) {
        StorageReport remapped = new StorageReport(portal());

        for (ProjectIdentifier project : projects()) {
            remapped.setReport(getReport(project).remapUsers(newMapping));
        }

        return remapped;
    }

    public StorageReport filter(DateRange range) {
        StorageReport filtered = new StorageReport(portal());

        for (ProjectIdentifier project : projects()) {
            filtered.setReport(getReport(project).filter(range));
        }

        return filtered;
    }

    public StorageReport plus(StorageReport other) {
        if (!portal().equals(other.portal())) {
            throw new IllegalArgumentException(
                    "Cannot combine reports from incompatible portals: " + portal()
                            + " and " + other.portal());
        }

        StorageReport sum = copy();

        for (ProjectIdentifier project : other.projects()) {
            sum.addReport(other.getReport(project));
        }

        return sum;
    }

    public static StorageReport combine(List<StorageReport> reports) {
        if (reports.isEmpty()) {
            throw new IllegalArgumentException("No reports to combine");
        }

        StorageReport combined = reports.get(0).copy();

        for (int i = 1; i < reports.size(); i++) {
            combined = combined.plus(reports.get(i));
        }

        return combined;
    }

    // ---- wire form ---------------------------------------------------------

    @Override
    public String typeName() {
        return "StorageReport";
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
        return other instanceof StorageReport && node.equals(((StorageReport) other).node);
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
