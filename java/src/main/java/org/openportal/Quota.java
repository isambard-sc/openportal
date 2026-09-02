// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.util.Optional;
import java.util.OptionalDouble;

/**
 * A storage quota: a ceiling, and optionally what is used against it.
 *
 * <p>The usage half is optional because the two are answered by different
 * questions - a quota <i>set</i> on a volume has no usage attached, while a
 * quota <i>reported</i> from one does. {@link #usage} being empty means "not
 * measured", not "nothing used", which is why {@link #isOverQuota} and
 * {@link #percentageUsed} both decline to answer rather than reporting zero.
 *
 * <p>On the wire this is an object, {@code {"limit": "100.00 GB", "usage":
 * "50.00 GB"}}, with {@code usage} omitted when unset.
 */
public record Quota(QuotaLimit limit, StorageUsage usage) implements OpenPortalType {

    public Quota {
        if (limit == null) {
            throw new IllegalArgumentException("a quota needs a limit");
        }
    }

    public static Quota limited(StorageSize limit) {
        return new Quota(QuotaLimit.limited(limit), null);
    }

    public static Quota unlimited() {
        return new Quota(QuotaLimit.unlimited(), null);
    }

    public static Quota withUsage(QuotaLimit limit, StorageUsage usage) {
        return new Quota(limit, usage);
    }

    /**
     * Parse {@code "unlimited"}, {@code "100GB"} or {@code "100GB used 50GB"}.
     *
     * <p>The {@code used} keyword is what separates the two sizes; without it
     * the whole string is the limit.
     */
    public static Quota parse(String value) {
        if (value == null || value.trim().isEmpty()) {
            throw new IllegalArgumentException("Storage quota cannot be empty");
        }

        String text = value.trim();

        if (text.equalsIgnoreCase("unlimited")) {
            return unlimited();
        }

        String[] parts = text.split("\\s+");
        int used = -1;

        for (int i = 0; i < parts.length; i++) {
            if (parts[i].equalsIgnoreCase("used")) {
                used = i;
                break;
            }
        }

        if (used < 0) {
            return limited(StorageSize.parse(String.join("", parts)));
        }

        String limitText = String.join("", java.util.Arrays.copyOfRange(parts, 0, used));
        String usageText = String.join("", java.util.Arrays.copyOfRange(parts, used + 1, parts.length));

        return withUsage(
                QuotaLimit.limited(StorageSize.parse(limitText)),
                StorageUsage.of(StorageSize.parse(usageText)));
    }

    /**
     * As {@link #parse}, refusing a string that carries usage.
     *
     * <p>For the {@code set_*_quota} path: a caller setting a limit and
     * accidentally passing a measured quota back would otherwise have the usage
     * half silently ignored.
     */
    public static Quota parseLimitOnly(String value) {
        Quota quota = parse(value);

        if (quota.usage != null) {
            throw new IllegalArgumentException("Cannot set quota with usage information. Use only"
                    + " the limit value (e.g., '100GB' or 'unlimited')");
        }

        return quota;
    }

    /** What is used, or empty when this quota carries no measurement. */
    public Optional<StorageUsage> usageIfSet() {
        return Optional.ofNullable(usage);
    }

    public boolean isUnlimited() {
        return limit.isUnlimited();
    }

    /** {@code false} when unlimited, and when nothing was measured. */
    public boolean isOverQuota() {
        if (usage == null || limit.isUnlimited()) {
            return false;
        }

        return usage.bytes() > limit.limit().bytes();
    }

    /** Empty when unlimited, unmeasured, or limited to zero bytes. */
    public OptionalDouble percentageUsed() {
        if (usage == null || limit.isUnlimited() || limit.limit().bytes() == 0) {
            return OptionalDouble.empty();
        }

        return OptionalDouble.of((usage.bytes() / (double) limit.limit().bytes()) * 100.0);
    }

    @Override
    public String typeName() {
        return "Quota";
    }

    @Override
    public JsonNode toJson() {
        ObjectNode node = Json.object();

        node.put("limit", limit.toString());

        if (usage != null) {
            node.put("usage", usage.toString());
        }

        return node;
    }

    public static Quota fromJson(JsonNode node) {
        QuotaLimit limit = QuotaLimit.parse(node.path("limit").asText());
        StorageUsage usage = node.hasNonNull("usage")
                ? StorageUsage.parse(node.get("usage").asText())
                : null;

        return new Quota(limit, usage);
    }

    /**
     * {@code "50.00 GB / 100.00 GB | 50.0%"} when measured, the bare limit
     * otherwise.
     *
     * <p>Not the {@link #parse} form - this is for a human, and
     * {@code parse} would reject it. Serialise with {@link #toJson}.
     */
    @Override
    public String toString() {
        if (usage == null) {
            return limit.toString();
        }

        if (limit.isUnlimited()) {
            return usage + " / unlimited";
        }

        OptionalDouble percentage = percentageUsed();

        if (percentage.isPresent()) {
            return usage + " / " + limit + " | " + Fmt.fixed(percentage.getAsDouble(), 1) + "%";
        }

        return usage + " / " + limit;
    }
}
