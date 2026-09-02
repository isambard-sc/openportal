// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * A set of destinations, which has <b>two</b> wire forms - and using the wrong
 * one is a {@code 500} from the bridge or an empty list on the way back.
 *
 * <ul>
 *   <li><b>As JSON</b> - over the HTTP API, and as a job result - it is a plain
 *       array of strings: {@code ["cluster1.site.allocator", "cluster2.site.allocator"]}.
 *       That is {@link #toJson}.
 *   <li><b>As text</b> - inside an instruction string, where a job command is one
 *       line - it is the bracketed form: {@code "[cluster1.site.allocator,
 *       cluster2.site.allocator]"}. That is {@link #toString}.
 * </ul>
 *
 * <p>The text form has one asymmetry worth knowing, and it is faithfully
 * reproduced here because the bridge produces it:
 *
 * <ul>
 *   <li>none is {@code "[]"}
 *   <li>one is the destination <b>on its own</b>, with no brackets at all -
 *       {@code "cluster1.site.allocator"}
 *   <li>several are bracketed and comma-space separated -
 *       {@code "[cluster1.site.allocator, cluster2.site.allocator]"}
 * </ul>
 *
 * <p>{@link #parse} accepts all three, and brackets either way round, so a
 * reader need not care.
 */
public record Destinations(List<Destination> destinations) implements OpenPortalType {

    public Destinations {
        destinations = List.copyOf(destinations);
    }

    public static Destinations of(List<Destination> destinations) {
        return new Destinations(destinations);
    }

    public static Destinations parse(String value) {
        String trimmed = value == null ? "" : value.trim();

        if (trimmed.startsWith("[")) {
            trimmed = trimmed.substring(1);
        }

        if (trimmed.endsWith("]")) {
            trimmed = trimmed.substring(0, trimmed.length() - 1);
        }

        List<Destination> parsed = new ArrayList<>();

        for (String part : Arrays.asList(trimmed.trim().split(","))) {
            String one = part.trim();

            if (!one.isEmpty()) {
                parsed.add(Destination.parse(one));
            }
        }

        return new Destinations(parsed);
    }

    @Override
    public String typeName() {
        return "Destinations";
    }

    /** As JSON: an array of strings, which is how this type travels over HTTP. */
    @Override
    public com.fasterxml.jackson.databind.JsonNode toJson() {
        com.fasterxml.jackson.databind.node.ArrayNode array = Json.array();
        destinations.forEach(destination -> array.add(destination.toString()));

        return array;
    }

    @Override
    public String toString() {
        if (destinations.isEmpty()) {
            return "[]";
        }

        if (destinations.size() == 1) {
            return destinations.get(0).toString();
        }

        List<String> parts = new ArrayList<>(destinations.size());
        destinations.forEach(destination -> parts.add(destination.toString()));

        return "[" + String.join(", ", parts) + "]";
    }
}
