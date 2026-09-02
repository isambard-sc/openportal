// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal.example;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import org.openportal.Json;

/**
 * One of <i>our</i> projects, and the usage recorded against it.
 *
 * <p>Not an OpenPortal concept - the awarding portal never sees this record, and
 * never learns our project identifier except as the second half of a
 * {@code ProjectMapping}. It exists because usage is a property of the project
 * and only a property of an award <i>derivatively</i>, by way of which award was
 * attached on which day.
 */
public final class LocalProject {

    private final String localProjectId;
    private final ObjectNode raw;

    LocalProject(String localProjectId, JsonNode raw) {
        this.localProjectId = localProjectId;
        this.raw = raw.isObject() ? (ObjectNode) raw.deepCopy() : Json.object();
    }

    public String localProjectId() {
        return localProjectId;
    }

    /**
     * Usage by date, then by member email, in <b>hours of the site's own
     * unit</b>.
     *
     * <p>Pushed in by the operator's own parsers, which identify the project by
     * our identifier - they have never heard of the awarding portal's.
     * Translating between the two is what the mapping is for.
     */
    public Map<LocalDate, Map<String, Double>> usage() {
        Map<LocalDate, Map<String, Double>> usage = new TreeMap<>();

        raw.path("usage").fields().forEachRemaining(day -> {
            Map<String, Double> perUser = new LinkedHashMap<>();
            day.getValue().fields().forEachRemaining(entry ->
                    perUser.put(entry.getKey(), entry.getValue().asDouble()));

            usage.put(LocalDate.parse(day.getKey()), perUser);
        });

        return usage;
    }

    /** Replace the recorded figures wholesale, as a push does. */
    public LocalProject setUsage(Map<LocalDate, Map<String, Double>> usage) {
        ObjectNode node = Json.object();

        new TreeMap<>(usage).forEach((date, perUser) -> {
            ObjectNode day = node.putObject(date.toString());
            perUser.forEach(day::put);
        });

        raw.set("usage", node);

        return this;
    }

    /**
     * The months whose accounting the site has declared final, as
     * {@code "YYYY-MM"}.
     *
     * <p>This is the operations team's lever over how long the allocator keeps
     * asking. A month listed here is reported with {@code is_complete} set and
     * the allocator stops re-requesting it; a month absent from here is reported
     * incomplete and keeps being asked for.
     *
     * <p>A property of the project rather than of the award, for the same reason
     * usage is: "August is settled" is a statement about the site's accounting,
     * and it stays true across a change of award.
     */
    public List<String> finalMonths() {
        List<String> months = new ArrayList<>();
        raw.path("final_months").forEach(entry -> months.add(entry.asText()));

        return months;
    }

    public LocalProject setFinal(String month, boolean isFinal) {
        java.util.SortedSet<String> months = new java.util.TreeSet<>(finalMonths());

        if (isFinal) {
            months.add(month);
        } else {
            months.remove(month);
        }

        var array = raw.putArray("final_months");
        months.forEach(array::add);

        return this;
    }

    JsonNode json() {
        return raw.deepCopy();
    }
}
