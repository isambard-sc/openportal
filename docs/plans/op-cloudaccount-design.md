<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# op-cloudaccount: Design & Implementation Plan

Status: **draft prototype design** — not yet implemented. This document
records the design decided in conversation so it can be picked up, reviewed,
or handed to someone else without re-deriving it.

## 1. Goal

Add a new agent, `op-cloudaccount`, that lets a project be assigned to a
cloud account (AWS to start with, other clouds later) in the same way a
project is assigned to an `op-cluster` instance today. The cloud operators
are co-developing their side of this at the same time, so the design has to
tolerate an evolving, incomplete, occasionally-inconsistent data feed from
them — this is explicitly a rough prototype, not a hardened integration.

Two things `op-cloudaccount` must do:

1. Track which projects/users have been assigned to the cloud account
   (there is no backend API for this yet, so we are the source of truth).
2. When asked for usage, parse whatever cost/usage JSON files the cloud
   operators have dropped in a designated directory, and turn them into a
   `ProjectUsageReport` with full component breakdown, in the same shape
   `op-slurm` produces.

## 2. Non-goals (this iteration)

- Real provisioning of cloud resources, IAM users, budgets, etc. We only
  record intent and report cost; we do not call any cloud API.
- Per-user cost attribution. The example payload's `users`/`reports` fields
  are empty; all usage is attributed as unattributed/project-level until
  the operators start populating them (see §10).
- Multi-currency aggregation or FX conversion.
- Multiple clouds behind one agent process — one `op-cloudaccount` process
  per cloud account, same as one `op-cluster` per cluster instance.

## 3. Where this sits in the agent hierarchy

Decision: **a single new agent, `op-cloudaccount`, registered as
`AgentType::Instance`** — not a `op-cloud` (Instance) + `op-cloudaccount`
(Scheduler) pair mirroring `op-cluster`/`op-slurm`.

Rationale: the cluster/slurm split exists because a cluster instance
genuinely delegates to independent subsystems (OS accounts via FreeIPA,
POSIX filesystem quotas, the Slurm scheduler) that can fail or scale
independently. A cloud account doesn't have that separation yet — project
assignment and cost reporting are both just "the account itself." Merging
them into one binary is a deliberate simplification for a prototype the
cloud operators need to debug against; it can be split later (the wire
protocol instructions are already Instance-level `AddProject`/`AddUser`/
`GetUsageReport`, so splitting later just means introducing a peer agent
and forwarding, exactly as `op-cluster` forwards to `op-slurm` today).

```
Provider
  |
  v
op-cloudaccount  (AgentType::Instance)
  - holds project/user assignment state (file-backed)
  - scans the accounting directory, builds ProjectUsageReport on demand
  - answers AddProject / AddUser / GetUsageReport(s) / GetLimit directly
```

It uses `templemeads::agent::instance::{process_args, run, Defaults}`, the
same framework module `op-cluster` uses.

## 4. State model

There are two, deliberately separate, pieces of state:

### 4.1 Assignment state (owned and mutated by us)

What projects and users have been added to this cloud account — this has
no other source of truth, so it must survive an agent restart.

**Decision: plain JSON files, one per project, not sqlite.**

- No crate in this workspace currently depends on `rusqlite`/`sqlx`/`sled`.
- There is a direct precedent for exactly this situation:
  `filesystem/src/fakequotaengine.rs` is a "fake" engine built for
  prototyping without real backend infrastructure, and it persists state as
  plain files in a configured directory, one file per entity.
- Files are trivially inspectable/editable by hand while debugging the
  prototype with the cloud operators — a real requirement given how much
  back-and-forth this integration will need.

Layout, under a configured `state-dir` (e.g. `~/.config/openportal/op-cloudaccount/state/`):

```
state-dir/
  <project>.<portal>.json   # one file per assigned project
```

Each file holds:

```jsonc
{
  "mapping": { "project": "myproject.waldur", "local_group": "myproject.waldur" },
  "users": {
    "alice.myproject.waldur": { "local_user": "alice", "local_group": "myproject.waldur" }
  },
  "blocked": false,
  "blocked_users": []
}
```

Writes are atomic (write to `<name>.json.tmp`, then rename over the target)
so a crash mid-write can't corrupt state. An in-memory `Lazy<RwLock<..>>`
cache (same pattern as `slurm/src/cache.rs`) holds the loaded state so
normal operation doesn't hit disk on every job; it is a write-through cache,
updated on every mutating call before or alongside the file write.

### 4.2 Accounting directory (owned by the cloud operators, read-only to us)

A directory the operators' cron script drops cost-report JSON files into
(shape: `cost_payload_example.json`), one drop roughly every 10 minutes to
daily. We only ever read from this directory — we never write, rename, or
delete anything in it, since the whole delta-reconstruction scheme (§6)
depends on every historical drop still being there to re-read.

## 5. Instruction surface

`op-cloudaccount` implements the same top-level Instance instructions
`op-cluster` does, minus anything filesystem/quota-specific (no analogue
for a cloud account yet):

| Instruction | Behaviour |
|---|---|
| `AddProject` / `RemoveProject` | Create/remove the project's state file. `RemoveProject` marks the project inactive but does not delete history (mirrors `op-slurm`'s "don't delete, preserve statistics" comment in `main.rs`). |
| `AddUser` / `RemoveUser` | Add/remove an entry in the project's `users` map. |
| `BlockUser` / `UnblockUser` / `IsBlockedUser` | Toggle/query the `blocked_users` set. |
| `BlockProject` / `UnblockProject` / `IsBlockedProject` | Toggle/query the project's `blocked` flag. |
| `IsProtectedUser` | Always `false` for now — no concept of a protected/system user on a cloud account. |
| `GetProjects` / `GetUsers` | List from assignment state. |
| `GetProjectMapping` / `GetUserMapping` | Read from assignment state. |
| `GetUsageReport` / `GetUsageReports` | Build via §6, for one project or all projects under a portal. |
| `GetLimit` | Read `allocated_budget` from the most recent cost-report file for the project (read-only passthrough — see §12, open question). |
| `SetLimit` | Not supported yet — return `Error::InvalidInstruction` until the cloud platform can accept a budget push from us. |
| everything else (storage/quota instructions) | `Error::InvalidInstruction`, same catch-all pattern `op-slurm` uses. |

## 6. Building the usage report

This is the core new logic, replacing what `sacctmgr::get_usage_report`
does for Slurm. Input: all files in the accounting directory that parse and
whose `project` field matches. Output: a `ProjectUsageReport` with one
`DailyProjectUsageReport` per calendar day, complete with component
breakdown.

### 6.1 Parse

Deserialize each file into a tolerant struct covering only the fields we
rely on (`project`, `portal`, `generated_at`, `currency`, `time_period`,
`total`, `components`, `users`). Everything else in the payload (`details`,
`sandbox_lease`, `accounts`, ...) is cloud-specific or forward-looking and
is ignored — `serde` already ignores unrecognised fields by default, so no
`deny_unknown_fields`. A file that fails to parse, or is missing a required
field, is skipped with a `tracing::warn!` — never a hard failure, matching
`op-slurm`'s "warn and return an empty/partial report" philosophy throughout
`sacctmgr.rs`.

### 6.2 Group and order

Group parsed reports by `ProjectIdentifier` (parsed directly from the
`project` field — the example payload's `"myproject.waldur"` already is a
valid `ProjectIdentifier` string). Within a project, sort by `generated_at`
— **this is the trusted ordering signal, not filename, not file mtime**
(mtime survives a copy/rsync inconsistently; `generated_at` is inside the
content and travels with it).

### 6.3 Dedup

Key on `(project, generated_at)`. If two files share a key:
- identical `total`/`components` → duplicate upload, keep either.
- different `total`/`components` for the same `generated_at` → a genuine
  data conflict; `tracing::warn!` and keep the one with the larger `total`
  (a correction is more likely to be "more complete" than "less").

### 6.4 Compute deltas

For each project, walk the sorted, deduped series of reports. Each report's
`total` and `components` are **cumulative since `time_period.start`**, so:

- **First report in a `time_period`**: the baseline is 0 at
  `time_period.start` (that's what "cumulative since period start" means),
  so the first report already yields a valid delta: `total` spent over the
  window `[time_period.start, generated_at]`. No data is thrown away
  waiting for a second report.
- **Subsequent reports**: `delta = current.total - previous.total` over the
  window `[previous.generated_at, current.generated_at]`, and equivalently
  per-key for `components` (missing a component key in either report treats
  it as 0 for that side of the subtraction).
- **Negative delta** (credits, refunds, tax corrections can make cumulative
  cost dip) is clamped to 0 and logged with `tracing::warn!` — the same
  defensive-clamping style already used in `slurm.rs` (e.g. `wait_time()`
  clamping negative durations to zero, negative TRES counts clamped to 0).
  This is a consistency *warning*, not a hard failure.

### 6.5 Spread the delta across days

Split each window's delta evenly across every calendar day the window
touches (whole days get an equal share; a partial first/last day is
weighted by its fraction of the window — for the prototype, simple equal
split across whole days is acceptable and matches what was agreed). Add
each day's share into the matching `Date` bucket of the `ProjectUsageReport`
via `DailyProjectUsageReport::add_unattributed_usage` (total) and
`add_unattributed_component_usage` (per component) — these
"unattributed" methods already exist in `templemeads::usagereport`
specifically for usage that can't yet be tied to a specific user, which is
exactly our situation until operators populate `users` (§10).

### 6.6 Mark days complete vs provisional

A day should only be marked complete (`DailyProjectUsageReport::set_complete()`)
once it is fully behind the most recent report's `generated_at` — i.e. not
the day the latest report was generated on, since that day's true total
could still change when the next report arrives. This mirrors exactly how
`op-slurm`'s `get_daily_report`/`get_hourly_report` only call
`set_complete()` for a day that is wholly in the past. Anything still
"in progress" stays uncached/unmarked so it gets recomputed as new files
arrive.

## 7. Resolved: cost vs. `Usage`'s "seconds" unit

`templemeads::usagereport::Usage` is a `u64` count of **seconds**
(`Usage { seconds: u64 }`), and its `Display` impl auto-formats it as a
duration ("3.000 days", "5 seconds", ...). It was designed for compute time
(`op-slurm` uses it as node-seconds), but the upstream portals already
convert those raw seconds into "node hours"/"GPU hours" for display, so the
convention throughout OpenPortal is already: *the agent's `Usage` carries
some base integer unit; what that unit means, and how it's redisplayed, is
a downstream/portal concern* (see `ProjectUsageReport::scale_total()`,
exposed to the Python bridge bindings for exactly this kind of rescale).
Reinterpreting the base unit as currency for a cloud account is consistent
with that, not a special case.

**Decision: 1 `Usage` second = 1e-6 of the account's configured currency
(micro-currency-units, e.g. micro-USD).** We are reporting accounting data,
not handling money/billing directly, and expected project budgets are of
the order of thousands-to-low-millions of dollars, so headroom and
precision are not really in tension with each other — there's no real cost
to picking a fine unit:

- **Headroom**: `u64::MAX` ≈ 1.8447 × 10¹⁹. At 1e-6 resolution that
  represents ≈ 18.4 trillion dollars of budget — roughly 10¹⁰ times more
  than any project's actual budget, so overflow is not a realistic concern
  even summed over a project's entire lifetime.
- **Precision**: resolves to a hundredth of a hundredth of a cent. Only
  values below a millionth of a dollar round to zero — and those only ever
  show up in the per-service `details[]` line items (e.g. the `1.4e-09`
  example), which this design already ignores; we only difference the
  top-level `total`/`components` figures. Rounding loss there is exactly
  the kind of noise that's acceptable given we aren't handling the actual
  billing.
- No framework changes needed — it's the same "reinterpret the base unit"
  move `op-slurm` already makes for node-seconds.
- The delta/day-splitting arithmetic (§6.4–6.5) already goes through
  `Usage`'s `Mul<f64>`/`Div<f64>` before landing back in the `u64` field, so
  this scale only bounds the *final stored* rounding granularity, not the
  intermediate math.
- **Downstream caveat**: `Usage::to_string()`/`in_hours()` will render
  cloud cost as a duration, which is wrong. Any UI consuming a cloud
  account's `ProjectUsageReport` must know to divide by 1e6 and format as
  currency instead of calling `Usage`'s default `Display` — the same kind
  of rescale the portals already do to turn compute-seconds into node/GPU
  hours, just with a different target unit. Flagged here so it isn't
  discovered late by whoever owns that rendering.

Currency itself isn't tracked anywhere in `ProjectUsageReport` today. For
this prototype, currency is a per-agent config option (`currency = "USD"`);
a payload reporting a different currency is a `tracing::warn!`, not
handled (no FX conversion) — acceptable since one cloud account should only
ever bill in one currency.

## 8. Caching / invalidation

Unlike `op-slurm` (where a completed past day's Slurm accounting data can
never change, so its cache never needs to invalidate), new files can land
in the accounting directory at any time. Cache key: the sorted list of
`(filename, mtime, size)` for the project's matching files. On each
`GetUsageReport`, compare the current directory listing's fingerprint
against the cached one; only re-parse and rebuild if it changed. This keeps
the common case (repeated polling with no new drop) cheap without needing a
filesystem watcher.

## 9. Config surface (new)

- `state-dir` — where project/user assignment JSON files live.
- `accounting-dir` — where the cloud operators drop cost-report JSON files.
- `currency` — expected currency code, default `"USD"`.
- (later) `currency-scale` if 1e-6 turns out to be the wrong precision for
  a given cloud's smallest reportable amount.

## 10. Requirements for the cloud operators

### Must-have (the pipeline doesn't work without these)

1. One JSON file per drop, in the `accounting-dir`, in the agreed common
   schema subset: `project` (as `"<project>.<portal>"`, matching
   `ProjectIdentifier` — confirmed intentional and stable), `account_id`,
   `account_name`, `generated_at` (ISO-8601 with timezone), `currency`,
   `time_period.start`/`.end`, `total`, `components` (map of name →
   cumulative amount). **We assign both `project` and `portal` to the
   operators** (e.g. portal `"cloud"`) rather than letting them choose
   freely — the example payload's `"myproject.waldur"` used `"waldur"` as
   the portal name because that's what the operators picked themselves;
   going forward we tell them the exact `project.portal` string to use per
   onboarded project, since it has to match the `ProjectIdentifier` we
   already assigned on our side.
2. `total`/`components` are **cumulative since `time_period.start`**, not
   deltas. The whole reconstruction in §6 depends on this.
3. `generated_at` must be trustworthy — the actual generation time, not a
   copy/template timestamp.
4. The accounting directory is append-only from our point of view: files,
   once dropped, are never edited or deleted (history has to stay
   available for re-parsing).
5. One currency per account, consistently reported.

### Nice-to-have (would materially simplify or de-risk this)

1. **Per-user breakdown** (`users`/`reports` populated) — the biggest one;
   unlocks per-user attribution instead of everything landing in
   "unattributed".
2. **Shrink `time_period` to match the drop cadence** (i.e. report the
   incremental window since the last drop, rather than always
   month-start-to-now). This would remove the need for delta
   reconstruction entirely — it's the single biggest simplification
   possible on their side, worth asking for explicitly even if it's not
   available at first.
3. A monotonically increasing sequence number or report ID, so ordering
   and dedup don't depend purely on timestamp comparison.
4. An explicit `is_final` flag on a `time_period` once no more corrections
   are expected, so we can mark days complete without waiting on the next
   report to arrive.
5. A consistent `component` taxonomy across clouds (today everything in
   the example is bucketed into `"other"`).
6. Refunds/credits flagged explicitly (e.g. a negative-amount line item
   with a reason) rather than surfacing only as an unexplained dip in the
   cumulative total — lets us log "known credit" instead of "possible bug"
   warnings.

## 11. Open questions to raise with the operators before/while building

- ~~Confirm `project` is always exactly `"<project>.<portal>"` and
  stable.~~ Confirmed — and we assign the exact `project.portal` string
  (portal name included), rather than the operators choosing it (see §10).
- Can one cloud account ever map to more than one project, or vice versa?
  (The `accounts` array in the sample has one entry, but exists as an
  array.)
- `total` vs `cost_explorer_total` — which is authoritative if they differ?
- Is `allocated_budget` something we're expected to be able to change via
  `SetLimit`, or is budget entirely operator-side for now?

## 12. Phased implementation plan

1. **Skeleton agent**: new `cloudaccount` crate, `AgentType::Instance`,
   `Cargo.toml` modeled on `slurm/Cargo.toml` (same lint config:
   `unsafe_code = "forbid"`, `unwrap_used`/`expect_used` = deny). Wire up
   `AddProject`/`RemoveProject`/`AddUser`/`RemoveUser`/`Block*`/`Unblock*`/
   `Is*`/`GetProjects`/`GetUsers`/`GetProjectMapping`/`GetUserMapping`
   against file-backed state (§4.1) only. Nothing reads the accounting
   directory yet.
2. **Naive usage report**: `GetUsageReport` returns the latest single
   report's cumulative `total`/`components` attributed to "today", no
   delta reconstruction. Gets something end-to-end working against real
   operator drops quickly, to unblock their side of testing.
3. **Delta reconstruction**: implement §6 in full (grouping, dedup,
   per-day delta spread, completeness marking).
4. **Currency/`Usage` unit decision wired in**, config surface (§9), and
   the downstream display caveat (§7) communicated to whoever owns
   portal/bridge rendering.
5. **Hardening**: unit tests for the delta math (synthetic report
   sequences covering duplicate/out-of-order/negative-delta/missing-file
   cases), state-file round-trip tests, `cargo fmt`/`cargo clippy` clean.

## 13. Testing strategy

- Delta math is the highest-risk logic and the most unit-testable in
  isolation: build synthetic sequences of `(generated_at, total,
  components)` tuples and assert the resulting per-day `Usage` values,
  covering: normal increasing sequence; duplicate `generated_at`; a missing
  day (gap spread correctly); a negative delta (clamped + warned); a
  component present in one report but not another.
- State persistence: add/remove project/user, restart (reload from disk),
  assert state matches.
- Parsing tolerance: a payload missing an optional field, or with an
  unrecognised extra field (like `sandbox_lease`), should not fail to
  parse.
