// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;

/**
 * A value that can be the result of a job.
 *
 * <p>Two things travel with a result and both matter: its JSON, and the
 * <b>name of its type</b>. The awarding portal deserialises the one against the
 * other, so a {@code ProjectMapping} returned as a bare string with no
 * {@code result_type} is not the same answer - see
 * {@code site-portal-api.md} §3.2, and §4 for the type each instruction must
 * return.
 *
 * <p>{@link #typeName} is the name the Rust side registers, which is not always
 * the name of the class it comes from: {@code AwardDetails} is
 * {@code "ProjectDetails"} on the wire, because the type was called that first.
 * Implementations state their own, so the wire name is never guessed from the
 * Java class.
 */
public interface OpenPortalType {

    /** The {@code result_type} this value goes on the wire as. */
    String typeName();

    /** This value as JSON, which becomes the (string-encoded) {@code result}. */
    JsonNode toJson();
}
