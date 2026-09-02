// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.Optional;

/**
 * A quota's ceiling: either a size, or none at all.
 *
 * <p>Unlimited is a distinct state, not a very large number. It compares
 * greater than every size, and {@link #size} is empty for it - so code that
 * reaches for the number has to decide what unlimited means for it rather than
 * comparing against a sentinel.
 *
 * <p>On the wire this is a string: a size, or the literal {@code "unlimited"}.
 */
public record QuotaLimit(StorageSize limit) implements OpenPortalType, Comparable<QuotaLimit> {

    private static final String UNLIMITED = "unlimited";

    public static QuotaLimit limited(StorageSize size) {
        if (size == null) {
            throw new IllegalArgumentException("a limited quota needs a size");
        }

        return new QuotaLimit(size);
    }

    public static QuotaLimit unlimited() {
        return new QuotaLimit(null);
    }

    /** Parse a size, or {@code "unlimited"} in any case. */
    public static QuotaLimit parse(String value) {
        if (value == null || value.trim().isEmpty()) {
            throw new IllegalArgumentException("Quota limit cannot be empty");
        }

        String text = value.trim();

        if (text.equalsIgnoreCase(UNLIMITED)) {
            return unlimited();
        }

        return limited(StorageSize.parse(text.replaceAll("\\s+", "")));
    }

    public boolean isUnlimited() {
        return limit == null;
    }

    public boolean isLimited() {
        return limit != null;
    }

    /** The ceiling, or empty when unlimited. */
    public Optional<StorageSize> size() {
        return Optional.ofNullable(limit);
    }

    /** Unlimited sorts above every size, and equal to itself. */
    @Override
    public int compareTo(QuotaLimit other) {
        if (isUnlimited()) {
            return other.isUnlimited() ? 0 : 1;
        }

        if (other.isUnlimited()) {
            return -1;
        }

        return Long.compare(limit.bytes(), other.limit.bytes());
    }

    @Override
    public String typeName() {
        return "QuotaLimit";
    }

    @Override
    public JsonNode toJson() {
        return Json.text(toString());
    }

    public static QuotaLimit fromJson(JsonNode node) {
        return parse(node.asText());
    }

    @Override
    public String toString() {
        return isUnlimited() ? UNLIMITED : limit.toString();
    }
}
