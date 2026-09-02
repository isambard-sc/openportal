// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;

/**
 * The name of a storage volume - {@code "home"}, {@code "scratch"},
 * {@code "project"}.
 *
 * <p>A bare string on the wire, and used as a map key inside a storage report.
 * No spaces, because it is interpolated into space-delimited instruction
 * strings.
 */
public record Volume(String name) implements OpenPortalType {

    public Volume {
        name = name == null ? "" : name.trim();

        if (name.isEmpty()) {
            throw new IllegalArgumentException("Volume name cannot be empty");
        }

        if (name.contains(" ")) {
            throw new IllegalArgumentException(
                    "Volume name '" + name + "' cannot contain spaces");
        }
    }

    public static Volume parse(String value) {
        return new Volume(value);
    }

    @Override
    public String typeName() {
        return "Volume";
    }

    @Override
    public JsonNode toJson() {
        return Json.text(name);
    }

    @Override
    public String toString() {
        return name;
    }
}
