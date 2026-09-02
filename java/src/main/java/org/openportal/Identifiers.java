// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * The grammar every identifier and mapping component is held to.
 *
 * <p>An allow-list, not a deny-list, and the reason is worth knowing before
 * relaxing it: these names are not only operands handed to spawned tools, they
 * are interpolated into space-delimited OpenPortal instruction strings (where a
 * space shifts every later argument), into {@code sacctmgr} {@code key=value}
 * arguments (where a comma is a list separator), and into Slurm REST URLs
 * (where a {@code ?} starts a query). A deny-list that admits whitespace,
 * {@code ,}, {@code =}, {@code %}, {@code ?} or {@code #} is a hole in all
 * three. Mirrors {@code templemeads/src/validate.rs}.
 */
final class Identifiers {

    /** As {@code MAX_IDENTIFIER_COMPONENT_LEN} - a Unix name has to fit. */
    static final int MAX_COMPONENT_LENGTH = 64;

    private Identifiers() {}

    /** One component of a dotted identifier: alphanumeric, {@code _} and {@code -}. */
    static String component(String value, String field) {
        return validate(value, field, false);
    }

    /**
     * One half of a mapping - a local user or group name, or a Slurm account.
     *
     * <p>As {@link #component} but {@code .} is allowed in the interior, because
     * a local account derived from {@code user.project} is legitimately named
     * {@code user.project}. Not at either end, and never {@code ..}: as a path
     * component those resolve to the current or parent directory.
     */
    static String mappingTarget(String value, String field) {
        String validated = validate(value, field, true);

        if (validated.startsWith(".") || validated.endsWith(".")) {
            throw new IllegalArgumentException(
                    "Invalid " + field + " - cannot start or end with '.' '" + validated + "'");
        }

        if (validated.contains("..")) {
            throw new IllegalArgumentException(
                    "Invalid " + field + " - cannot contain '..' '" + validated + "'");
        }

        return validated;
    }

    private static String validate(String value, String field, boolean allowPeriod) {
        if (value == null) {
            throw new IllegalArgumentException("Invalid identifier - " + field + " cannot be null");
        }

        String trimmed = value.trim();

        if (trimmed.isEmpty()) {
            throw new IllegalArgumentException("Invalid identifier - " + field + " cannot be empty");
        }

        // Bytes, not characters: the Rust side measures a `&str`'s length in
        // UTF-8 bytes, and Java's `length()` counts UTF-16 code units. The
        // charset below admits only ASCII, so the two agree in practice - but
        // the length check runs first, and disagreeing about the limit would
        // let a name through here that the other side rejects.
        int bytes = trimmed.getBytes(java.nio.charset.StandardCharsets.UTF_8).length;

        if (bytes > MAX_COMPONENT_LENGTH) {
            throw new IllegalArgumentException("Invalid identifier - "
                    + field + " is longer than " + MAX_COMPONENT_LENGTH + " characters '"
                    + trimmed + "'");
        }

        if (trimmed.startsWith("-")) {
            throw new IllegalArgumentException(
                    "Invalid identifier - " + field + " cannot start with '-' '" + trimmed + "'");
        }

        for (int i = 0; i < trimmed.length(); i++) {
            char c = trimmed.charAt(i);
            boolean ok = (c >= 'A' && c <= 'Z')
                    || (c >= 'a' && c <= 'z')
                    || (c >= '0' && c <= '9')
                    || c == '_'
                    || c == '-'
                    || (allowPeriod && c == '.');

            if (!ok) {
                throw new IllegalArgumentException("Invalid identifier - " + field
                        + " contains an illegal character '" + c + "' (allowed: A-Z, a-z, 0-9, '_', '-'"
                        + (allowPeriod ? ", '.'" : "") + ") '" + trimmed + "'");
            }
        }

        return trimmed;
    }

    /**
     * Split a dotted identifier into exactly {@code count} components.
     *
     * <p>{@code -1} as the limit so trailing empty components are kept rather
     * than dropped: {@code "project."} has to be a parse error, not a
     * one-component identifier.
     */
    static String[] split(String value, int count, String type) {
        if (value == null) {
            throw new IllegalArgumentException("Invalid " + type + ": null");
        }

        String[] parts = value.trim().split("\\.", -1);

        if (parts.length != count) {
            throw new IllegalArgumentException("Invalid " + type + ": \"" + value + "\"");
        }

        return parts;
    }

    /** As {@link #split}, for the {@code :}-separated mapping forms. */
    static String[] splitMapping(String value, int count, String type) {
        if (value == null) {
            throw new IllegalArgumentException("Invalid " + type + ": null");
        }

        String[] parts = value.trim().split(":", -1);

        if (parts.length != count) {
            throw new IllegalArgumentException("Invalid " + type + ": \"" + value + "\"");
        }

        return parts;
    }
}
