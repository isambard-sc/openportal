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

Both `templemeads` and `greatwestern` use
[ts-rs](https://github.com/Aleph-Alpha/ts-rs) to derive TypeScript type
definitions directly from the Rust structs and enums that are serialised to
JSON. This means the TypeScript types are always in sync with the Rust source
of truth — any change to a serialised Rust type requires a corresponding
`cargo test` run to regenerate the bindings, and the compiler will catch any
inconsistency.

`templemeads` is generic over a `Domain` (see
[writing-a-domain.md](writing-a-domain.md)) and holds only the types that
don't depend on which `Domain` an agent uses - `Job`'s envelope shape,
`Status`, diagnostics, health, agent type. `greatwestern`, the reference
`Domain`, holds everything domain-specific: the instruction/notification
vocabulary, identifiers, and usage/storage report types. The generated files
therefore live in **two** directories - `templemeads/bindings/` and
`greatwestern/bindings/` - each a standalone set of TypeScript modules (one
file per type) with cross-type imports handled automatically by ts-rs. A
project that swaps in its own `Domain` in place of `greatwestern` would ship
its own bindings directory instead of `greatwestern/bindings/`, but would
still consume `templemeads/bindings/` unchanged.

---

## Generating the bindings

```bash
# templemeads' domain-agnostic types
cargo test -p templemeads export_ts_bindings

# greatwestern's domain-specific types (Instruction, identifiers, reports, Job)
cargo test -p greatwestern
```

The `templemeads` command runs a single test in `templemeads/src/lib.rs` that
calls `Type::export_all()` on each registered type, writing to
`templemeads/bindings/`. `greatwestern` instead exports each type via its own
`#[ts(export)]` attribute (ts-rs generates one test per type automatically),
so `cargo test -p greatwestern` regenerates every file in
`greatwestern/bindings/` in one pass, including the hand-written `Job`
binding (see [Job](#job---a-hand-written-binding) below). Re-run either
command whenever a serialised Rust type in that crate changes.

The `TS_RS_EXPORT_DIR` environment variable overrides the output directory:

```bash
TS_RS_EXPORT_DIR=/path/to/frontend/src/types \
  cargo test -p templemeads export_ts_bindings
TS_RS_EXPORT_DIR=/path/to/frontend/src/types \
  cargo test -p greatwestern
```

---

## Exported types

### `templemeads/bindings/` — domain-agnostic types

#### Core job types

| File | Rust source | Description |
|------|-------------|-------------|
| `Status.ts` | `templemeads::job::Status` | Job lifecycle state |

`Status` is the only piece of `Job`'s shape templemeads can derive `TS` for
directly - see [Job](#job--a-hand-written-binding) below for why the rest of
`Job` isn't here.

#### Agent type

| File | Rust source | Description |
|------|-------------|-------------|
| `Type.ts` | `templemeads::agent::Type` | Agent role enum |

#### Diagnostics

| File | Rust source | Description |
|------|-------------|-------------|
| `DiagnosticsReport.ts` | `templemeads::diagnostics::DiagnosticsReport` | Full diagnostics snapshot for one agent |
| `JobStatistics.ts` | `templemeads::diagnostics::JobStatistics` | All-time job counters |
| `FailedJobEntry.ts` | `templemeads::diagnostics::FailedJobEntry` | Deduplicated failed-job record |
| `SlowJobEntry.ts` | `templemeads::diagnostics::SlowJobEntry` | Slowest-job record |
| `ExpiredJobEntry.ts` | `templemeads::diagnostics::ExpiredJobEntry` | Deduplicated expired-job record |
| `RunningJobEntry.ts` | `templemeads::diagnostics::RunningJobEntry` | Currently-running job record |
| `LogEntry.ts` | `templemeads::diagnostics::LogEntry` | Single captured log message |

#### Health

| File | Rust source | Description |
|------|-------------|-------------|
| `HealthInfo.ts` | `templemeads::health::HealthInfo` | Real-time health snapshot for one agent |

### `greatwestern/bindings/` — the `Hpc` domain's types

#### `Job` — a hand-written binding

| File | Rust source | Description |
|------|-------------|-------------|
| `Job.ts` | `templemeads::job::Job<Hpc>` (binding hand-written in `greatwestern::job_bindings`) | Top-level job container |

`Job<L>` cannot `#[derive(TS)]`: ts-rs's derive requires every generic
parameter to implement `TS`, which would force `Hpc` (and every other
`Domain`) to depend on ts-rs just to be usable as a type parameter. A direct
`impl TS for Job<Hpc>` isn't possible either — both `TS` and `Job` are
foreign to `greatwestern`, and Rust's orphan rules only grant an exception
when a local type appears as a parameter of the *trait*, not of the type
being implemented for. So `greatwestern/src/job_bindings.rs` instead defines
a zero-sized local marker type that hosts a hand-written `impl TS`, with
`name()`/`output_path()` overridden to still produce `Job.ts`. A test
(`job_shape_matches_binding`) serialises a real `Job::<Hpc>::parse(...)` and
checks its JSON keys against the hand-written shape, so the binding cannot
silently drift from `Job`'s actual fields.

`Job` timestamps (`created`, `changed`, `expires`) are Unix seconds
(`number`), not ISO 8601 strings, because the Rust fields use
`#[serde(with = "ts_seconds")]`. The `command` field is an opaque string
in the form `"<destination> <instruction>"`. `Job.ts` imports `Status` from
`templemeads/bindings/` in principle, but since ts-rs writes each type's
dependencies relative to its own crate's output directory, a copy of
`Status.ts` is generated inside `greatwestern/bindings/` too - both files are
identical and either may be imported.

#### Storage

| File | Rust source | Description |
|------|-------------|-------------|
| `Volume.ts` | `greatwestern::storage::Volume` | Storage volume name (transparent `string`) |
| `Quota.ts` | `greatwestern::storage::Quota` | Storage quota with limit and optional usage |

`Quota.limit` and `Quota.usage` are human-readable size strings such as
`"100GB"` or `"unlimited"` — they come from custom serde implementations
and are represented as `string` in TypeScript.

#### Storage reports

| File | Rust source | Description |
|------|-------------|-------------|
| `StorageReport.ts` | `greatwestern::storagereport::StorageReport` | Portal-level storage report |
| `ProjectStorageReport.ts` | `greatwestern::storagereport::ProjectStorageReport` | Per-project quotas and per-user quotas |
| `DailyStorageReport.ts` | `greatwestern::storagereport::DailyStorageReport` | Point-in-time storage snapshot (used inside `ProjectStorageReport`) |

#### Usage reports

| File | Rust source | Description |
|------|-------------|-------------|
| `UsageReport.ts` | `greatwestern::usagereport::UsageReport` | Portal-level CPU usage report |
| `ProjectUsageReport.ts` | `greatwestern::usagereport::ProjectUsageReport` | Per-project usage report |
| `DailyProjectUsageReport.ts` | `greatwestern::usagereport::DailyProjectUsageReport` | Per-day per-user usage |
| `UserUsageReport.ts` | `greatwestern::usagereport::UserUsageReport` | Single user's usage total |
| `Usage.ts` | `greatwestern::usagereport::Usage` | CPU-seconds value |

#### Award details

| File | Rust source | Description |
|------|-------------|-------------|
| `AwardDetails.ts` | `greatwestern::grammar::AwardDetails` | Project / award metadata |
| `Link.ts` | `greatwestern::grammar::Link` | Optional (id, url) reference |
| `Note.ts` | `greatwestern::grammar::Note` | Timestamped message attached to an award |
| `MembershipControl.ts` | `greatwestern::grammar::MembershipControl` | Membership policy enum |

A `Domain` other than `greatwestern` would export its own equivalent set of
instruction/report types from its own crate; `Job.ts` is the only binding
every `Domain` must hand-write itself, following the same pattern.

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

Companion files sit alongside the generated bindings in both crates. None of
them are auto-generated and all are safe to edit.

### `identifiers.ts` — identifier parse / stringify

Split across the two crates along the same domain-agnostic /
domain-specific line as the generated bindings:

- `templemeads/bindings/identifiers.ts` — `PortalIdentifier` only, since it
  names a fixed position in templemeads' agent hierarchy (the Portal role)
  rather than domain vocabulary.
- `greatwestern/bindings/identifiers.ts` — `ProjectIdentifier`,
  `UserIdentifier`, `ProjectMapping`, `UserMapping` and their parse/stringify
  functions, since these are `greatwestern`-specific.

The tables below cover both files together.

### `helpers.ts` (`greatwestern/bindings/`) — business logic mirrors

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

The steps differ slightly depending on which crate the type lives in.

**In `greatwestern`** (or your own `Domain` crate), where each type exports
independently via its own attribute:

1. Add `TS` to the `#[derive(...)]` list and `#[ts(export)]` to the struct or
   enum in the appropriate `greatwestern/src/*.rs` file.
2. For fields whose Rust type serialises differently from its Rust structure
   (custom serde, `ts_seconds`, etc.) add the appropriate field attribute:
   - `#[ts(type = "number")]` — override to a raw TypeScript type literal
   - `#[ts(as = "SomeRustType")]` — use another type's TS representation
     (dependency tracking works correctly with this form)
3. Run `cargo test -p greatwestern` to generate the file — no separate
   registration step needed, since `#[ts(export)]` generates its own test.

**In `templemeads`**, where a single test registers every exported type:

1. Add `TS` to the `#[derive(...)]` list and `#[ts(export)]` to the struct or
   enum in the appropriate `templemeads/src/*.rs` file.
2. Add field attributes as above if needed.
3. Add the type to the export test in `templemeads/src/lib.rs`:
   ```rust
   MyNewType::export_all().expect("Could not export MyNewType");
   ```
4. Run `cargo test -p templemeads export_ts_bindings` to generate the file.

A type that is generic over `L: Domain` (like `Job<L>`) can't use either
route directly - see [Job](#job--a-hand-written-binding) above for the
hand-written-marker-type pattern to follow instead.
