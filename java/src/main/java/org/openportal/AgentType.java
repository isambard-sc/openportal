// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * What kind of agent something is.
 *
 * <p>The position an agent occupies in the hierarchy, which decides what it is
 * allowed to do rather than what software it runs. {@link #VIRTUAL} is the odd
 * one: an offering registered by a portal, which is an address rather than a
 * process.
 */
public enum AgentType {
    PORTAL,
    PROVIDER,
    PLATFORM,
    INSTANCE,
    BRIDGE,
    ACCOUNT,
    FILESYSTEM,
    SCHEDULER,
    VIRTUAL,

    /** Not one of the above - a type this client predates. */
    UNKNOWN;

    /** Capitalised on the wire - {@code "Portal"}, not {@code "portal"}. */
    public String wire() {
        String name = name();

        return name.charAt(0) + name.substring(1).toLowerCase(java.util.Locale.ROOT);
    }

    /** {@link #UNKNOWN} rather than an exception for a name this client does not know. */
    public static AgentType parse(String value) {
        if (value == null || value.isBlank()) {
            return UNKNOWN;
        }

        for (AgentType type : values()) {
            if (type.name().equalsIgnoreCase(value.trim())) {
                return type;
            }
        }

        return UNKNOWN;
    }

    @Override
    public String toString() {
        return wire();
    }
}
