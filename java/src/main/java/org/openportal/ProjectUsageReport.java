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
import java.util.Optional;
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

    /** A report whose usage is in {@code unit} - what new code should use. */
    public ProjectUsageReport(ProjectIdentifier project, UsageUnit unit) {
        this(project);
        node.set("usage_unit", unit.toJson());
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

        // A component's figures are in the component's unit.
        componentUnit(component).ifPresent(unit -> report.node.set("usage_unit", unit.toJson()));

        for (LocalDate date : dates()) {
            report.putReport(date, getReport(date).getComponent(component));
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

    /** Replace one day's report. Refused until the report has a unit - see {@link #requireUnit}. */
    public ProjectUsageReport setReport(LocalDate date, DailyProjectUsageReport report) {
        requireUnit();

        return putReport(date, report);
    }

    private ProjectUsageReport putReport(LocalDate date, DailyProjectUsageReport report) {
        mutableReports().set(date.toString(), report.toJson());

        return this;
    }

    /**
     * Add to one day's report, summing with whatever is already there.
     *
     * <p>A day with no report yet takes this one as it is, rather than summing
     * it with an empty day: an empty day is not complete, so the sum would
     * have turned a complete day incomplete. That made every day that
     * {@link #plus} and {@link #combine} brought in from a second report
     * incomplete, unlike the Rust side, which inserts it unchanged.
     */
    public ProjectUsageReport addReport(LocalDate date, DailyProjectUsageReport report) {
        requireUnit();

        return mergeReport(date, report);
    }

    private ProjectUsageReport mergeReport(LocalDate date, DailyProjectUsageReport report) {
        if (!reports().path(date.toString()).isObject()) {
            return putReport(date, report.copy());
        }

        return putReport(date, getReport(date).plus(report));
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
        return putReport(date, getReport(date).setComplete());
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
            remapped.putReport(date, getReport(date).remapUsers(renames));
        }

        ObjectNode users = remapped.mutableUserMap();
        newMapping.forEach((user, localUser) -> users.put(user.toString(), localUser));

        return remapped;
    }

    /** A copy holding only the days inside {@code range}. */
    public ProjectUsageReport filter(DateRange range) {
        ProjectUsageReport filtered = new ProjectUsageReport(project());
        filtered.node.set("users", userMapNode().deepCopy());
        filtered.copyUnitsFrom(this);

        for (LocalDate date : dates()) {
            if (range.contains(date)) {
                filtered.putReport(date, getReport(date));
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

        MergedUnits units = mergedUnits(other);
        ProjectUsageReport sum = copy();

        for (LocalDate date : other.dates()) {
            sum.mergeReport(date, other.getReport(date));
        }

        other.userMapNode().fields().forEachRemaining(entry ->
                sum.mutableUserMap().put(entry.getKey(), entry.getValue().asText()));

        sum.node.remove("usage_unit");
        units.unit().ifPresent(unit -> sum.node.set("usage_unit", unit.toJson()));
        sum.writeComponentUnits(units.components());

        return sum;
    }

    /** Every day's figures scaled. */
    public ProjectUsageReport times(double factor) {
        ProjectUsageReport scaled = copy();

        for (LocalDate date : dates()) {
            scaled.putReport(date, getReport(date).times(factor));
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

    // ---- units -------------------------------------------------------------

    /**
     * The unit this report's usage is in. Empty only for a report from before
     * units, whose figures were 3600 counts per hour of the award's unit; it
     * can be declared with {@link #setUnit} or {@link #setTimeUnit}.
     */
    public Optional<UsageUnit> unit() {
        JsonNode found = node.path("usage_unit");

        return found.isObject() ? Optional.of(UsageUnit.fromJson(found)) : Optional.empty();
    }

    /**
     * The unit, or an exception saying none is set. Figures can be added only to
     * a report that says what they are in: one from before units can still be
     * read, totalled and passed on, but must have its unit declared first.
     */
    public UsageUnit requireUnit() {
        return unit().orElseThrow(() -> new IllegalStateException("The usage report for "
                + project() + " has no unit. Set one with setUnit (or setTimeUnit for a "
                + "report from before units, which were all time) before adding figures"));
    }

    /**
     * Declare the unit of this report. Allowed only while it has none; on a
     * report from before units it declares what the figures already mean, and
     * changes none of them. Use {@link #convert} to change a unit.
     */
    public ProjectUsageReport setUnit(UsageUnit unit) {
        unit().ifPresent(existing -> {
            throw new IllegalStateException("The usage report for " + project()
                    + " is already in " + existing.name() + ". Use convert to change it");
        });

        node.set("usage_unit", unit.toJson());
        writeComponentUnits(componentUnits());

        return this;
    }

    /**
     * Declare that this report is in the time unit {@code name} at 3600 counts
     * per hour - what every report from before units was, in the unit of its
     * award's allocation. Refuses a currency, and a report with a unit.
     */
    public ProjectUsageReport setTimeUnit(String name) {
        UsageUnit unit = UsageUnit.parse(name);

        if (!unit.isTime()) {
            throw new IllegalArgumentException("'" + name + "' is not a time unit. Use setUnit "
                    + "for a currency or a unit of your own");
        }

        return setUnit(unit);
    }

    /** The unit {@code component} is in: its own if it has one, else the report's. */
    public Optional<UsageUnit> componentUnit(String component) {
        JsonNode own = node.path("component_units").path(component);

        return own.isObject() ? Optional.of(UsageUnit.fromJson(own)) : unit();
    }

    /** The components with a unit of their own. */
    public Map<String, UsageUnit> componentUnits() {
        Map<String, UsageUnit> units = new TreeMap<>();

        node.path("component_units").fields().forEachRemaining(entry ->
                units.put(entry.getKey(), UsageUnit.fromJson(entry.getValue())));

        return units;
    }

    /**
     * Give {@code component} a unit of its own - GPU hours beside a report in
     * GBP, say. Refused if the component already has figures in a different
     * unit, which would relabel them.
     */
    public ProjectUsageReport setComponentUnit(String component, UsageUnit unit) {
        Optional<UsageUnit> current = componentUnit(component);

        if (current.isPresent() && !current.get().equals(unit) && components().contains(component)) {
            throw new IllegalStateException("Component '" + component + "' of the usage report "
                    + "for " + project() + " already has figures in " + current.get().name());
        }

        Map<String, UsageUnit> units = componentUnits();
        units.put(component, unit);
        writeComponentUnits(units);

        return this;
    }

    /** The total usage in the report's unit, or empty for a report from before units. */
    public Optional<Amount> total() {
        return unit().map(unit -> new Amount(totalUsage(), unit));
    }

    /** The total of one component, in that component's unit. */
    public Optional<Amount> componentTotal(String component) {
        return componentUnit(component)
                .map(unit -> new Amount(getComponent(component).totalUsage(), unit));
    }

    /**
     * Scale only the consumption figures - usage, requeue, charged-requeue and
     * reservation - leaving the component breakdowns, each in its own unit,
     * alone. The unit is unchanged: to change it, use {@link #convert}.
     */
    public ProjectUsageReport scaleTotal(double factor) {
        ProjectUsageReport scaled = copy();

        for (LocalDate date : dates()) {
            scaled.putReport(date, getReport(date).scaleTotals(factor));
        }

        return scaled;
    }

    /**
     * This report re-expressed in {@code to}, as a new report: every
     * consumption figure is multiplied by {@code factor} - how many of
     * {@code to} one of the current unit is worth - and moved to the new
     * resolution, rounding once. A component that was inheriting the old unit
     * is given it explicitly first, so none is relabelled.
     */
    public ProjectUsageReport convert(UsageUnit to, double factor) {
        UsageUnit from = requireUnit();

        if (Double.isNaN(factor) || Double.isInfinite(factor) || factor < 0.0) {
            throw new IllegalArgumentException("Cannot convert the usage report for " + project()
                    + " by a factor of " + factor);
        }

        Map<String, UsageUnit> pinned = componentUnits();
        components().forEach(component -> pinned.putIfAbsent(component, from));

        double scale = factor * to.countsPerUnit() / from.countsPerUnit();
        ProjectUsageReport converted = scaleTotal(scale);
        converted.node.set("usage_unit", to.toJson());
        converted.writeComponentUnits(pinned);

        return converted;
    }

    /** The units of the sum of two reports: see {@link #mergedUnits}. */
    private record MergedUnits(Optional<UsageUnit> unit, Map<String, UsageUnit> components) {}

    /**
     * The units of this report plus {@code other}, or an exception saying why
     * they cannot be summed: the units must match - or both be absent, so two
     * reports from before units still combine - a report with no figures takes
     * the other's, and a component named by both must be in one unit.
     */
    private MergedUnits mergedUnits(ProjectUsageReport other) {
        Optional<UsageUnit> mine = unit();
        Optional<UsageUnit> theirs = other.unit();
        Optional<UsageUnit> unit;

        if (mine.equals(theirs)) {
            unit = mine;
        } else if (other.isBare()) {
            unit = mine.or(() -> theirs);
        } else if (isBare()) {
            unit = theirs.or(() -> mine);
        } else {
            throw new IllegalArgumentException("Cannot combine usage reports for " + project()
                    + " in different units: " + mine.map(UsageUnit::toString).orElse("no unit")
                    + " and " + theirs.map(UsageUnit::toString).orElse("no unit"));
        }

        java.util.Set<String> ours = namedComponents();
        java.util.Set<String> yours = other.namedComponents();
        java.util.Set<String> all = new java.util.TreeSet<>(ours);
        all.addAll(yours);

        Map<String, UsageUnit> components = new TreeMap<>();

        for (String component : all) {
            Optional<UsageUnit> a = componentUnit(component);
            Optional<UsageUnit> b = other.componentUnit(component);

            if (ours.contains(component) && yours.contains(component) && !a.equals(b)) {
                throw new IllegalArgumentException("Cannot combine usage reports for "
                        + project() + ": component '" + component + "' is in "
                        + a.map(UsageUnit::toString).orElse("no unit") + " in one and "
                        + b.map(UsageUnit::toString).orElse("no unit") + " in the other");
            }

            Optional<UsageUnit> effective = ours.contains(component) ? a : b;

            if (effective.isPresent() && !effective.equals(unit)) {
                components.put(component, effective.get());
            }
        }

        return new MergedUnits(unit, components);
    }

    /** No figures and no component units: a report that can take any unit. */
    private boolean isBare() {
        return reports().isEmpty() && componentUnits().isEmpty();
    }

    private java.util.Set<String> namedComponents() {
        java.util.Set<String> names = new java.util.TreeSet<>(components());
        names.addAll(componentUnits().keySet());

        return names;
    }

    /** Write the component units, leaving out any equal to the report's unit. */
    private void writeComponentUnits(Map<String, UsageUnit> units) {
        Optional<UsageUnit> unit = unit();
        ObjectNode written = Json.object();

        units.forEach((component, componentUnit) -> {
            if (unit.isEmpty() || !unit.get().equals(componentUnit)) {
                written.set(component, componentUnit.toJson());
            }
        });

        if (written.isEmpty()) {
            node.remove("component_units");
        } else {
            node.set("component_units", written);
        }
    }

    private void copyUnitsFrom(ProjectUsageReport other) {
        other.unit().ifPresent(unit -> node.set("usage_unit", unit.toJson()));
        writeComponentUnits(other.componentUnits());
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
