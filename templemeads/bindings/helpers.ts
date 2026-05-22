// Hand-written helpers for OpenPortal types.
//
// These mirror business logic from the Rust source so that React components
// do not need to re-implement policy decisions that are authoritative in Rust.

import type { AwardDetails } from "./AwardDetails";
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

// ---------------------------------------------------------------------------
// AwardDetails — allowed_domains helpers
// ---------------------------------------------------------------------------

/**
 * Tests whether a bare domain matches one entry from an `allowed_domains` list.
 * Mirrors `DomainPattern::matches` in Rust — email-pattern entries are ignored.
 *
 * Pattern forms:
 *   - `"*.example.com"` — matches any subdomain at any depth that ends with
 *     `.example.com` (e.g. `sub.example.com`, `a.b.example.com`).
 *   - `"example.com"`  — exact case-insensitive match only.
 */
function matchesDomainPattern(pattern: string, domain: string): boolean {
  if (pattern.startsWith("*.")) {
    return domain.toLowerCase().endsWith(pattern.slice(1).toLowerCase());
  }
  return domain.toLowerCase() === pattern.toLowerCase();
}

/**
 * Returns true if `email` is permitted by the `allowed_domains` list of an
 * `AwardDetails` object.
 *
 * Mirrors `AwardDetails::is_email_allowed` in Rust:
 * - `null` list  → all addresses permitted.
 * - Empty list   → no addresses permitted.
 * - Otherwise    → permitted if at least one entry matches:
 *     • An email-pattern entry (contains `@`) matches the full address
 *       case-insensitively.
 *     • A domain-pattern entry matches the domain part of the address.
 */
export function isEmailAllowed(
  allowedDomains: AwardDetails["allowed_domains"],
  email: string,
): boolean {
  if (allowedDomains === null) return true;
  if (allowedDomains.length === 0) return false;

  const atIdx = email.indexOf("@");
  const domain = atIdx >= 0 ? email.slice(atIdx + 1) : "";

  for (const pattern of allowedDomains) {
    if (pattern.includes("@")) {
      if (pattern.toLowerCase() === email.toLowerCase()) return true;
    } else {
      if (domain && matchesDomainPattern(pattern, domain)) return true;
    }
  }

  return false;
}

/**
 * Returns true if the bare `domain` (no `@`) is permitted by the
 * `allowed_domains` list.  Email-pattern entries are ignored.
 *
 * Mirrors `AwardDetails::is_domain_allowed` in Rust.
 */
export function isDomainAllowed(
  allowedDomains: AwardDetails["allowed_domains"],
  domain: string,
): boolean {
  if (allowedDomains === null) return true;
  if (allowedDomains.length === 0) return false;

  for (const pattern of allowedDomains) {
    if (!pattern.includes("@") && matchesDomainPattern(pattern, domain)) return true;
  }

  return false;
}
