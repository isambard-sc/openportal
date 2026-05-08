// Hand-written helpers for OpenPortal types.
//
// These mirror business logic from the Rust source so that React components
// do not need to re-implement policy decisions that are authoritative in Rust.

import type { MembershipControl } from "./MembershipControl";

// ---------------------------------------------------------------------------
// MembershipControl
// ---------------------------------------------------------------------------

/**
 * Returns true if the receiving portal may add or remove members.
 *
 * Mirrors `MembershipControl::can_change_membership` in Rust:
 * true for `open` and `members_only`; false for `roles_only` and `locked`.
 * A null/undefined control is treated as `open` (the Rust default when the
 * field is absent).
 */
export function canChangeMembership(control: MembershipControl | null | undefined): boolean {
  return control == null || control === "open" || control === "members_only";
}

/**
 * Returns true if the receiving portal may change the role of an existing member.
 *
 * Mirrors `MembershipControl::can_change_roles` in Rust:
 * true for `open` and `roles_only`; false for `members_only` and `locked`.
 * A null/undefined control is treated as `open` (the Rust default when the
 * field is absent).
 */
export function canChangeRoles(control: MembershipControl | null | undefined): boolean {
  return control == null || control === "open" || control === "roles_only";
}
