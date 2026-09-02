// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;

/**
 * Storage currently in use - a {@link StorageSize} in the "used" position.
 *
 * <p>A separate type only so that a limit and a usage cannot be passed to each
 * other's parameter. Serialises exactly as its size does, transparently: no
 * wrapping object.
 */
public record StorageUsage(StorageSize size) implements OpenPortalType {

    public static final StorageUsage ZERO = new StorageUsage(StorageSize.ZERO);

    public StorageUsage {
        if (size == null) {
            throw new IllegalArgumentException("a storage usage needs a size");
        }
    }

    public static StorageUsage of(StorageSize size) {
        return new StorageUsage(size);
    }

    public static StorageUsage fromBytes(long bytes) {
        return new StorageUsage(StorageSize.fromBytes(bytes));
    }

    public static StorageUsage parse(String value) {
        return new StorageUsage(StorageSize.parse(value));
    }

    public long bytes() {
        return size.bytes();
    }

    @Override
    public String typeName() {
        return "StorageUsage";
    }

    @Override
    public JsonNode toJson() {
        return size.toJson();
    }

    public static StorageUsage fromJson(JsonNode node) {
        return new StorageUsage(StorageSize.fromJson(node));
    }

    @Override
    public String toString() {
        return size.toString();
    }
}
