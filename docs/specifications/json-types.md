<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal JSON Type Specifications

This document specifies the JSON serialisation format for the key object types
used in OpenPortal. All results returned by agents are carried inside a `Job`
object; this document covers `Job` itself and every result type it can contain.

Types from the instruction protocol that serialise as plain strings (e.g.
`UserIdentifier`, `ProjectIdentifier`, `UserMapping`, `ProjectMapping`,
`Destination`) are documented in [instruction-protocol.md](instruction-protocol.md).

---

## Job

The top-level container transmitted between agents. Every instruction creates a
`Job`, which is updated in-place as it travels through the agent hierarchy.

```json
{
  "id":          "<uuid-v4>",
  "created":     <unix-timestamp-seconds>,
  "changed":     <unix-timestamp-seconds>,
  "expires":     <unix-timestamp-seconds>,
  "version":     <u64>,
  "command":     "<destination> <instruction>",
  "state":       "<status>",
  "result":      "<json-string>" | null,
  "result_type": "<type-name>" | null
}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | UUID string | Unique job identifier (UUID v4) |
| `created` | integer | Unix timestamp (seconds) when the job was created |
| `changed` | integer | Unix timestamp (seconds) when the job was last updated |
| `expires` | integer | Unix timestamp (seconds) after which the job is invalid |
| `version` | integer | Monotonically increasing version counter |
| `command` | string | Full command string: `<destination> <instruction>` |
| `state` | string | One of `created`, `pending`, `running`, `complete`, `error`, `duplicate` |
| `result` | string or null | JSON-encoded result payload (see below); null when not yet complete |
| `result_type` | string or null | Rust type name of the result (see [Result Types](#result-types)) |

### Job States

| State | Meaning |
|-------|---------|
| `created` | Job has been created but not yet queued |
| `pending` | Job is queued, awaiting processing |
| `running` | Job is currently being processed; `result` may hold a progress message |
| `complete` | Job finished successfully; `result` holds the JSON payload |
| `error` | Job failed; `result` holds a plain-text error message |
| `duplicate` | Job is a duplicate of an earlier pending job; `result` holds the original job's UUID |

### Extracting Results

When `state` is `complete`, `result` is a JSON string containing the serialised
return value. Parse it according to the `result_type` field. When `state` is
`error`, `result` is a plain-text error message (not JSON).

**Example — complete job with a `ProjectUsageReport` result:**

```json
{
  "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "created": 1700000000,
  "changed": 1700000010,
  "expires":  1700000120,
  "version": 1002,
  "command": "waldur.provider.cluster add_user alice.myproject.waldur",
  "state": "complete",
  "result": "{\"project\":\"myproject.waldur\",\"reports\":{},\"users\":{}}",
  "result_type": "ProjectUsageReport"
}
```

---

## Result Types

### None

Instructions that return no value (`add_user`, `remove_user`, `add_project`, etc.)
complete with `result_type` of `"None"` and `result` of `null` or `"{}"`.

---

### `bool`

Returned by: `is_protected_user`, `is_existing_user`, `is_existing_project`

```json
true | false
```

**Example:**
```json
true
```

---

### `String`

Returned by: `get_home_dir`, `get_local_home_dir`

A plain JSON string containing a filesystem path.

```json
"/home/alice_hpc"
```

---

### `Vec<String>`

Returned by: `get_project_dirs`, `get_local_project_dirs`

A JSON array of filesystem path strings.

```json
[
  "/project/myproject",
  "/scratch/myproject"
]
```

---

### `UserMapping`

Returned by: `get_user_mapping`

Serialised as a plain string in the instruction protocol format (see
[instruction-protocol.md](instruction-protocol.md)).

```json
"alice.myproject.waldur:alice_hpc:hpc_myproject"
```

---

### `ProjectMapping`

Returned by: `get_project_mapping`

Serialised as a plain string in the instruction protocol format.

```json
"myproject.waldur:hpc_myproject"
```

---

### `Vec<UserIdentifier>`

Returned by: `get_users`

A JSON array of user identifier strings, each in `username.project.portal` format.

```json
[
  "alice.myproject.waldur",
  "bob.myproject.waldur"
]
```

---

### `ProjectDetails`

Also known as `AwardDetails` — `ProjectDetails` is a backward-compatibility
alias; new code should use `AwardDetails`. The `result_type` field in a `Job`
will contain `"ProjectDetails"` for wire-protocol compatibility.

Returned by: `get_project`, `get_award`

A JSON object. All fields are optional and may be absent if not set by the portal.
Fields that are `null` or unset are omitted from the serialised JSON.

```json
{
  "name":        "My Research Project",
  "template":    "cpu-cluster",
  "key":         "secret-access-key",
  "description": "A project for running large-scale simulations",
  "members": {
    "alice@example.com": "pi",
    "bob@example.com":   "member"
  },
  "start_date":  "2024-01-01",
  "end_date":    "2024-12-31",
  "allocation":  "1000 NHR",
  "award":       { "id": "061-4738952-1", "url": "https://gtr.ukri.org/..." },
  "call":        { "id": "EPSRC-2024-AI", "url": "https://..." },
  "project_link":{ "id": "PRJ-001", "url": "https://waldur.example.ac.uk/projects/..." },
  "renewal":     { "url": "https://apply.example.ac.uk/renew" },
  "notes": [
    { "timestamp": "2024-01-15T10:30:00Z", "author": "Jane Smith", "text": "Approved." }
  ],
  "earliest_approve": "2024-01-15T11:30:00Z",
  "allowed_domains": [
    "example.com",
    "*.university.ac.uk"
  ]
}
```

**Field details:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Human-readable project name |
| `template` | string | Template identifier (alphanumeric, `_`, `-` only) |
| `key` | string | Authorization key for the template |
| `description` | string | Free-text project description |
| `members` | object | Map of email address → role string |
| `start_date` | string | ISO date `YYYY-MM-DD` |
| `end_date` | string | ISO date `YYYY-MM-DD` |
| `allocation` | string | Resource allocation, e.g. `"1000 NHR"` or `"No allocation"` |
| `award` | `Link` | Link to the award record on the funding body's system |
| `call` | `Link` | Link to the funding call that produced this award |
| `project_link` | `Link` | Link to the project page on the remote/awarding portal |
| `renewal` | `Link` | Link to the renewal / more-time application page |
| `notes` | array of `Note` | Append-only timestamped messages; omitted when empty |
| `earliest_approve` | string | RFC 3339 UTC — do not approve before this time; omitted when unset |
| `allowed_domains` | array of strings | Domain allow-list; `null` = all; `[]` = none |

**`Link` object:**

```json
{ "id": "<string>", "url": "<url>" }
```

Both `id` and `url` are optional; fields absent when `null`.

**`Note` object:**

```json
{ "timestamp": "<RFC 3339 UTC>", "author": "<string>", "text": "<string>" }
```

All three fields are always present.

---

### `Vec<ProjectDetails>`

Returned by: `get_projects`, `get_awards`

A JSON array of `ProjectDetails` objects.

```json
[
  {
    "name": "Project A",
    "template": "cpu-cluster"
  },
  {
    "name": "Project B",
    "template": "gpu-cluster",
    "allocation": "500 GPUHR"
  }
]
```

---

### `Usage`

Returned by: `get_limit`, `get_local_limit`

A JSON object containing a single integer field `seconds`.

```json
{
  "seconds": 3600000
}
```

| Field | Type | Description |
|-------|------|-------------|
| `seconds` | integer (u64) | Total compute time in seconds |

---

### `Quota`

Returned by: `get_project_quota`, `get_user_quota`, `get_local_project_quota`,
`get_local_user_quota`

A JSON object with a required `limit` field and an optional `usage` field.
Both the limit and usage are serialised as human-readable size strings.

**With usage:**
```json
{
  "limit": "5.00 TB",
  "usage": "2.34 TB"
}
```

**Without usage (limit only):**
```json
{
  "limit": "100.00 GB"
}
```

**Unlimited:**
```json
{
  "limit": "unlimited"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `limit` | string | Storage limit: a size string (e.g. `"5.00 TB"`) or `"unlimited"` |
| `usage` | string | (Optional) Current storage usage as a size string |

**Size string format:** a number with two decimal places followed by a space and
a unit. Units produced by serialisation: `B`, `KB`, `MB`, `GB`, `TB`, `PB`.

---

### `HashMap<Volume, Quota>`

Returned by: `get_project_quotas`, `get_user_quotas`, `get_local_project_quotas`,
`get_local_user_quotas`

A JSON object whose keys are volume name strings and whose values are `Quota`
objects. The key is the bare volume name (no quotes beyond standard JSON).

```json
{
  "home":    {"limit": "100.00 GB", "usage": "42.00 GB"},
  "scratch": {"limit": "5.00 TB",   "usage": "1.20 TB"},
  "project": {"limit": "unlimited"}
}
```

---

### `UserUsageReport`

A per-user usage summary within a daily report. Not returned directly as a
top-level result but appears inside `DailyProjectUsageReport`.

```json
{
  "user":  "alice.myproject.waldur",
  "usage": {"seconds": 7200}
}
```

| Field | Type | Description |
|-------|------|-------------|
| `user` | string | `UserIdentifier` in `username.project.portal` format |
| `usage` | `Usage` | Compute usage for this user |

---

### `DailyProjectUsageReport`

Compute usage for a single project on a single day, broken down by local
username. Appears as a value inside `ProjectUsageReport.reports`.

```json
{
  "reports": {
    "alice_hpc": {"seconds": 7200},
    "bob_hpc":   {"seconds": 3600},
    "unknown":   {"seconds": 900}
  },
  "components": {
    "gpu-partition": {
      "alice_hpc": {"seconds": 3600}
    },
    "cpu-partition": {
      "alice_hpc": {"seconds": 3600},
      "bob_hpc":   {"seconds": 3600}
    }
  },
  "num_jobs":    15,
  "total_wait_seconds": 1800,
  "user_job_counts": {
    "alice_hpc": 10,
    "bob_hpc":   5
  },
  "user_wait_seconds": {
    "alice_hpc": 1200,
    "bob_hpc":   600
  },
  "is_complete": true
}
```

| Field | Type | Description |
|-------|------|-------------|
| `reports` | object | Map of local username → `Usage`. The key `"unknown"` is used for usage that cannot be attributed to a named user |
| `components` | object | (Optional, defaults to `{}`) Map of component name → (local username → `Usage`). Components are sub-categories of usage such as scheduler partitions or queue names |
| `num_jobs` | integer | Total number of jobs that started during this day (scalar total across all users) |
| `total_wait_seconds` | integer | Total queue wait time in seconds across all jobs that started this day (scalar total across all users). Defaults to `0` if absent (backwards-compatible) |
| `user_job_counts` | object | (Optional, defaults to `{}`) Map of local username → number of jobs started by that user. Defaults to empty if absent (backwards-compatible) |
| `user_wait_seconds` | object | (Optional, defaults to `{}`) Map of local username → total queue wait seconds for that user's jobs. Defaults to empty if absent (backwards-compatible) |
| `is_complete` | boolean | `true` if all usage data for the day has been collected; `false` for partial/aggregated data |

**Backwards compatibility:** `total_wait_seconds`, `user_job_counts`, and
`user_wait_seconds` were added in a later release. Older serialised data will
lack these fields; readers should treat absent fields as `0` / empty map
respectively (which is the behaviour of `#[serde(default)]`). The scalar
totals `num_jobs` and `total_wait_seconds` must equal the sums across the
per-user maps when both are present.

---

### `ProjectUsageReport`

Returned by: `get_usage_report`, `get_local_usage_report`

Compute usage for a single project over a date range, indexed by calendar date.

```json
{
  "project": "myproject.waldur",
  "reports": {
    "2024-01-15": {
      "reports": {
        "alice_hpc": {"seconds": 7200},
        "bob_hpc":   {"seconds": 3600}
      },
      "components": {
        "gpu-partition": {
          "alice_hpc": {"seconds": 7200}
        }
      },
      "num_jobs": 10,
      "total_wait_seconds": 1500,
      "user_job_counts": {"alice_hpc": 7, "bob_hpc": 3},
      "user_wait_seconds": {"alice_hpc": 1050, "bob_hpc": 450},
      "is_complete": true
    },
    "2024-01-16": {
      "reports": {
        "alice_hpc": {"seconds": 1800}
      },
      "components": {},
      "num_jobs": 2,
      "total_wait_seconds": 240,
      "user_job_counts": {"alice_hpc": 2},
      "user_wait_seconds": {"alice_hpc": 240},
      "is_complete": true
    }
  },
  "users": {
    "alice.myproject.waldur": "alice_hpc",
    "bob.myproject.waldur":   "bob_hpc"
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `project` | string | `ProjectIdentifier` in `project.portal` format |
| `reports` | object | Map of date string (`YYYY-MM-DD`) → `DailyProjectUsageReport` |
| `users` | object | Map of `UserIdentifier` string → local username string. Provides the mapping used to translate local usernames back to OpenPortal identifiers |

**Empty report (no usage data for the requested period):**
```json
{
  "project": "myproject.waldur",
  "reports": {},
  "users": {}
}
```

---

### `UsageReport`

Returned by: `get_usage_reports`

Portal-level aggregate report containing `ProjectUsageReport` objects for all
active projects.

```json
{
  "portal": "waldur",
  "reports": {
    "myproject.waldur": {
      "project": "myproject.waldur",
      "reports": {
        "2024-01-15": {
          "reports": {"alice_hpc": {"seconds": 7200}},
          "components": {},
          "num_jobs": 5,
          "total_wait_seconds": 600,
          "user_job_counts": {"alice_hpc": 5},
          "user_wait_seconds": {"alice_hpc": 600},
          "is_complete": true
        }
      },
      "users": {
        "alice.myproject.waldur": "alice_hpc"
      }
    },
    "otherproject.waldur": {
      "project": "otherproject.waldur",
      "reports": {},
      "users": {}
    }
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `portal` | string | `PortalIdentifier` — the portal this report covers |
| `reports` | object | Map of `ProjectIdentifier` string → `ProjectUsageReport` |

---

### `ProjectStorageReport`

Returned by: `get_storage_report`, `get_local_storage_report`

A storage quota report for a single project. The top-level fields always hold
the **most recent** point-in-time snapshot. An optional `daily_reports` map
holds older snapshots keyed by calendar date (`YYYY-MM-DD`), with at most one
snapshot per date (the newest seen for that date). The date of the top-level
snapshot is never duplicated in `daily_reports`.

**Report with no history (typical single query result):**

```json
{
  "project":       "myproject.waldur",
  "generated_at":  "2024-03-10T14:23:00Z",
  "project_quotas": {
    "home":    {"limit": "unlimited"},
    "scratch": {"limit": "10.00 TB", "usage": "3.45 TB"},
    "project": {"limit": "20.00 TB", "usage": "8.12 TB"}
  },
  "user_quotas": {
    "alice.myproject.waldur": {
      "home":    {"limit": "100.00 GB", "usage": "42.00 GB"},
      "scratch": {"limit": "2.00 TB",   "usage": "0.87 TB"}
    },
    "bob.myproject.waldur": {
      "home":    {"limit": "100.00 GB", "usage": "10.00 GB"}
    }
  },
  "users": {
    "alice.myproject.waldur": "alice_hpc",
    "bob.myproject.waldur":   "bob_hpc"
  }
}
```

**Report with historical snapshots:**

```json
{
  "project":       "myproject.waldur",
  "generated_at":  "2024-03-12T09:00:00Z",
  "project_quotas": {"scratch": {"limit": "10.00 TB", "usage": "4.10 TB"}},
  "user_quotas":    {},
  "users":          {"alice.myproject.waldur": "alice_hpc"},
  "daily_reports": {
    "2024-03-10": {
      "project":        "myproject.waldur",
      "generated_at":   "2024-03-10T14:23:00Z",
      "project_quotas": {"scratch": {"limit": "10.00 TB", "usage": "3.45 TB"}},
      "user_quotas":    {}
    },
    "2024-03-11": {
      "project":        "myproject.waldur",
      "generated_at":   "2024-03-11T10:05:00Z",
      "project_quotas": {"scratch": {"limit": "10.00 TB", "usage": "3.80 TB"}},
      "user_quotas":    {}
    }
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `project` | string | `ProjectIdentifier` in `project.portal` format |
| `generated_at` | string | ISO 8601 UTC timestamp of the most recent snapshot |
| `project_quotas` | object | Map of volume name → `Quota` for the most recent snapshot |
| `user_quotas` | object | Map of `UserIdentifier` string → (volume name → `Quota`) for the most recent snapshot |
| `users` | object | Map of `UserIdentifier` string → local username string |
| `daily_reports` | object | *(Optional)* Map of `YYYY-MM-DD` date string → historical snapshot object. Omitted when empty. Each snapshot object has `project`, `generated_at`, `project_quotas`, and `user_quotas` fields only — **no `users` field** (the `users` mapping is held exclusively at the top level). The current top-level snapshot date is **not** stored here; `daily_reports()` returns all historical entries plus the current top-level snapshot, and `get_report(date)` retrieves a single day. |

**Merge semantics:** when two `ProjectStorageReport` values are combined (`+`
/ `+=` / `combine()`), the newer snapshot (by `generated_at`) becomes the
top-level state. The older snapshot is stored in `daily_reports` under its
calendar date. If both snapshots fall on the same calendar day, the older is
silently discarded. Historical entries from both sides are merged keeping the
newest snapshot per date. The `users` maps from both sides are merged into the
top-level `users` field (the newer report's entries take precedence for any
duplicate keys).

**Empty report (no quota data available):**

```json
{
  "project":        "myproject.waldur",
  "generated_at":   "2024-03-10T14:23:00Z",
  "project_quotas": {},
  "user_quotas":    {},
  "users":          {}
}
```

---

### `StorageReport`

Returned by: `get_storage_reports`

Portal-level aggregate report containing `ProjectStorageReport` objects for all
active projects.

```json
{
  "portal": "waldur",
  "reports": {
    "myproject.waldur": {
      "project":        "myproject.waldur",
      "generated_at":   "2024-03-10T14:23:00Z",
      "project_quotas": {
        "scratch": {"limit": "10.00 TB", "usage": "3.45 TB"}
      },
      "user_quotas": {
        "alice.myproject.waldur": {
          "scratch": {"limit": "2.00 TB", "usage": "0.87 TB"}
        }
      },
      "users": {
        "alice.myproject.waldur": "alice_hpc"
      }
    },
    "otherproject.waldur": {
      "project":        "otherproject.waldur",
      "generated_at":   "2024-03-10T14:23:01Z",
      "project_quotas": {},
      "user_quotas":    {},
      "users":          {}
    }
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `portal` | string | `PortalIdentifier` — the portal this report covers |
| `reports` | object | Map of `ProjectIdentifier` string → `ProjectStorageReport` |

---

### `Destinations`

Returned by: `get_offerings`

Serialised as a comma-separated list of dot-delimited destination strings,
wrapped in square brackets.

```json
"[portal.provider.cluster_a, portal.provider.cluster_b]"
```

Note: `Destinations` serialises as a single JSON string (not a JSON array),
matching the instruction protocol format.

---

## Type Name Reference

The `result_type` field of a `Job` uses the Rust type name as recorded in the
`NamedType` trait. The table below maps each type name to its corresponding JSON
structure:

| `result_type` value | JSON structure | Returned by |
|---------------------|----------------|-------------|
| `"None"` | `null` / `"{}"` | Most write instructions |
| `"bool"` | `true` or `false` | `is_*` instructions |
| `"String"` | JSON string | `get_home_dir`, `get_local_home_dir` |
| `"Vec<String>"` | JSON array of strings | `get_project_dirs`, `get_local_project_dirs` |
| `"UserMapping"` | Mapping string | `get_user_mapping` |
| `"ProjectMapping"` | Mapping string | `get_project_mapping` |
| `"Vec<UserIdentifier>"` | Array of identifier strings | `get_users` |
| `"ProjectDetails"` | Object | `get_project`, `get_award` |
| `"Vec<ProjectDetails>"` | Array of objects | `get_projects`, `get_awards` |
| `"Usage"` | `{"seconds": <u64>}` | `get_limit`, `get_local_limit` |
| `"Quota"` | `{"limit": "…", "usage": "…"}` | `get_*_quota` |
| `"HashMap<Volume, Quota>"` | Object: volume → Quota | `get_*_quotas` |
| `"ProjectUsageReport"` | Object (see above) | `get_usage_report`, `get_local_usage_report` |
| `"UsageReport"` | Object (see above) | `get_usage_reports` |
| `"ProjectStorageReport"` | Object (see above) | `get_storage_report`, `get_local_storage_report` |
| `"StorageReport"` | Object (see above) | `get_storage_reports` |
| `"Destinations"` | String | `get_offerings` |
| `"Error"` | plain-text string | Any failed job |

---

## Implementation Notes

- All JSON serialisation uses `serde_json`. Types implement `Serialize` and
  `Deserialize` from the `serde` crate.
- `Usage` serialises as `{"seconds": <u64>}` because the struct uses the
  default derived serde implementation.
- `StorageSize` and `QuotaLimit` serialise as human-readable strings (e.g.
  `"5.00 TB"`, `"unlimited"`) rather than raw byte counts.
- `Volume` uses `#[serde(transparent)]`, so it appears as a bare JSON string
  rather than a `{"name": "…"}` object.
- `StorageUsage` also uses `#[serde(transparent)]`, delegating to `StorageSize`'s
  string serialisation.
- `Quota.usage` is skipped when serialising if it is `None`
  (`#[serde(skip_serializing_if = "Option::is_none")]`), so the field is simply
  absent in the JSON rather than present as `null`.
- Timestamps in `Job` use Unix seconds (via `chrono::serde::ts_seconds`).
- The key types `UserIdentifier`, `ProjectIdentifier`, `PortalIdentifier`,
  `UserMapping`, `ProjectMapping`, and `Destination` all serialise as plain strings
  (via custom `Serialize`/`Deserialize` impls that delegate to `to_string()` and
  `parse()`).
- Source files: `templemeads/src/job.rs` (`Job`, `Status`),
  `templemeads/src/usagereport.rs` (`Usage`, `UserUsageReport`,
  `DailyProjectUsageReport`, `ProjectUsageReport`, `UsageReport`),
  `templemeads/src/storagereport.rs` (`ProjectStorageReport`, `StorageReport`),
  `templemeads/src/storage.rs` (`StorageSize`, `QuotaLimit`, `Quota`, `Volume`),
  `templemeads/src/grammar.rs` (`AwardDetails` / `ProjectDetails`, identifiers and mappings).
