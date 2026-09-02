// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * The machine-readable {@code kind} that sits beside a failure's prose.
 *
 * <p>Agents branch on the kind rather than reading the message, so it is what
 * makes "ask me again" and "no" distinguishable without parsing English. A site
 * portal reports failures as class names (see {@link OpenPortalError}), and this
 * is the one place a class name becomes a kind - mirroring
 * {@code greatwestern::errorkind::classify} so both sides agree by construction.
 *
 * <p>These strings go on the wire and peers branch on them: treat a change as
 * breaking.
 */
public final class JobErrorKind {

    /** The award was accepted but is not in place yet. Not a fault; ask again. */
    public static final String AWARD_PENDING = "award_pending";

    /** The award was refused. Re-sending it unchanged will be refused again. */
    public static final String AWARD_REJECTED = "award_rejected";

    /** An award decision with no more specific kind. */
    public static final String AWARD_PERMISSION = "award_permission";

    /** The instruction is not implemented here. */
    public static final String UNSUPPORTED = "unsupported";

    /** Something failed while running, with nothing more specific to say. */
    public static final String RUN = "run";

    /** Nothing recognised the failure. */
    public static final String UNKNOWN = "unknown";

    /**
     * The class-name prefixes a site portal reports failures with, and the kind
     * each maps to.
     *
     * <p>Ordered so a subclass is tried before the base it shares a prefix with:
     * {@code ManagedProjectRejectedError} and {@code ManagedProjectPermissionError}
     * both begin {@code ManagedProject}, and only an exact match on the whole
     * class name separates them.
     */
    private static final String[][] CLASS_KINDS = {
        {"ManagedProjectPendingError", AWARD_PENDING},
        {"ManagedProjectRejectedError", AWARD_REJECTED},
        {"ManagedProjectPermissionError", AWARD_PERMISSION},
        {"OpenPortalUnsupportedCommandError", UNSUPPORTED},
    };

    private JobErrorKind() {}

    /**
     * The kind a failure message describes.
     *
     * <p>A bare class name, or the specified {@code "<class>: <message>"} form.
     * Nothing else counts: a class name that merely opens some free text is not a
     * classification. Anything unrecognised is {@link #UNKNOWN}, which is what
     * the transport would have inferred anyway.
     */
    public static String classify(String message) {
        String trimmed = message == null ? "" : message.trim();

        for (String[] entry : CLASS_KINDS) {
            String className = entry[0];

            if (trimmed.equals(className)) {
                return entry[1];
            }

            if (trimmed.startsWith(className) && trimmed.startsWith(": ", className.length())) {
                return entry[1];
            }
        }

        return UNKNOWN;
    }
}
