// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal.example;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.TreeMap;

/**
 * One resource we advertise, the templates it accepts, and the unit conversions
 * agreed for it.
 *
 * <p>The name is the resource's own - {@code cluster1}, not
 * {@code cluster1.site.allocator}. The three-part form is assembled per
 * awarding portal when the set is registered, because the same resource is
 * normally offered to several of them and that is a property of the
 * relationship rather than of the resource.
 */
public final class Offering {

    private final String name;
    private final ObjectNode raw;

    Offering(String name, JsonNode raw) {
        this.name = name;
        this.raw = raw.isObject() ? (ObjectNode) raw.deepCopy() : org.openportal.Json.object();
    }

    public String name() {
        return name;
    }

    JsonNode json() {
        return raw.deepCopy();
    }

    /**
     * The {@code AwardDetails.template} values this resource accepts, sorted.
     *
     * <p>Per-resource, because a template selects things that belong to the
     * resource. An award naming a template this resource does not offer is
     * rejected rather than quietly given a default.
     */
    public List<String> templates() {
        List<String> templates = new ArrayList<>();
        raw.path("templates").forEach(entry -> templates.add(entry.asText()));
        java.util.Collections.sort(templates);

        return templates;
    }

    /**
     * What one of this site's units is worth in an awarding portal's, per unit.
     *
     * <p>{@code {"GPUHR": 4.0}} records an agreement between the two portals:
     * one node hour here is four of their GPU hours. It is an <b>agreement</b>
     * rather than a calculation - neither side derives it from the other's
     * hardware - so it is stored, not computed, and it is per-resource because
     * a node hour on a GPU cluster and one on a CPU cluster are not worth the
     * same credit.
     *
     * <p>Empty is a position rather than a gap: this resource can hold awards
     * allocated in the site's own unit and no others.
     */
    public Map<String, Double> conversions() {
        Map<String, Double> conversions = new TreeMap<>();

        raw.path("conversions").fields().forEachRemaining(entry ->
                conversions.put(entry.getKey(), entry.getValue().asDouble()));

        return conversions;
    }

    /** The day we started advertising it, for the operator's benefit. */
    public Optional<LocalDate> since() {
        return raw.hasNonNull("since")
                ? Optional.of(LocalDate.parse(raw.get("since").asText()))
                : Optional.empty();
    }
}
