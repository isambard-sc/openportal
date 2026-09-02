// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

/**
 * The JSON in one place, so the rest of the client is not littered with checked
 * exceptions.
 *
 * <p>One mapper, configured once. Two of its settings matter: non-ASCII is
 * emitted as UTF-8 rather than escaped (which is what the bridge signs - see
 * {@link BridgeAuth}), and nothing is pretty-printed, because a body's bytes are
 * what the signature covers and reformatting it after signing would break it.
 */
public final class Json {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    private Json() {}

    public static JsonNode parse(String json) {
        try {
            return MAPPER.readTree(json);
        } catch (JsonProcessingException e) {
            throw new OpenPortalOtherError("could not parse JSON: " + e.getOriginalMessage());
        }
    }

    public static String write(JsonNode node) {
        try {
            return MAPPER.writeValueAsString(node);
        } catch (JsonProcessingException e) {
            throw new OpenPortalOtherError("could not write JSON: " + e.getOriginalMessage());
        }
    }

    /** Serialise any object - a map, a list, a record - to JSON text. */
    public static String write(Object value) {
        try {
            return MAPPER.writeValueAsString(value);
        } catch (JsonProcessingException e) {
            throw new OpenPortalOtherError("could not write JSON: " + e.getOriginalMessage());
        }
    }

    public static ObjectNode object() {
        return MAPPER.createObjectNode();
    }

    public static ArrayNode array() {
        return MAPPER.createArrayNode();
    }

    /** A JSON string node, for a type whose whole wire form is a string. */
    public static JsonNode text(String value) {
        return MAPPER.getNodeFactory().textNode(value);
    }
}
