// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * Where a job has got to.
 *
 * <p>Capitalised on the wire - {@code "Pending"}, not {@code "pending"} - which
 * is the sort of thing that silently never matches if you compare strings by
 * hand. {@link #parse} is case-insensitive so it does not matter which spelling
 * a caller has in front of it.
 */
public enum Status {
    CREATED,
    PENDING,
    RUNNING,
    COMPLETE,
    ERROR,
    DUPLICATE;

    /** The wire spelling: initial capital, rest lower case. */
    public String wire() {
        String name = name();

        return name.charAt(0) + name.substring(1).toLowerCase();
    }

    public static Status parse(String value) {
        if (value == null || value.isBlank()) {
            return CREATED;
        }

        return valueOf(value.trim().toUpperCase());
    }

    /** Whether a job in this state will not change again. */
    public boolean isFinished() {
        return this == COMPLETE || this == ERROR;
    }
}
