// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * Base of the OpenPortal error hierarchy, and the wire encoding both ends of a
 * portal-to-portal exchange agree on.
 *
 * <p>A job is answered either with a result or with an error, and for several
 * instructions <b>the error is the answer</b>: an award awaiting human approval
 * has no mapping to return, only a reason. The awarding portal acts on
 * <i>which</i> error it receives - {@link ManagedProjectPendingError} means "ask
 * me again" and is benign, {@link ManagedProjectRejectedError} means "no" and is
 * terminal - so the class has to survive the trip.
 *
 * <p>A job carries one error string, so the class travels inside it as a
 * {@code "<ClassName>: <message>"} prefix, and the portal agent wraps that once
 * more as {@code RuntimeError{…}} on the way back. {@link #encode} and
 * {@link #decode} are the two ends of that convention, and they mirror
 * {@code python/src/errors.rs} so that a Java portal and a Python one are
 * understood identically.
 *
 * <p>Extends {@link RuntimeException} rather than a checked exception: these are
 * answers to questions, thrown from handlers that a dispatcher is expected to
 * catch wholesale (see the site portal example's {@code answer}).
 */
public class OpenPortalError extends RuntimeException {

    private static final long serialVersionUID = 1L;

    /**
     * The classes that survive a round trip, in the order {@link #decode} tries
     * them. The match is exact on the part before {@code ": "}, so the order is
     * for readability rather than correctness - subclasses sit next to their base.
     */
    private static final String[] CLASSES = {
        "ManagedProjectPendingError",
        "ManagedProjectRejectedError",
        "ManagedProjectPermissionError",
        "OpenPortalUnsupportedCommandError",
        "OpenPortalOtherError",
        "OpenPortalError",
    };

    public OpenPortalError(String message) {
        super(message);
    }

    /** The class name as it goes on the wire. Overridden by nothing - {@code getClass} is enough. */
    public final String wireClass() {
        return getClass().getSimpleName();
    }

    /** The message used when an error is raised without one. */
    static String defaultMessage(String wireClass) {
        return switch (wireClass) {
            case "ManagedProjectPendingError" -> "The project is pending.";
            case "ManagedProjectRejectedError" -> "The project is rejected.";
            case "ManagedProjectPermissionError" -> "The project is not permitted.";
            case "OpenPortalUnsupportedCommandError" -> "The command is not supported.";
            default -> "An unspecified error occurred.";
        };
    }

    /**
     * The wire form of this error: {@code "<ClassName>: <message>"}.
     *
     * <p>A portal that subclasses one of these still reports something
     * intelligible, because the name comes from the class itself - but only the
     * six names above decode back to a specific type, and anything else arrives
     * at the far end as an {@link OpenPortalOtherError} with its text intact.
     */
    public String encode() {
        return encode(wireClass(), getMessage());
    }

    /** As {@link #encode()}, for a class name and message held separately. */
    public static String encode(String wireClass, String message) {
        String trimmed = message == null ? "" : message.trim();

        return wireClass + ": " + (trimmed.isEmpty() ? defaultMessage(wireClass) : trimmed);
    }

    /**
     * Build the error a wire message describes.
     *
     * <p>Unrecognised text is an {@link OpenPortalOtherError} carrying the whole
     * message - nothing is discarded on the guess that it might have been a
     * prefix.
     */
    public static OpenPortalError decode(String raw) {
        String inner = unwrapRuntimeError(raw);

        for (String wireClass : CLASSES) {
            if (!inner.startsWith(wireClass)) {
                continue;
            }

            String rest = inner.substring(wireClass.length());

            // Only a genuine "<class>: <message>" separator counts, so a class
            // name that merely starts the free text is not mistaken for one.
            if (rest.startsWith(": ")) {
                return of(wireClass, rest.substring(2).trim());
            }

            if (rest.isEmpty()) {
                return of(wireClass, defaultMessage(wireClass));
            }
        }

        return new OpenPortalOtherError(inner);
    }

    /**
     * Build the error a structured job error describes.
     *
     * <p>The preferred path when a job carries an {@code error.kind}: the kind was
     * decided by the agent that failed, so nothing here has to read prose. The
     * message is decoded first so that an error built from a kind does not read
     * {@code "ClassName: ClassName: …"}.
     */
    public static OpenPortalError fromKind(String kind, String message) {
        String detail = decode(message == null ? "" : message).getMessage();

        return switch (kind == null ? "" : kind) {
            case "award_pending" -> new ManagedProjectPendingError(detail);
            case "award_rejected" -> new ManagedProjectRejectedError(detail);
            case "award_permission" -> new ManagedProjectPermissionError(detail);
            case "unsupported" -> new OpenPortalUnsupportedCommandError(detail);
            // No class is tied to this kind - a transport kind such as `expired`,
            // or one a future domain added. The prose may still name a class, so
            // defer to it rather than flattening everything to "other".
            default -> decode(message == null ? "" : message);
        };
    }

    private static OpenPortalError of(String wireClass, String message) {
        return switch (wireClass) {
            case "ManagedProjectPendingError" -> new ManagedProjectPendingError(message);
            case "ManagedProjectRejectedError" -> new ManagedProjectRejectedError(message);
            case "ManagedProjectPermissionError" -> new ManagedProjectPermissionError(message);
            case "OpenPortalUnsupportedCommandError" ->
                    new OpenPortalUnsupportedCommandError(message);
            case "OpenPortalError" -> new OpenPortalError(message);
            default -> new OpenPortalOtherError(message);
        };
    }

    /**
     * Strip the {@code RuntimeError{…}} wrapper the portal agent adds, if present.
     *
     * <p>A prefix/suffix match rather than trimming a character set: the inner
     * message can begin with any character, and trimming would eat the start of it.
     */
    private static String unwrapRuntimeError(String raw) {
        String trimmed = raw == null ? "" : raw.trim();

        if (!trimmed.startsWith("RuntimeError{")) {
            return trimmed;
        }

        String inner = trimmed.substring("RuntimeError{".length());

        return (inner.endsWith("}") ? inner.substring(0, inner.length() - 1) : inner).trim();
    }
}
