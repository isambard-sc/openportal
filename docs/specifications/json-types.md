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

Returned by: `get_project`

A JSON object. All fields are optional and may be absent if not set by the portal.

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
  "award": {
    "id":   "EP/X000000/1",
    "link": "https://example.com/award/EP-X000000-1"
  },
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
| `award` | object | Award details (see below) |
| `allowed_domains` | array of strings | Domain allow-list; `null` = all; `[]` = none |

**`award` object:**

```json
{
  "id":   "<award-id>",
  "link": "<url>"
}
```

Both `id` and `link` are optional and may be `null`.

---

### `Vec<ProjectDetails>`

Returned by: `get_projects`

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
  "is_complete": true
}
```

| Field | Type | Description |
|-------|------|-------------|
| `reports` | object | Map of local username → `Usage`. The key `"unknown"` is used for usage that cannot be attributed to a named user |
| `components` | object | (Optional, defaults to `{}`) Map of component name → (local username → `Usage`). Components are sub-categories of usage such as scheduler partitions or queue names |
| `num_jobs` | integer | Number of jobs that ran during this day |
| `is_complete` | boolean | `true` if all usage data for the day has been collected; `false` for partial/aggregated data |

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
      "is_complete": true
    },
    "2024-01-16": {
      "reports": {
        "alice_hpc": {"seconds": 1800}
      },
      "components": {},
      "num_jobs": 2,
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
| `"ProjectDetails"` | Object | `get_project` |
| `"Vec<ProjectDetails>"` | Array of objects | `get_projects` |
| `"Usage"` | `{"seconds": <u64>}` | `get_limit`, `get_local_limit` |
| `"Quota"` | `{"limit": "…", "usage": "…"}` | `get_*_quota` |
| `"HashMap<Volume, Quota>"` | Object: volume → Quota | `get_*_quotas` |
| `"ProjectUsageReport"` | Object (see above) | `get_usage_report`, `get_local_usage_report` |
| `"UsageReport"` | Object (see above) | `get_usage_reports` |
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
  `templemeads/src/storage.rs` (`StorageSize`, `QuotaLimit`, `Quota`, `Volume`),
  `templemeads/src/grammar.rs` (`ProjectDetails`, identifiers and mappings).
