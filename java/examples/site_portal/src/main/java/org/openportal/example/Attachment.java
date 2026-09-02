// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal.example;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.time.LocalDate;
import java.util.Optional;

/**
 * One period during which an award was attached to one of our projects.
 *
 * <p>Both ends are <b>inclusive days</b>, and {@code to} is absent while the
 * attachment is open. Inclusive at the far end is the point: an award detached
 * <i>during</i> a day was attached during that day, and still owns it.
 */
public final class Attachment {

    private final ObjectNode raw;

    Attachment(JsonNode raw) {
        this.raw = (ObjectNode) raw;
    }

    /** Our {@code ProjectIdentifier} for the project that was attached. */
    public String project() {
        return raw.path("project").asText();
    }

    /** The first day this attachment covers. */
    public LocalDate since() {
        return LocalDate.parse(raw.path("since").asText());
    }

    /** The last day it covers, or empty while it is still open. */
    public Optional<LocalDate> to() {
        return raw.hasNonNull("to")
                ? Optional.of(LocalDate.parse(raw.get("to").asText()))
                : Optional.empty();
    }

    /** Whether this attachment was in force on {@code date}. */
    public boolean covers(LocalDate date) {
        Optional<LocalDate> end = to();

        return !since().isAfter(date) && (end.isEmpty() || !end.get().isBefore(date));
    }

    void close(LocalDate on) {
        raw.put("to", on.toString());
    }

    JsonNode json() {
        return raw.deepCopy();
    }
}
