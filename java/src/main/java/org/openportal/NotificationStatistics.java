// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;

/**
 * How many notifications an agent has received, sent and failed to send.
 *
 * <p>{@link #totalFailed} counts the ones that failed after every retry - a
 * notification is fire-and-forget, so a failure here is a delivery nobody was
 * told about.
 */
public record NotificationStatistics(JsonNode json) {

    public long totalReceived() {
        return json.path("total_received").asLong();
    }

    public long totalSent() {
        return json.path("total_sent").asLong();
    }

    public long totalFailed() {
        return json.path("total_failed").asLong();
    }

    @Override
    public String toString() {
        return "received " + totalReceived() + ", sent " + totalSent()
                + ", failed " + totalFailed();
    }
}
