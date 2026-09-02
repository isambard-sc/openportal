// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal.example;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;
import org.openportal.AwardDetails;
import org.openportal.Json;

/**
 * One award as this portal holds it.
 *
 * <p>{@link #details} is the {@code AwardDetails} exactly as the awarding
 * portal sent it, merged across updates. {@link #state} is <b>ours alone</b> and
 * never goes on the wire - the awarding portal learns about it only through
 * which error we answer with.
 */
public final class Award {

    /** Waiting for a human. {@code create_award} answers pending. */
    public static final String PENDING = "pending";

    /** Approved and attached to a project. {@code create_award} answers the mapping. */
    public static final String APPROVED = "approved";

    /** Refused for good. {@code create_award} answers rejected. */
    public static final String REJECTED = "rejected";

    private final String offering;
    private final String projectId;
    private final ObjectNode raw;

    Award(String offering, String projectId, JsonNode raw) {
        this.offering = offering;
        this.projectId = projectId;
        this.raw = raw.isObject() ? (ObjectNode) raw : Json.object();
    }

    /**
     * The resource this award is for - the virtual agent it arrived through.
     *
     * <p>Half of this award's identity, not an attribute of it.
     */
    public String offering() {
        return offering;
    }

    /** What the awarding portal calls this award. */
    public String projectId() {
        return projectId;
    }

    public String state() {
        return raw.path("state").asText(PENDING);
    }

    void setState(String state) {
        raw.put("state", state);
    }

    /** Why it is pending or rejected - the text we put in the error. */
    public String reason() {
        return raw.path("reason").asText("");
    }

    void setReason(String reason) {
        raw.put("reason", reason);
    }

    public AwardDetails details() {
        return AwardDetails.fromJson(raw.path("details"));
    }

    void setDetails(AwardDetails details) {
        raw.set("details", details.toJson());
    }

    /** The offering path the request arrived through, for the record. */
    public Optional<String> forwardedFor() {
        return raw.hasNonNull("forwarded_for")
                ? Optional.of(raw.get("forwarded_for").asText())
                : Optional.empty();
    }

    /**
     * Every period this award has been attached to a project of ours, oldest
     * first.
     *
     * <p><b>A history rather than a single field</b>, which is the shape the
     * billing rule needs. An award can be detached and re-attached, and can be
     * moved from one project to another, and each of those episodes owns its own
     * days. Collapsing it to one "attached from" date would silently disown
     * every day before the latest attachment.
     */
    public List<Attachment> attachments() {
        List<Attachment> attachments = new ArrayList<>();
        attachmentsArray().forEach(entry -> attachments.add(new Attachment(entry)));

        return attachments;
    }

    ArrayNode attachmentsArray() {
        JsonNode found = raw.path("attachments");

        return found.isArray() ? (ArrayNode) found : raw.putArray("attachments");
    }

    /** The open attachment, if this award is attached to anything now. */
    public Optional<Attachment> currentAttachment() {
        for (Attachment attachment : attachments()) {
            if (attachment.to().isEmpty()) {
                return Optional.of(attachment);
            }
        }

        return Optional.empty();
    }

    /**
     * <b>Our own project identifier for this award</b> - {@code myproject1.site}
     * - while it is attached, and empty when it is not.
     *
     * <p>This is our half of the mapping, and the single most important thing
     * this record holds. It is empty until the award is approved, because until
     * then no project of ours is attached to name, and empty again after
     * {@code remove_award}, because the link has been severed. Two things hang
     * off it: it goes back to the awarding portal in the {@code ProjectMapping},
     * and it is the identifier our own accounting records usage against.
     *
     * <p>A detached award reports empty here but keeps its {@link #attachments},
     * so the days it owned stay reportable. Use {@link #projectsEverAttached}
     * for the historical question.
     */
    public Optional<String> localProjectId() {
        return currentAttachment().map(Attachment::project);
    }

    /** Every project of ours this award has been attached to, in order. */
    public List<String> projectsEverAttached() {
        List<String> seen = new ArrayList<>();

        for (Attachment attachment : attachments()) {
            if (!seen.contains(attachment.project())) {
                seen.add(attachment.project());
            }
        }

        return seen;
    }

    /** Whether this award is currently connected to a project of ours. */
    public boolean isAttached() {
        return currentAttachment().isPresent();
    }

    /**
     * An award's identity: the offering <b>and</b> the identifier.
     *
     * <p>Records are read fresh from disk on every access, so two objects
     * describing the same award are different objects - identity has to be
     * compared on the key.
     */
    public String key() {
        return offering + "/" + projectId;
    }

    JsonNode json() {
        return raw;
    }

    @Override
    public String toString() {
        return projectId + " on " + offering + " (" + state() + ")";
    }
}
