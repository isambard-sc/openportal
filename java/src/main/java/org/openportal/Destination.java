// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;

/**
 * A route through the agent network, written as dot-separated agent names.
 *
 * <p>A destination starts with the sender and ends with the thing being
 * addressed, so {@code allocator.site.cluster1} is "from {@code allocator},
 * through {@code site}, to {@code cluster1}". An offering is *registered* the
 * other way round, as {@code cluster1.site.allocator} - the resource, offered by
 * us, to them - and the two forms being reversed catches everybody once.
 *
 * <p>For a site portal the two that matter are the job's own destination
 * ({@code site.<bridge>.cluster1}, ending in the resource) and its
 * {@code forwarded_for} ({@code allocator.site.cluster1}, starting with the
 * portal that asked and ending with the offering it came in through). The last
 * element of either names the resource, which is what
 * {@link #last} is for.
 */
public record Destination(List<String> agents) {

    public Destination {
        agents = List.copyOf(agents);

        if (agents.isEmpty()) {
            throw new IllegalArgumentException("a destination has at least one agent");
        }
    }

    public static Destination parse(String value) {
        return new Destination(Arrays.asList(value.trim().split("\\.")));
    }

    public String first() {
        return agents.get(0);
    }

    public String last() {
        return agents.get(agents.size() - 1);
    }

    /** The route back the way it came. */
    public Destination reverse() {
        List<String> backwards = new ArrayList<>(agents);
        Collections.reverse(backwards);

        return new Destination(backwards);
    }

    @Override
    public String toString() {
        return String.join(".", agents);
    }
}
