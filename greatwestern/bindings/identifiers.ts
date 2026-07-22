// Hand-written utility for the identifier types used by the HPC/Waldur
// (greatwestern) command vocabulary.
//
// On the wire, all identifiers are compact dot- or colon-separated strings
// (e.g. "alice.myproject.brics"). These helpers let React components
// decompose them into named parts and reassemble them for sending back.
//
// PortalIdentifier is not here - it lives in templemeads/bindings/
// identifiers.ts, since it names a fixed position in templemeads' agent
// hierarchy (the Portal role) rather than domain vocabulary. See
// docs/plans/grammar-split-design.md.

// ---------------------------------------------------------------------------
// Interfaces
// ---------------------------------------------------------------------------

export interface ProjectIdentifierParts {
  project: string;
  portal: string;
}

export interface UserIdentifierParts {
  username: string;
  project: string;
  portal: string;
}

export interface ProjectMappingParts {
  project: ProjectIdentifierParts;
  local_group: string;
}

export interface UserMappingParts {
  user: UserIdentifierParts;
  local_user: string;
  local_group: string;
}

// ---------------------------------------------------------------------------
// Parse functions  (string → parts)
// ---------------------------------------------------------------------------

export function parseProjectIdentifier(s: string): ProjectIdentifierParts {
  const parts = s.split(".");
  if (parts.length !== 2 || parts.some((p) => !p))
    throw new Error(`Invalid ProjectIdentifier: "${s}"`);
  return { project: parts[0], portal: parts[1] };
}

export function parseUserIdentifier(s: string): UserIdentifierParts {
  const parts = s.split(".");
  if (parts.length !== 3 || parts.some((p) => !p))
    throw new Error(`Invalid UserIdentifier: "${s}"`);
  return { username: parts[0], project: parts[1], portal: parts[2] };
}

export function parseProjectMapping(s: string): ProjectMappingParts {
  const parts = s.split(":");
  if (parts.length !== 2 || parts.some((p) => !p))
    throw new Error(`Invalid ProjectMapping: "${s}"`);
  return {
    project: parseProjectIdentifier(parts[0]),
    local_group: parts[1],
  };
}

export function parseUserMapping(s: string): UserMappingParts {
  const parts = s.split(":");
  if (parts.length !== 3 || parts.some((p) => !p))
    throw new Error(`Invalid UserMapping: "${s}"`);
  return {
    user: parseUserIdentifier(parts[0]),
    local_user: parts[1],
    local_group: parts[2],
  };
}

// ---------------------------------------------------------------------------
// Stringify functions  (parts → string, for sending back to OpenPortal)
// ---------------------------------------------------------------------------

export function projectIdentifier(parts: ProjectIdentifierParts): string {
  return `${parts.project}.${parts.portal}`;
}

export function userIdentifier(parts: UserIdentifierParts): string {
  return `${parts.username}.${parts.project}.${parts.portal}`;
}

export function projectMapping(parts: ProjectMappingParts): string {
  return `${projectIdentifier(parts.project)}:${parts.local_group}`;
}

export function userMapping(parts: UserMappingParts): string {
  return `${userIdentifier(parts.user)}:${parts.local_user}:${parts.local_group}`;
}
