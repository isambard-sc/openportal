// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.Optional;

/**
 * The bridge's answer to {@code GET /health}.
 *
 * <p>A status word plus, when the bridge could reach the network, the tree of
 * {@link HealthInfo}. Note the field is {@code health} on the wire while the
 * accessor is {@link #detail} - and it is absent when the bridge itself is up
 * but the agents behind it are not, so {@code isHealthy() == false} with no
 * detail at all is a normal answer, not a malformed one.
 */
public record Health(JsonNode json) {

    public static Health fromJson(JsonNode node) {
        return new Health(node == null ? Json.object() : node);
    }

    public String status() {
        return json.path("status").asText();
    }

    public boolean isHealthy() {
        return "ok".equalsIgnoreCase(status());
    }

    /** The agent tree, or empty when the bridge could not reach it. */
    public Optional<HealthInfo> detail() {
        JsonNode found = json.path("health");

        return found.isObject() ? Optional.of(new HealthInfo(found)) : Optional.empty();
    }

    @Override
    public String toString() {
        return Json.write(json);
    }
}
