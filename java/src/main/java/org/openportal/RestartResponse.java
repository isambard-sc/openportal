// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.Optional;

/**
 * The bridge's answer to {@code POST /restart}.
 *
 * <p>The status says whether the restart was <i>accepted</i>, not whether the
 * agent has come back - a restarted agent is unreachable for a moment
 * afterwards by definition.
 */
public record RestartResponse(JsonNode json) {

    public static RestartResponse fromJson(JsonNode node) {
        return new RestartResponse(node == null ? Json.object() : node);
    }

    public String status() {
        return json.path("status").asText();
    }

    public String message() {
        return json.path("message").asText();
    }

    public boolean isOk() {
        return "ok".equalsIgnoreCase(status());
    }

    @Override
    public String toString() {
        return Json.write(json);
    }

    /** Kept for symmetry with the other response types. */
    public Optional<String> detail() {
        String message = message();

        return message.isEmpty() ? Optional.empty() : Optional.of(message);
    }
}
