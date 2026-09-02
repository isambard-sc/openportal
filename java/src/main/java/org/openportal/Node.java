// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;

/**
 * The shape of one compute node, which is what lets one unit be expressed in
 * another.
 *
 * <p>Only useful alongside {@link Allocation}: {@code 4 GPUHR} is
 * {@code 1 NHR} on a four-GPU node and {@code 0.5 NHR} on an eight-GPU one, so
 * a conversion needs the node. Note that this is the <i>derivation</i> route,
 * and a site portal usually should not be doing it - the site and the allocator
 * pre-agree a factor between their units instead, and how the site divides its
 * own unit among real GPUs and cores is the site's own business (see the
 * {@code site_portal} example). This type is here for the code that does need
 * to reason about hardware.
 *
 * <p>A field left at zero is not neutral. Converting GPU hours against a node
 * with no GPUs has no answer, and {@link Allocation} raises rather than
 * quietly returning zero - see the {@code cannot convert} messages there.
 */
public record Node(int cpus, int coresPerCpu, int gpus, int memoryMb, int billing) {

    public Node {
        requireNotNegative(cpus, "cpus");
        requireNotNegative(coresPerCpu, "cores_per_cpu");
        requireNotNegative(gpus, "gpus");
        requireNotNegative(memoryMb, "memory_mb");
        requireNotNegative(billing, "billing");
    }

    /** An empty node - every field zero, so no conversion is possible from it. */
    public static Node empty() {
        return new Node(0, 0, 0, 0, 0);
    }

    public static Node of(int cpus, int coresPerCpu, int gpus, int memoryMb, int billing) {
        return new Node(cpus, coresPerCpu, gpus, memoryMb, billing);
    }

    /** Total cores: {@code cpus × coresPerCpu}. */
    public int cores() {
        return cpus * coresPerCpu;
    }

    /** Memory in GB. Binary GB - {@code memoryMb / 1024}, as the Rust side does it. */
    public double memoryGb() {
        return memoryMb / 1024.0;
    }

    public JsonNode toJson() {
        return Json.object()
                .put("cpus", cpus)
                .put("cores_per_cpu", coresPerCpu)
                .put("gpus", gpus)
                .put("memory_mb", memoryMb)
                .put("billing", billing);
    }

    public static Node fromJson(JsonNode node) {
        if (node == null || node.isNull()) {
            return empty();
        }

        return new Node(
                node.path("cpus").asInt(),
                node.path("cores_per_cpu").asInt(),
                node.path("gpus").asInt(),
                node.path("memory_mb").asInt(),
                node.path("billing").asInt());
    }

    @Override
    public String toString() {
        return "Node(cpus: " + cpus
                + ", cores_per_cpu: " + coresPerCpu
                + ", gpus: " + gpus
                + ", memory: " + Fmt.number(memoryGb()) + " GB"
                + ", billing: " + billing + ")";
    }

    private static void requireNotNegative(int value, String field) {
        if (value < 0) {
            throw new IllegalArgumentException("Node " + field + " cannot be negative: " + value);
        }
    }
}
