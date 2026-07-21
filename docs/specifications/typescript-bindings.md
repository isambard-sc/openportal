<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# TypeScript Bindings

This document describes the auto-generated TypeScript type bindings for
OpenPortal's Rust types, and the hand-written identifier utility that
accompanies them.

---

## Overview

The `templemeads` crate uses [ts-rs](https://github.com/Aleph-Alpha/ts-rs)
to derive TypeScript type definitions directly from the Rust structs and enums
that are serialised to JSON. This means the TypeScript types are always in sync
with the Rust source of truth — any change to a serialised Rust type requires
a corresponding `cargo test` run to regenerate the bindings, and the compiler
will catch any inconsistency.

The generated files live in `templemeads/bindings/`. They are standalone
TypeScript modules (one file per type) with cross-type imports handled
automatically by ts-rs.

---

## Generating the bindings

```bash
cargo test -p templemeads export_ts_bindings
```

This runs a single test in `templemeads/src/lib.rs` that calls
`Type::export_all()` on each registered type. The files are written to
`templemeads/bindings/` relative to the crate root. Re-run whenever a
serialised Rust type changes.

The `TS_RS_EXPORT_DIR` environment variable overrides the output directory:

```bash
TS_RS_EXPORT_DIR=/path/to/frontend/src/types \
  cargo test -p templemeads export_ts_bindings
```

---

## Exported types

### Core job types

| File | Rust source | Description |
|------|-------------|-------------|
| `Job.ts` | `templemeads::job::Job` | Top-level job container |
| `Status.ts` | `templemeads::job::Status` | Job lifecycle state |

`Job` timestamps (`created`, `changed`, `expires`) are Unix seconds
(`number`), not ISO 8601 strings, because the Rust fields use
`#[serde(with = "ts_seconds")]`. The `command` field is an opaque string
in the form `"<destination> <instruction>"`.

### Agent type

| File | Rust source | Description |
|------|-------------|-------------|
| `Type.ts` | `templemeads::agent::Type` | Agent role enum |

### Diagnostics

| File | Rust source | Description |
|------|-------------|-------------|
| `DiagnosticsReport.ts` | `templemeads::diagnostics::DiagnosticsReport` | Full diagnostics snapshot for one agent |
| `JobStatistics.ts` | `templemeads::diagnostics::JobStatistics` | All-time job counters |
| `FailedJobEntry.ts` | `templemeads::diagnostics::FailedJobEntry` | Deduplicated failed-job record |
| `SlowJobEntry.ts` | `templemeads::diagnostics::SlowJobEntry` | Slowest-job record |
| `ExpiredJobEntry.ts` | `templemeads::diagnostics::ExpiredJobEntry` | Deduplicated expired-job record |
| `RunningJobEntry.ts` | `templemeads::diagnostics::RunningJobEntry` | Currently-running job record |
| `LogEntry.ts` | `templemeads::diagnostics::LogEntry` | Single captured log message |

### Health

| File | Rust source | Description |
|------|-------------|-------------|
| `HealthInfo.ts` | `templemeads::health::HealthInfo` | Real-time health snapshot for one agent |

### Storage

| File | Rust source | Description |
|------|-------------|-------------|
| `Volume.ts` | `templemeads::storage::Volume` | Storage volume name (transparent `string`) |
| `Quota.ts` | `templemeads::storage::Quota` | Storage quota with limit and optional usage |

`Quota.limit` and `Quota.usage` are human-readable size strings such as
`"100GB"` or `"unlimited"` — they come from custom serde implementations
and are represented as `string` in TypeScript.

### Storage reports

| File | Rust source | Description |
|------|-------------|-------------|
| `StorageReport.ts` | `templemeads::storagereport::StorageReport` | Portal-level storage report |
| `ProjectStorageReport.ts` | `templemeads::storagereport::ProjectStorageReport` | Per-project quotas and per-user quotas |
| `DailyStorageReport.ts` | `templemeads::storagereport::DailyStorageReport` | Point-in-time storage snapshot (used inside `ProjectStorageReport`) |

### Usage reports

| File | Rust source | Description |
|------|-------------|-------------|
| `UsageReport.ts` | `templemeads::usagereport::UsageReport` | Portal-level CPU usage report |
| `ProjectUsageReport.ts` | `templemeads::usagereport::ProjectUsageReport` | Per-project usage report |
| `DailyProjectUsageReport.ts` | `templemeads::usagereport::DailyProjectUsageReport` | Per-day per-user usage |
| `UserUsageReport.ts` | `templemeads::usagereport::UserUsageReport` | Single user's usage total |
| `Usage.ts` | `templemeads::usagereport::Usage` | CPU-seconds value |

### Award details

| File | Rust source | Description |
|------|-------------|-------------|
| `AwardDetails.ts` | `templemeads::grammar::AwardDetails` | Project / award metadata |
| `Link.ts` | `templemeads::grammar::Link` | Optional (id, url) reference |
| `Note.ts` | `templemeads::grammar::Note` | Timestamped message attached to an award |
| `MembershipControl.ts` | `templemeads::grammar::MembershipControl` | Membership policy enum |

---

## Serialisation notes

### Identifier types are strings

`UserIdentifier`, `ProjectIdentifier`, `PortalIdentifier`, `UserMapping`,
and `ProjectMapping` all serialise to compact dot- or colon-separated strings
on the wire (e.g. `"alice.myproject.brics"`). They therefore appear as
`string` in the generated TypeScript, not as structured objects. Use the
[identifier utilities](#identifier-utilities) to decompose them when needed.

### HashMap keys are always `string`

Rust `HashMap<IdentifierType, V>` fields (e.g. `project_quotas`,
`user_quotas`, `reports`) appear as `{ [key in string]?: V }` in TypeScript.
The `?` reflects that TypeScript mapped types treat all keys as potentially
absent; in practice the values are always present.

### Dates and timestamps

- Fields annotated with `#[serde(with = "ts_seconds")]` (the three timestamp
  fields on `Job`) are Unix epoch seconds and appear as `number`.
- All other `DateTime<Utc>` fields (e.g. `DiagnosticsReport.generated_at`,
  `Note.timestamp`) serialise as ISO 8601 strings and appear as `string`.
- `Date` fields (`AwardDetails.start_date`, `AwardDetails.end_date`) are
  calendar-date strings in the format `"YYYY-MM-DD"` and appear as `string`.

### Custom-format strings

The following fields serialise as human-readable strings rather than structured
objects and are typed as `string` in TypeScript:

| Field | Example wire value |
|-------|--------------------|
| `Quota.limit` | `"100GB"`, `"unlimited"` |
| `Quota.usage` | `"42.3GB"` |
| `AwardDetails.template` | `"default"`, `"gpu-project"` |
| `AwardDetails.allocation` | `"1000 NHR"`, `"500 GPUHR"` |
| `AwardDetails.allowed_domains` | `["*.bristol.ac.uk", "example.com"]` |

---

## Hand-written utilities

Two companion files sit alongside the generated bindings. Neither is
auto-generated and both are safe to edit.

### `identifiers.ts` — identifier parse / stringify

Provides parse and stringify helpers for the five string-encoded identifier
types.

### `helpers.ts` — business logic mirrors

Mirrors Rust methods that encode non-obvious policy decisions, so React
components do not have to re-implement them.

#### MembershipControl helpers

```typescript
canChangeMembership(control: MembershipControl | null | undefined): boolean
canChangeRoles(control: MembershipControl | null | undefined): boolean
```

Both functions treat `null`/`undefined` as `"open"`, matching the Rust
behaviour when the `membership_control` field is absent from `AwardDetails`.

| `control` value | `canChangeMembership` | `canChangeRoles` |
|-----------------|----------------------|-----------------|
| `null` / absent | `true` | `true` |
| `"open"` | `true` | `true` |
| `"members_only"` | `true` | `false` |
| `"roles_only"` | `false` | `true` |
| `"locked"` | `false` | `false` |

#### AwardDetails allow-list helpers

```typescript
isEmailAllowed(allowedDomains: AwardDetails["allowed_domains"], email: string): boolean
isDomainAllowed(allowedDomains: AwardDetails["allowed_domains"], domain: string): boolean
```

Both mirror the corresponding Rust methods on `AwardDetails`.

`isEmailAllowed` accepts the `allowed_domains` array (or `null`) and a full
email address. An entry in the list is either a domain pattern or an exact
email address:

| Entry form | Example | Matches |
|---|---|---|
| Exact domain | `"example.com"` | Any email whose domain is exactly `example.com` |
| Wildcard subdomain | `"*.university.ac.uk"` | Any email whose domain ends with `.university.ac.uk`, at any depth |
| Exact email | `"collaborator@gmail.com"` | Only that address (case-insensitive) |

`isDomainAllowed` accepts a bare domain (no `@`) and ignores any email-pattern
entries in the list.

**Three-state allow-list semantics** (same as Rust):

| `allowedDomains` value | Result |
|---|---|
| `null` | All addresses / domains permitted |
| `[]` (empty array) | None permitted |
| `["a", "b", ...]` | Permitted if at least one entry matches |

**Usage example:**

```typescript
import { isEmailAllowed } from "./helpers";

const award: AwardDetails = /* ... */;

// Check before displaying an "add member" form
if (isEmailAllowed(award.allowed_domains, "alice@cs.bristol.ac.uk")) {
  // show the form
}
```

## Identifier utilities

### Interfaces

```typescript
interface PortalIdentifierParts   { portal: string }
interface ProjectIdentifierParts  { project: string; portal: string }
interface UserIdentifierParts     { username: string; project: string; portal: string }
interface ProjectMappingParts     { project: ProjectIdentifierParts; local_group: string }
interface UserMappingParts        { user: UserIdentifierParts; local_user: string; local_group: string }
```

### Parse functions (string → parts)

| Function | Input format | Output |
|----------|-------------|--------|
| `parsePortalIdentifier(s)` | `"portal"` | `PortalIdentifierParts` |
| `parseProjectIdentifier(s)` | `"project.portal"` | `ProjectIdentifierParts` |
| `parseUserIdentifier(s)` | `"username.project.portal"` | `UserIdentifierParts` |
| `parseProjectMapping(s)` | `"project.portal:local_group"` | `ProjectMappingParts` |
| `parseUserMapping(s)` | `"username.project.portal:local_user:local_group"` | `UserMappingParts` |

All parse functions throw `Error` if the input is malformed.

### Stringify functions (parts → string)

| Function | Output |
|----------|--------|
| `portalIdentifier(parts)` | `"portal"` |
| `projectIdentifier(parts)` | `"project.portal"` |
| `userIdentifier(parts)` | `"username.project.portal"` |
| `projectMapping(parts)` | `"project.portal:local_group"` |
| `userMapping(parts)` | `"username.project.portal:local_user:local_group"` |

### Usage example

```typescript
import type { UsageReport } from "./UsageReport";
import { parseProjectIdentifier, parseUserIdentifier } from "./identifiers";

function renderReport(report: UsageReport) {
  for (const [projectStr, projectReport] of Object.entries(report.reports ?? {})) {
    const { project, portal } = parseProjectIdentifier(projectStr);
    console.log(`Project: ${project} (portal: ${portal})`);

    for (const [userStr] of Object.entries(projectReport.users ?? {})) {
      const { username } = parseUserIdentifier(userStr);
      console.log(`  User: ${username}`);
    }
  }
}
```

---

## Adding a new exported type

1. Add `TS` to the `#[derive(...)]` list and `#[ts(export)]` to the struct or
   enum in the appropriate `templemeads/src/*.rs` file.
2. For fields whose Rust type serialises differently from its Rust structure
   (custom serde, `ts_seconds`, etc.) add the appropriate field attribute:
   - `#[ts(type = "number")]` — override to a raw TypeScript type literal
   - `#[ts(as = "SomeRustType")]` — use another type's TS representation
     (dependency tracking works correctly with this form)
3. Add the type to the export test in `templemeads/src/lib.rs`:
   ```rust
   MyNewType::export_all().expect("Could not export MyNewType");
   ```
4. Run `cargo test -p templemeads export_ts_bindings` to generate the file.
