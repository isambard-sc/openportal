// Hand-written utility for the identifier types used by OpenPortal.
//
// On the wire, all identifiers are compact dot- or colon-separated strings
// (e.g. "alice.myproject.brics"). These helpers let React components
// decompose them into named parts and reassemble them for sending back.
//
// PortalIdentifier lives here because it names a fixed position in
// templemeads' agent hierarchy (the Portal role), not domain vocabulary -
// see docs/plans/grammar-split-design.md. ProjectIdentifier/UserIdentifier
// and their mappings are domain vocabulary and live in
// greatwestern/bindings/identifiers.ts instead.

// ---------------------------------------------------------------------------
// Interfaces
// ---------------------------------------------------------------------------

export interface PortalIdentifierParts {
  portal: string;
}

// ---------------------------------------------------------------------------
// Parse functions  (string → parts)
// ---------------------------------------------------------------------------

export function parsePortalIdentifier(s: string): PortalIdentifierParts {
  if (!s) throw new Error(`Invalid PortalIdentifier: "${s}"`);
  return { portal: s };
}

// ---------------------------------------------------------------------------
// Stringify functions  (parts → string, for sending back to OpenPortal)
// ---------------------------------------------------------------------------

export function portalIdentifier(parts: PortalIdentifierParts): string {
  return parts.portal;
}
