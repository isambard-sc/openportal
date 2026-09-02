// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.util.UUID;

/**
 * A fire-and-forget event from the network.
 *
 * <p>UDP to a {@link Job}'s TCP: no board, no state, no result, and no
 * acknowledgement. Something happened - a user was added, an award was accepted -
 * and you are being told. Nothing is retried on your behalf and nothing notices
 * if you ignore it.
 *
 * <p>Arrives one of two ways. The bridge signals your {@code notification_url}
 * with {@code ?notification_id=<uuid>} and you fetch it
 * ({@link BridgeClient#fetchNotification}), or the whole JSON is posted to you.
 * Either way {@link #eventType} is what to dispatch on and
 * {@link #eventArgument} carries the identifier it is about.
 *
 * <p>The event is an externally-tagged enum on the wire -
 * {@code {"UserAdded": "chris.p.portal"}} - which is why the string form
 * ({@code "user_added chris.p.portal"}) and the JSON form spell it differently.
 * Both are handled here; compare against {@link #eventType}, which is always
 * the snake_case name.
 */
public final class Notification {

    /** The domain whose event vocabulary this client speaks. */
    public static final String DOMAIN = "greatwestern";

    /** The domain release this client was written against. */
    public static final String DOMAIN_VERSION = "0.92.0";

    private final ObjectNode node;

    private Notification(ObjectNode node) {
        this.node = node;
    }

    /**
     * Parse the {@code "<destination> <event> [<argument>]"} string form.
     *
     * <p>Gets a fresh id, since the string form carries none.
     */
    public static Notification parse(String command) {
        if (command == null || command.isBlank()) {
            throw new IllegalArgumentException("a notification needs a destination and an event");
        }

        String[] parts = command.trim().split("\\s+", 3);

        if (parts.length < 2) {
            throw new IllegalArgumentException(
                    "Invalid notification - needs a destination and an event: '" + command + "'");
        }

        String eventType = parts[1];
        String argument = parts.length > 2 ? parts[2] : "";

        ObjectNode node = Json.object();
        node.put("id", UUID.randomUUID().toString());
        node.put("destination", parts[0]);
        node.putObject("event").put(variantFor(eventType), argument);

        // Stamped once, at construction, exactly as the Rust side does - the
        // event vocabulary belongs to a domain, and a peer speaking a different
        // one has no business reading it. Both fields are optional on the wire,
        // so a notification without them is still valid; this is what an
        // agent's own would carry.
        node.put("domain", DOMAIN);
        node.put("domain_version", DOMAIN_VERSION);

        return new Notification(node);
    }

    public static Notification fromJson(String json) {
        return fromJson(Json.parse(json));
    }

    public static Notification fromJson(JsonNode json) {
        if (json == null || !json.isObject()) {
            throw new IllegalArgumentException("a notification is a JSON object");
        }

        return new Notification((ObjectNode) json.deepCopy());
    }

    /** This notification's UUID. For logging - it is not stored anywhere. */
    public String id() {
        return node.path("id").asText();
    }

    /** Where the notification came from, as a dotted path. */
    public String destination() {
        return node.path("destination").asText();
    }

    /** The route, parsed. */
    public Destination destinationPath() {
        return Destination.parse(destination());
    }

    /** The event keyword alone, snake_case - {@code "user_added"}. Dispatch on this. */
    public String eventType() {
        JsonNode event = node.path("event");

        if (event.isTextual()) {
            // The string form, if a peer ever sends one: "user_added <arg>".
            String[] parts = event.asText().trim().split("\\s+", 2);

            return parts[0];
        }

        java.util.Iterator<String> names = event.fieldNames();

        return names.hasNext() ? snakeCase(names.next()) : "";
    }

    /** Everything after the keyword - usually an identifier. Empty if there is none. */
    public String eventArgument() {
        JsonNode event = node.path("event");

        if (event.isTextual()) {
            String[] parts = event.asText().trim().split("\\s+", 2);

            return parts.length > 1 ? parts[1] : "";
        }

        java.util.Iterator<JsonNode> values = event.elements();

        if (!values.hasNext()) {
            return "";
        }

        JsonNode argument = values.next();

        // A `Forward` carries a whole nested notification rather than an
        // identifier, so it has no text form - hand back its JSON.
        return argument.isTextual() ? argument.asText() : Json.write(argument);
    }

    /** The whole event, keyword and argument - {@code "user_added chris.p.portal"}. */
    public String event() {
        String argument = eventArgument();

        return argument.isEmpty() ? eventType() : eventType() + " " + argument;
    }

    /** The argument as a user identifier, for a {@code user_*} event. */
    public UserIdentifier user() {
        return UserIdentifier.parse(eventArgument());
    }

    /** The argument as a project identifier, for a {@code project_*} or {@code award_*} event. */
    public ProjectIdentifier projectOrAward() {
        return ProjectIdentifier.parse(eventArgument());
    }

    /** The domain that authored this event, if it said. */
    public java.util.Optional<String> domain() {
        return node.hasNonNull("domain")
                ? java.util.Optional.of(node.get("domain").asText())
                : java.util.Optional.empty();
    }

    /** That domain's version, if it said. */
    public java.util.Optional<String> domainVersion() {
        return node.hasNonNull("domain_version")
                ? java.util.Optional.of(node.get("domain_version").asText())
                : java.util.Optional.empty();
    }

    public JsonNode toJson() {
        return node.deepCopy();
    }

    /** The string form: {@code "<destination> <event>"}. */
    @Override
    public String toString() {
        return destination() + " " + event();
    }

    @Override
    public boolean equals(Object other) {
        return other instanceof Notification && node.equals(((Notification) other).node);
    }

    @Override
    public int hashCode() {
        return node.hashCode();
    }

    /**
     * The CamelCase variant name a snake_case event goes on the wire as.
     *
     * <p>Only the fifteen the domain defines are accepted; {@code forward} is
     * infrastructure-only and cannot be built from a string. Rejecting an
     * unknown name here rather than CamelCasing it blindly means a typo fails
     * where it is written instead of being delivered as an event nobody
     * handles.
     */
    private static String variantFor(String eventType) {
        return switch (eventType) {
            case "user_added" -> "UserAdded";
            case "user_removed" -> "UserRemoved";
            case "user_changed" -> "UserChanged";
            case "user_blocked" -> "UserBlocked";
            case "user_unblocked" -> "UserUnblocked";
            case "project_added" -> "ProjectAdded";
            case "project_removed" -> "ProjectRemoved";
            case "project_changed" -> "ProjectChanged";
            case "project_blocked" -> "ProjectBlocked";
            case "project_unblocked" -> "ProjectUnblocked";
            case "award_added" -> "AwardAdded";
            case "award_removed" -> "AwardRemoved";
            case "award_changed" -> "AwardChanged";
            case "award_accepted" -> "AwardAccepted";
            case "award_rejected" -> "AwardRejected";
            case "forward" -> throw new IllegalArgumentException(
                    "forward is an infrastructure-only event and cannot be parsed from a string");
            default -> throw new IllegalArgumentException(
                    "Unknown notification event: '" + eventType + "'");
        };
    }

    /** {@code "UserAdded"} to {@code "user_added"}, for whatever arrives. */
    private static String snakeCase(String variant) {
        StringBuilder text = new StringBuilder();

        for (int i = 0; i < variant.length(); i++) {
            char c = variant.charAt(i);

            if (Character.isUpperCase(c)) {
                if (i > 0) {
                    text.append('_');
                }

                text.append(Character.toLowerCase(c));
            } else {
                text.append(c);
            }
        }

        return text.toString();
    }
}
