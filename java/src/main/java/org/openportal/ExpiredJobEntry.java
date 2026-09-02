// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.time.Instant;

/**
 * A job that expired before anything answered it, deduplicated with a count.
 *
 * <p>Not the same as a failure: nobody said no, nobody said anything. A site
 * portal that appears here is one that did not answer inside the two-minute
 * expiry - see {@code site-portal-api.md} on the answer budget.
 */
public record ExpiredJobEntry(JsonNode json) {

    public String destination() {
        return json.path("destination").asText();
    }

    public String instruction() {
        return json.path("instruction").asText();
    }

    public Instant createdAt() {
        return Times.fromJson(json.path("created_at"));
    }

    public Instant expiredAt() {
        return Times.fromJson(json.path("expired_at"));
    }

    public long count() {
        return json.path("count").asLong();
    }

    @Override
    public String toString() {
        return destination() + " " + instruction() + " (×" + count() + ") expired at "
                + expiredAt();
    }
}
