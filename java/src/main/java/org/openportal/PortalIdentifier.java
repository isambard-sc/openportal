// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * The name of a portal - the last component of every identifier below.
 *
 * <p>A single word, and the only reason it is a type rather than a
 * {@code String} is that a report carries one and the map keys inside it must
 * agree with it (see {@link UsageReport}).
 */
public record PortalIdentifier(String portal) implements OpenPortalType {

    public PortalIdentifier {
        portal = portal.trim();

        if (portal.isEmpty()) {
            throw new IllegalArgumentException("Invalid PortalIdentifier: \"\"");
        }

        if (portal.contains(".")) {
            throw new IllegalArgumentException("Invalid PortalIdentifier: \"" + portal + "\"");
        }
    }

    public static PortalIdentifier parse(String value) {
        return new PortalIdentifier(value);
    }

    @Override
    public String typeName() {
        return "PortalIdentifier";
    }

    @Override
    public com.fasterxml.jackson.databind.JsonNode toJson() {
        return Json.text(toString());
    }

    @Override
    public String toString() {
        return portal;
    }
}
