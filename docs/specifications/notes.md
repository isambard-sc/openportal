<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Specification Notes

This file records known gaps in the formal specifications, evolving or
provisional schemas, and operational observations that do not fit neatly into
the formal specification documents.

---

## 1. Errata and Known Gaps

### 1.1 `GetUserDirs` and `GetLocalUserDirs` missing from instruction parser

The instructions `get_user_dirs` and `get_local_user_dirs` exist in the
`Instruction` enum and have `Display` implementations (so they can be
serialised as strings), but neither has a corresponding case in
`Instruction::parse()`. As a result, these instructions cannot currently be
parsed from a command string sent over the wire.

This is a known omission and **will be fixed in a later update**. Until then,
these instructions cannot be invoked through the standard command protocol.

**Affected source:** `templemeads/src/grammar.rs`, `Instruction::parse()`

---

### 1.2 `HealthInfo` schema (provisional — still evolving)

The `HealthInfo` struct is returned by the `HealthResponse` Templemeads command
and exposed through the bridge API's `GET /health` endpoint. Its schema is
**still evolving** and may change without notice in future releases.

The current schema is:

```json
{
  "name":               "<agent-name>",
  "agent_type":         "<AgentType>",
  "connected":          <boolean>,

  "active_jobs":        <integer>,
  "pending_jobs":       <integer>,
  "running_jobs":       <integer>,
  "completed_jobs":     <integer>,
  "duplicate_jobs":     <integer>,
  "successful_jobs":    <integer>,
  "expired_jobs":       <integer>,
  "errored_jobs":       <integer>,
  "inflight_jobs":      <integer>,
  "queued_jobs":        <integer>,

  "worker_count":       <integer>,

  "memory_bytes":       <integer>,
  "cpu_percent":        <float>,
  "system_memory_total":<integer>,
  "system_cpus":        <integer>,

  "job_time_min_ms":    <float>,
  "job_time_max_ms":    <float>,
  "job_time_mean_ms":   <float>,
  "job_time_median_ms": <float>,
  "job_time_count":     <integer>,

  "total_completed":    <integer>,
  "total_failed":       <integer>,
  "total_expired":      <integer>,
  "total_slow":         <integer>,

  "start_time":         "<ISO 8601 datetime>",
  "current_time":       "<ISO 8601 datetime>",
  "uptime_seconds":     <integer>,
  "last_updated":       "<ISO 8601 datetime>",

  "engine":             "<engine-name>",
  "version":            "<version-string>",

  "peers": {
    "<peer-name>": { <nested HealthInfo> },
    ...
  }
}
```

**Field notes:**

- `connected` — `true` if the agent is currently reachable; `false` if the
  health sweep could not contact it (in which case cached data may be shown)
- `active_jobs` — total jobs currently on this agent's job boards (all states)
- `inflight_jobs` — jobs passing through this agent as an intermediate hop
- `queued_jobs` — jobs waiting to be sent because the target connection is not
  yet ready
- `job_time_*` — execution time statistics for the most recent jobs processed
  by this agent (excludes jobs with no timing data)
- `total_*` — all-time counters, persisted only while the process is running
  (reset on restart)
- `peers` — recursively nested `HealthInfo` for downstream agents; populated by
  the health-check cascade (each agent queries its direct neighbours, which
  query theirs, up to 500 ms timeout per hop). Absent peers are marked
  `connected: false`.
- Portals do not cascade health checks to other portals (security constraint).

**Source:** `templemeads/src/health.rs`

---

### 1.3 `DiagnosticsReport` schema (provisional — still evolving)

The `DiagnosticsReport` struct is returned by the `DiagnosticsResponse`
Templemeads command and exposed through the bridge API's `POST /diagnostics`
endpoint. Its schema is **still evolving** and may change without notice in
future releases.

The current schema is:

```json
{
  "agent_name":    "<agent-name>",
  "generated_at":  "<ISO 8601 datetime>",

  "failed_jobs": [
    {
      "destination":    "<destination-string>",
      "instruction":    "<instruction-string>",
      "error_message":  "<string>",
      "count":          <integer>,
      "first_seen":     "<ISO 8601 datetime>",
      "last_seen":      "<ISO 8601 datetime>"
    }
  ],

  "slowest_jobs": [
    {
      "destination":  "<destination-string>",
      "instruction":  "<instruction-string>",
      "duration_ms":  <float>,
      "completed_at": "<ISO 8601 datetime>"
    }
  ],

  "expired_jobs": [
    {
      "destination": "<destination-string>",
      "instruction": "<instruction-string>",
      "created_at":  "<ISO 8601 datetime>",
      "expired_at":  "<ISO 8601 datetime>",
      "count":       <integer>
    }
  ],

  "running_jobs": [
    {
      "destination":          "<destination-string>",
      "instruction":          "<instruction-string>",
      "started_at":           "<ISO 8601 datetime>",
      "count":                <integer>,
      "running_for_seconds":  <integer>
    }
  ],

  "warnings": ["<string>", ...]
}
```

**Field notes:**

- `failed_jobs` — deduplicated by `(destination, instruction)` pair; up to 200
  unique pairs tracked, showing the 100 most recent. `count` is the number of
  times that `(destination, instruction)` pair has failed.
- `slowest_jobs` — top 200 slowest successful jobs (threshold: >10 seconds),
  showing the 100 slowest. Sorted by `duration_ms` descending.
- `expired_jobs` — deduplicated by `(destination, instruction)` pair; up to 200
  unique pairs tracked, showing the 100 most recent.
- `running_jobs` — jobs currently in progress, deduplicated by
  `(destination, instruction)` pair. Sorted by `running_for_seconds` descending.
- `warnings` — auto-generated alert strings: high failure rates (≥10
  occurrences), jobs running longer than 5 minutes, large numbers of expired
  jobs (>50 tracked).
- All counters and lists reset when the agent restarts.
- Diagnostics can be forwarded through the agent hierarchy using dot-separated
  paths (e.g. `"cluster.filesystem"`) and zone specifiers
  (`"cluster@zone-name"`). Leaf agents (FreeIPA, Filesystem, Slurm) cannot
  forward requests further.

**Source:** `templemeads/src/diagnostics.rs`

---

## 2. Duplicate Job Handling

When a job board receives a new `Put` for a job that is already `pending`, it
checks whether the incoming job is a **duplicate** of the existing one.

### 2.1 Duplicate detection

Two pending jobs are considered duplicates if:

- They share the same **final destination** (last component of the destination
  path), and
- They carry the **same instruction** (keyword + arguments).

Both jobs must be in `pending` state for the check to trigger.

### 2.2 What happens to a duplicate

The incoming job is not discarded. Instead, it is transitioned to the
`duplicate` state and stored on the board:

- `state` → `"duplicate"`
- `result` → the UUID string of the **original** pending job

This allows the caller to look up the original job and poll its status.

### 2.3 Resolution

When the **original** job finishes (transitions to `complete` or `error`), all
jobs recorded as its duplicates are automatically updated to match its final
state and result. Callers polling the duplicate job will therefore eventually
see the same outcome as the original.

### 2.4 Limits and error cases

- A maximum of **100 duplicates** are allowed per original job. Attempts to add
  a 101st duplicate return a `TooManyDuplicatesError`.
- If the original job is considered "too old" (stale pending state), new
  duplicates are also refused with a `TooManyDuplicatesError`. The caller
  should retry the instruction from scratch.

**Source:** `templemeads/src/board.rs`, `templemeads/src/job.rs`

---

## 3. Job Expiry

Every `Job` carries an `expires` Unix timestamp. Agents check this field before
processing a job. If the current time is past `expires`, the job is marked
`error` with an expiry message and is not executed.

Default job lifetime is set per instruction type in the agent layer (typically
1–5 minutes for standard instructions). The bridge agent sets a longer lifetime
of 5 minutes for jobs submitted via `/run`, to give the portal software
sufficient time to poll for results.

Expired jobs are tracked in the `DiagnosticsReport` (see §1.3) and counted in
`HealthInfo.expired_jobs` (see §1.2). A spike in expired jobs typically
indicates: network latency between agents, a downstream agent being too slow,
or a client polling loop that is too long.

---

## 4. Virtual Agents

When the bridge agent receives a `sync_offerings` instruction, it creates
lightweight **virtual agent** records internally — one per offering in the
provided `Destinations` set. Virtual agents are not real processes; they are
entries in the agent registry that allow the portal to route jobs to named
resources without those resources being directly connected peers.

Each virtual agent is created with:
- **Name** — the first component of the offering destination (the resource name)
- **Zone** — the same zone as the connected portal agent

Virtual agents serve two purposes:

1. They allow instructions addressed to `resource-name.portal-name` to be
   accepted and routed correctly within the bridge.
2. They drain any jobs that were queued while the offering was not yet
   registered (jobs submitted before `sync_offerings` was called are
   automatically flushed to the virtual agent once it exists).

Virtual agents for offerings removed from the list are deleted as part of the
same `sync_offerings` call.

**Source:** `bridge/src/main.rs` (`sync_offerings`), `templemeads/src/virtual_agent.rs`

---

## 5. Operational Notes

### 5.1 Common connection failure causes

| Symptom | Likely cause |
|---------|-------------|
| Connection rejected immediately | Connecting IP does not match any `ClientConfig` entry (Layer 1 check) |
| Handshake fails / decryption error | Wrong pre-shared keys; invite file used by the wrong peer pair |
| Zone mismatch error in logs | The `zone` field in the peer's config does not match the zone in the invite |
| `Date is outside acceptable time window` | Server and client clocks differ by more than 5 seconds; synchronise NTP |
| Agent connects but jobs don't flow | Agent names in the destination path don't match the configured service names |

### 5.2 Key rotation in a live deployment

Key rotation does not require stopping agents. The procedure is:

1. On the **server** agent: `<agent> client --rotate <client-name>` — generates
   a `rotate_<name>_<zone>.toml` file.
2. Transfer the rotation invite to the client operator securely.
3. On the **client** agent: `<agent> server --rotate rotate_<name>_<zone>.toml`
4. Restart both agents. The new keys take effect on the next connection.

Existing in-flight jobs will complete using the old keys for the duration of
the current connection. Only new connections use the rotated keys.

### 5.3 Health check cascade behaviour

The `GET /health` bridge endpoint triggers a cascading health sweep. Each agent
queries its direct peers and waits up to **500 ms** for responses before
returning. The full sweep therefore takes approximately `500 ms × depth` in the
worst case. For a typical four-level hierarchy, expect up to ~2 seconds for a
complete response.

Agents that do not respond within the 500 ms window are shown with
`connected: false` using their last cached health data (if any). A fresh
health check may return more up-to-date data for those agents.

### 5.4 Slow job threshold

Jobs that take longer than **10 seconds** to complete are classified as "slow"
and appear in `DiagnosticsReport.slowest_jobs`. This threshold is fixed in the
current implementation.

### 5.5 Diagnostics path format

The `POST /diagnostics` endpoint accepts a dot-separated path to reach agents
below the bridge in the hierarchy. An optional `@zone` suffix disambiguates
agents with the same name in different zones:

```
"cluster"             → direct peer named "cluster"
"cluster.filesystem"  → "filesystem" agent reached via "cluster"
"cluster@prod.slurm"  → "slurm" agent reached via the "cluster" peer in zone "prod"
```

Leaf agents (FreeIPA, Filesystem, Slurm) cannot forward diagnostics requests
further down the hierarchy.
