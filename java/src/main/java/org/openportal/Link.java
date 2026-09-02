// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.net.URI;
import java.util.Locale;
import java.util.Optional;

/**
 * A pointer at something outside OpenPortal: a human-readable id, a URL, or
 * both. Used for an award's {@code award}, {@code call}, {@code project_link}
 * and {@code renewal} fields.
 *
 * <p>The URL is restricted to {@code http} and {@code https}. These links are
 * meant to be rendered in a portal UI, and a URL parser will happily accept
 * {@code javascript:} and {@code file:} - which in an anchor tag are a
 * stored-XSS and a local-file-read primitive respectively. The restriction is
 * enforced here, on the way in, so the wire path and the programmatic path
 * cannot drift.
 *
 * <p>Both halves are omitted from the JSON when unset, so an empty link
 * serialises as {@code {}}.
 */
public record Link(String id, String url) {

    public Link {
        id = blankToNull(id);
        url = validateUrl(blankToNull(url));
    }

    public static Link empty() {
        return new Link(null, null);
    }

    public static Link of(String id, String url) {
        return new Link(id, url);
    }

    /** A copy with a different id. */
    public Link withId(String newId) {
        return new Link(newId, url);
    }

    /** A copy with a different URL. */
    public Link withUrl(String newUrl) {
        return new Link(id, newUrl);
    }

    public Optional<String> idIfSet() {
        return Optional.ofNullable(id);
    }

    public Optional<String> urlIfSet() {
        return Optional.ofNullable(url);
    }

    public boolean isEmpty() {
        return id == null && url == null;
    }

    public JsonNode toJson() {
        ObjectNode node = Json.object();

        if (id != null) {
            node.put("id", id);
        }

        if (url != null) {
            node.put("url", url);
        }

        return node;
    }

    public static Link fromJson(JsonNode node) {
        if (node == null || node.isNull()) {
            return empty();
        }

        return new Link(
                node.hasNonNull("id") ? node.get("id").asText() : null,
                node.hasNonNull("url") ? node.get("url").asText() : null);
    }

    /** The JSON, which is what the Rust side's {@code Display} gives too. */
    @Override
    public String toString() {
        return Json.write(toJson());
    }

    private static String blankToNull(String value) {
        if (value == null) {
            return null;
        }

        String trimmed = value.trim();

        return trimmed.isEmpty() ? null : trimmed;
    }

    private static String validateUrl(String value) {
        if (value == null) {
            return null;
        }

        URI uri;

        try {
            uri = new URI(value);
        } catch (java.net.URISyntaxException e) {
            throw new IllegalArgumentException("Invalid URL for link: " + e.getMessage());
        }

        String scheme = uri.getScheme();

        if (scheme == null) {
            throw new IllegalArgumentException(
                    "Invalid URL for link: no scheme - only http and https are allowed");
        }

        scheme = scheme.toLowerCase(Locale.ROOT);

        if (!scheme.equals("http") && !scheme.equals("https")) {
            throw new IllegalArgumentException("Invalid URL for link: scheme '" + scheme
                    + "' is not allowed - only http and https are, because these links are"
                    + " intended to be rendered in a portal UI");
        }

        return value;
    }
}
