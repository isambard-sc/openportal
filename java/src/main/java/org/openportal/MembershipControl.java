// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

/**
 * How much of an award's membership the receiving portal may change itself.
 *
 * <p>The sending portal declares a policy; the receiver is expected to honour
 * it. Absent means {@link #OPEN} - so a site reading this field must treat a
 * missing value as permission, not as refusal.
 *
 * <p>Ask {@link #canChangeMembership} and {@link #canChangeRoles} rather than
 * comparing values: {@link #MEMBERS_ONLY} and {@link #ROLES_ONLY} are not
 * symmetric, and getting them the wrong way round means either overwriting
 * roles the awarder owns or refusing a member the awarder expects added.
 */
public enum MembershipControl {

    /** Add, remove and re-role freely. The default when the field is absent. */
    OPEN("open"),

    /** Add and remove members; roles are the sender's to set. */
    MEMBERS_ONLY("members_only"),

    /** Re-role existing members; membership is the sender's to set. */
    ROLES_ONLY("roles_only"),

    /** Change neither. Both are authoritative in the award. */
    LOCKED("locked");

    private final String wire;

    MembershipControl(String wire) {
        this.wire = wire;
    }

    /** The snake_case spelling this goes on the wire as. */
    public String wire() {
        return wire;
    }

    public static MembershipControl parse(String value) {
        if (value == null || value.isBlank()) {
            return OPEN;
        }

        String wanted = value.trim();

        for (MembershipControl control : values()) {
            if (control.wire.equalsIgnoreCase(wanted)) {
                return control;
            }
        }

        throw new IllegalArgumentException("Unknown MembershipControl: '" + value + "'");
    }

    public boolean canChangeMembership() {
        return this == OPEN || this == MEMBERS_ONLY;
    }

    public boolean canChangeRoles() {
        return this == OPEN || this == ROLES_ONLY;
    }

    @Override
    public String toString() {
        return wire;
    }
}
