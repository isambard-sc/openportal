<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Agent Instruction Protocol

This document formally specifies the instruction protocol used for communication
between agents in the OpenPortal system. Instructions are text strings serialised
as Job payloads and transmitted over the paddington secure WebSocket layer.

## Instruction String Format

An instruction is a single-line text string:

```
<command> [<arg1> [<arg2> [...]]]
```

The command keyword is separated from arguments by single spaces. Compound argument
types use internal delimiters (`.`, `:`) that do not conflict with the space delimiter.
The exception is `AwardDetails` (also accepted as `ProjectDetails` for backward
compatibility), which is a JSON object and may contain spaces; it always appears
as the final argument of its instruction.

Instruction strings are the canonical serialisation format: `Instruction::to_string()`
produces them and `Instruction::parse()` consumes them.

---

## Identifier Types

### PortalIdentifier

Identifies a portal (e.g. Waldur).

```
<portal>
```

Constraints: non-empty; no spaces; no `.` characters.

Example: `waldur`

---

### ProjectIdentifier

Identifies a project within a portal.

```
<project>.<portal>
```

Constraints: each component is non-empty and contains no spaces.

Example: `myproject.waldur`

---

### UserIdentifier

Identifies a user within a project within a portal.

```
<username>.<project>.<portal>
```

Constraints: each component is non-empty and contains no spaces.

Example: `alice.myproject.waldur`

---

### ProjectMapping

Maps a `ProjectIdentifier` to a local system group name.

```
<project>.<portal>:<local_group>
```

Constraints: `local_group` must not be empty, and must not start or end with `.`
or `/`.

Example: `myproject.waldur:hpc_myproject`

---

### UserMapping

Maps a `UserIdentifier` to a local username and local group.

```
<username>.<project>.<portal>:<local_user>:<local_group>
```

Constraints: neither `local_user` nor `local_group` may be empty, or start/end
with `.` or `/`.

Example: `alice.myproject.waldur:alice_hpc:hpc_myproject`

---

### Destination

A dot-delimited path of agent names describing the routing path for a job.
Must contain at least two components.

```
<agent1>.<agent2>[.<agent3>...]
```

Example: `portal.provider.cluster_platform.cluster_instance`

---

### Destinations

A comma-separated list of `Destination` values, optionally enclosed in square
brackets.

```
[<dest1>, <dest2>, ...]
```

or without brackets:

```
<dest1>, <dest2>, ...
```

Example: `[portal.provider.cluster_a, portal.provider.cluster_b]`

---

## Data Types

### DateRange

Specifies a range of dates for usage queries.

**Named periods** (case-insensitive):

| Keyword | Meaning |
|---------|---------|
| `yesterday` | Previous calendar day |
| `today` or `this_day` | Current calendar day |
| `tomorrow` | Next calendar day |
| `this_week` | Current Mon–Sun week (also the default when omitted) |
| `last_week` | Previous Mon–Sun week |
| `this_month` | Current calendar month |
| `last_month` | Previous calendar month |
| `this_year` | Current calendar year |
| `last_year` | Previous calendar year |

**Explicit range:**

```
<YYYY-MM-DD>:<YYYY-MM-DD>
```

Start date is inclusive; end date is inclusive. If both dates are the same,
the single-date form may be used:

```
<YYYY-MM-DD>
```

Example: `2024-01-01:2024-03-31`

---

### Volume

Identifies a named storage volume (e.g. `home`, `scratch`, `project`).

```
<volume_name>
```

Constraints: non-empty; no spaces.

---

### QuotaLimit

A storage quota limit: either a concrete size or `unlimited`.

```
<size> | unlimited
```

**Size format:** a number followed immediately by a unit (case-insensitive):

| Unit | Meaning |
|------|---------|
| `B` or `BYTES` | Bytes |
| `KB` or `KILOBYTES` | Kibibytes (1024 B) |
| `MB` or `MEGABYTES` | Mebibytes |
| `GB` or `GIGABYTES` | Gibibytes |
| `TB` or `TERABYTES` | Tebibytes |
| `PB` or `PETABYTES` | Pebibytes |

Examples: `100GB`, `2TB`, `unlimited`

---

### Usage (compute time)

Compute usage expressed as an integer number of seconds.

```
<integer>
```

Example: `3600` (one hour)

---

### Link

A reference to an external resource. Both fields are optional.

```json
{ "id": "<string>", "url": "<url>" }
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Human-readable identifier, e.g. `"EP/X000000/1"` |
| `url` | string | Valid URL pointing to the resource |

---

### Note

A timestamped message attached to an award.

```json
{ "timestamp": "<RFC 3339 UTC>", "author": "<string>", "text": "<string>" }
```

| Field | Type | Description |
|-------|------|-------------|
| `timestamp` | string | ISO 8601 / RFC 3339 UTC timestamp, e.g. `"2024-01-15T10:30:00Z"` |
| `author` | string | Name of the person who created the note |
| `text` | string | Free-text content of the note |

---

### AwardDetails

Award/project metadata serialised as a JSON object. All fields are optional,
enabling partial updates. This type is also accepted under the legacy name
`ProjectDetails` for backward compatibility.

```json
{
  "name":             "<string>",
  "template":         "<string>",
  "key":              "<string>",
  "description":      "<string>",
  "members":          { "<email>": "<role>", ... },
  "start_date":       "<YYYY-MM-DD>",
  "end_date":         "<YYYY-MM-DD>",
  "allocation":       "<size> <units>",
  "breakdown":        { "<key>": "<value>", ... },
  "award":            { "id": "<string>", "url": "<url>" },
  "call":             { "id": "<string>", "url": "<url>" },
  "project_link":     { "id": "<string>", "url": "<url>" },
  "renewal":          { "id": "<string>", "url": "<url>" },
  "notes":            [ { "timestamp": "<RFC 3339>", "author": "<string>", "text": "<string>" } ],
  "earliest_approve": "<RFC 3339 UTC>",
  "allowed_domains":  ["<domain_pattern>", ...]
}
```

**`template`**: alphanumeric characters, underscores and dashes only; no spaces.

**`members`**: keys are email addresses, values are role strings. Keys are
serialised in ascending alphabetical order. Omitted when empty.

**`breakdown`**: free-form map of named allocation components. Keys and values
are arbitrary strings agreed between the local and remote portals — OpenPortal
does not interpret them. Omitted when empty. Example:
```json
{
  "gpu_hours":             "500 GPUHR",
  "interactive_cpu_hours": "1000 CPUHR",
  "project_storage":       "5 TB",
  "user_storage":          "500 GB"
}
```
On `merge` / `update_award`, entries from the incoming object overwrite matching
keys in the existing map; new keys are added; existing keys not mentioned are
unchanged.

**`award`**: link back to the award record on the funding body's system (e.g. UKRI GtR).

**`call`**: link to the funding call from which the award was made.

**`project_link`**: link to the project page on the remote/awarding portal,
so local users can navigate there.

**`renewal`**: link to the page where more time or a renewal can be requested.

**`notes`**: append-only list of `Note` objects, sorted by timestamp.
Omitted from JSON when empty; deserialises to `[]` when absent.
Display format per note: `[YYYY-MM-DD HH:MM UTC — Author] text`.

**`earliest_approve`**: RFC 3339 UTC timestamp before which the receiving portal
must not approve or provision this award. Omitted when not set.

**`allocation` units** (canonical forms, case-insensitive aliases accepted):

| Canonical | Aliases | Meaning |
|-----------|---------|---------|
| `NHR` | `node hours`, `node hour` | Node-hours |
| `GPUHR` | `gpu hours`, `gpu hour` | GPU-hours |
| `CPUHR` | `cpu hours`, `cpu hour` | CPU-hours |
| `COREHR` | `core hours`, `core hour` | Core-hours |
| `GBHR` | `gb hours`, `gb hour` | Gigabyte-hours |
| `BHR` | `billing hours`, `billing hour` | Billing-hours |

Use `"No allocation"` or `"none"` to represent no allocation.

**`allowed_domains`**: each entry is either an exact domain (`"example.com"`) or
a single-level wildcard (`"*.example.com"`). `null` means all domains are
permitted; `[]` means no domains are permitted.

---

## Instructions

### `submit`

Wraps another instruction with an explicit routing destination. Used to forward
a job along a specified path through the agent hierarchy.

```
submit <destination> <instruction>
```

`<instruction>` is the full text of any other instruction (parsed recursively).

**Example:**
```
submit portal.provider.cluster_platform.cluster_a add_user alice.proj.waldur
```

---

### Project Instructions

#### `create_project` / `create_award`

Create a new project with the given identifier and metadata.
`create_award` is accepted as a synonym; `create_project` is the canonical
wire form for now (until all agents are migrated).

```
create_project <project_id> <award_details_json>
```

#### `update_project` / `update_award`

Update metadata for an existing project. Only fields present in the JSON are
changed. `update_award` is accepted as a synonym; `update_project` is the
canonical wire form for now.

```
update_project <project_id> <award_details_json>
```

#### `get_project`

Retrieve the full details of a single project.

```
get_project <project_id>
```

Returns: `ProjectDetails` (`AwardDetails`)

#### `get_projects`

List all projects managed by a portal.

```
get_projects <portal_id>
```

Returns: `Vec<ProjectDetails>` (`Vec<AwardDetails>`)

#### `get_award`

Retrieve the award details for a single project.

```
get_award <project_id>
```

Returns: `ProjectDetails` (`AwardDetails`)

#### `get_awards` / `list_awards`

List the award details for all projects managed by a portal.
`list_awards` is accepted as a synonym.

```
get_awards <portal_id>
```

Returns: `Vec<ProjectDetails>` (`Vec<AwardDetails>`)

#### `add_project`

Register a project with an agent (add it to the agent's management scope).

```
add_project <project_id>
```

#### `remove_project`

Deregister a project from an agent's management scope.

```
remove_project <project_id>
```

#### `is_existing_project`

Check whether a project exists on the target system.

```
is_existing_project <project_id>
```

Returns: `bool`

---

### User Instructions

#### `get_users`

List all users in a project.

```
get_users <project_id>
```

Returns: `Vec<UserIdentifier>`

#### `add_user`

Add a user to a project.

```
add_user <user_id>
```

#### `remove_user`

Remove a user from a project.

```
remove_user <user_id>
```

#### `is_protected_user`

Check whether a user is protected (i.e. should not be managed by OpenPortal).

```
is_protected_user <user_id>
```

Returns: `bool`

#### `is_existing_user`

Check whether a local user account already exists.

```
is_existing_user <user_id>
```

Returns: `bool`

---

### Mapping Instructions

These translate between OpenPortal identifiers and local system names.

#### `get_user_mapping`

Look up the local account mapping for a user.

```
get_user_mapping <user_id>
```

Returns: `UserMapping`

#### `get_project_mapping`

Look up the local group mapping for a project.

```
get_project_mapping <project_id>
```

Returns: `ProjectMapping`

---

### Directory Instructions

#### `get_home_dir`

Look up the home directory path for a user. The directory may not yet exist.

```
get_home_dir <user_id>
```

Returns: filesystem path string

#### `get_user_dirs`

Look up user-specific directory paths for a user. Directories may not yet exist.

```
get_user_dirs <user_id>
```

Returns: `Vec<String>` (filesystem paths)

> **Note:** `get_user_dirs` is defined in the Instruction type but is not yet
> handled by the parser. It cannot be round-tripped through `parse()`.

#### `get_project_dirs`

Look up project directory paths for a project. Directories may not yet exist.

```
get_project_dirs <project_id>
```

Returns: `Vec<String>` (filesystem paths)

#### `update_homedir`

Notify an agent of the actual home directory path for a user after it has been
created.

```
update_homedir <user_id> <path>
```

---

### Local Account Instructions

Used by account agents (e.g. FreeIPA) that directly manage local accounts.

#### `add_local_user`

Create a local user account described by a user mapping.

```
add_local_user <user_mapping>
```

#### `remove_local_user`

Remove a local user account described by a user mapping.

```
remove_local_user <user_mapping>
```

#### `add_local_project`

Create a local project group described by a project mapping.

```
add_local_project <project_mapping>
```

#### `remove_local_project`

Remove a local project group described by a project mapping.

```
remove_local_project <project_mapping>
```

#### `get_local_home_dir`

Retrieve the home directory path for a locally mapped user. The directory may
not yet exist.

```
get_local_home_dir <user_mapping>
```

Returns: filesystem path string

#### `get_local_user_dirs`

Retrieve user-specific directory paths for a locally mapped user. Directories
may not yet exist.

```
get_local_user_dirs <user_mapping>
```

Returns: `Vec<String>` (filesystem paths)

> **Note:** `get_local_user_dirs` is defined in the Instruction type but is not
> yet handled by the parser. It cannot be round-tripped through `parse()`.

#### `get_local_project_dirs`

Retrieve project directory paths for a locally mapped project. Directories may
not yet exist.

```
get_local_project_dirs <project_mapping>
```

Returns: `Vec<String>` (filesystem paths)

---

### Usage Reporting Instructions

#### `get_usage_report`

Get the compute usage report for a project over a date range.

```
get_usage_report <project_id> [<date_range>]
```

If `<date_range>` is omitted it defaults to `this_week`.

Returns: `ProjectUsageReport`

#### `get_usage_reports`

Get compute usage reports for all active projects in a portal over a date range.

```
get_usage_reports <portal_id> [<date_range>]
```

If `<date_range>` is omitted it defaults to `this_week`.

Returns: `Vec<ProjectUsageReport>`

#### `get_local_usage_report`

Get a local compute usage report for a locally mapped project over a date range.

```
get_local_usage_report <project_mapping> [<date_range>]
```

If `<date_range>` is omitted it defaults to `this_week`.

Returns: `ProjectUsageReport`

---

### Storage Reporting Instructions

These instructions return point-in-time snapshots of storage quota and usage
across all volumes for a project. An optional date range may be supplied; it
defaults to `today`. The underlying filesystem agent only supports `today` —
any other range will cause the job to fail with an error.

#### `get_storage_report`

Get the storage quota report for a project across all volumes and all users.
If no date range is given, the report covers today only.

```
get_storage_report <project_id> [<date_range>]
```

Returns: `ProjectStorageReport`

#### `get_storage_reports`

Get storage quota reports for all active projects in a portal.
If no date range is given, the reports cover today only.

```
get_storage_reports <portal_id> [<date_range>]
```

Returns: `StorageReport`

#### `get_local_storage_report`

Get a local storage quota report for a locally mapped project. Sent by the
cluster instance agent to the filesystem agent. The filesystem agent fetches
project and per-user quotas locally, calling back to the sender with
`get_users <project_id>` to obtain the member list.

If no date range is given, it defaults to `today`. The filesystem agent will
return an error if the requested range is anything other than today.

```
get_local_storage_report <project_mapping> [<date_range>]
```

Returns: `ProjectStorageReport`

---

### Compute Limit Instructions

#### `set_limit`

Set a compute usage limit for a project (expressed in seconds).

```
set_limit <project_id> <seconds>
```

#### `get_limit`

Get the current compute usage limit for a project.

```
get_limit <project_id>
```

Returns: `Usage` (seconds)

#### `set_local_limit`

Set a compute usage limit for a locally mapped project (expressed in seconds).

```
set_local_limit <project_mapping> <seconds>
```

#### `get_local_limit`

Get the current compute usage limit for a locally mapped project.

```
get_local_limit <project_mapping>
```

Returns: `Usage` (seconds)

---

### Storage Quota Instructions — Portal Level

These instructions operate on projects/users identified by OpenPortal identifiers.

#### `set_project_quota`

Set the storage quota for a project on a named volume.

```
set_project_quota <project_id> <volume> <quota_limit>
```

#### `get_project_quota`

Get the current storage quota for a project on a named volume.

```
get_project_quota <project_id> <volume>
```

Returns: `Quota`

#### `clear_project_quota`

Remove (clear) the storage quota for a project on a named volume.

```
clear_project_quota <project_id> <volume>
```

#### `get_project_quotas`

Get all storage quotas for a project across all volumes.

```
get_project_quotas <project_id>
```

Returns: `HashMap<Volume, Quota>`

#### `set_user_quota`

Set the storage quota for a user on a named volume.

```
set_user_quota <user_id> <volume> <quota_limit>
```

#### `get_user_quota`

Get the current storage quota for a user on a named volume.

```
get_user_quota <user_id> <volume>
```

Returns: `Quota`

#### `clear_user_quota`

Remove (clear) the storage quota for a user on a named volume.

```
clear_user_quota <user_id> <volume>
```

#### `get_user_quotas`

Get all storage quotas for a user across all volumes.

```
get_user_quotas <user_id>
```

Returns: `HashMap<Volume, Quota>`

---

### Storage Quota Instructions — Local/Mapped Level

These instructions operate on locally mapped projects/users.

#### `set_local_project_quota`

Set the storage quota for a locally mapped project on a named volume.

```
set_local_project_quota <project_mapping> <volume> <quota_limit>
```

#### `get_local_project_quota`

Get the storage quota for a locally mapped project on a named volume.

```
get_local_project_quota <project_mapping> <volume>
```

Returns: `Quota`

#### `clear_local_project_quota`

Remove (clear) the storage quota for a locally mapped project on a named volume.

```
clear_local_project_quota <project_mapping> <volume>
```

#### `get_local_project_quotas`

Get all storage quotas for a locally mapped project.

```
get_local_project_quotas <project_mapping>
```

Returns: `HashMap<Volume, Quota>`

#### `set_local_user_quota`

Set the storage quota for a locally mapped user on a named volume.

```
set_local_user_quota <user_mapping> <volume> <quota_limit>
```

#### `get_local_user_quota`

Get the storage quota for a locally mapped user on a named volume.

```
get_local_user_quota <user_mapping> <volume>
```

Returns: `Quota`

#### `clear_local_user_quota`

Remove (clear) the storage quota for a locally mapped user on a named volume.

```
clear_local_user_quota <user_mapping> <volume>
```

#### `get_local_user_quotas`

Get all storage quotas for a locally mapped user.

```
get_local_user_quotas <user_mapping>
```

Returns: `HashMap<Volume, Quota>`

---

### Offerings Instructions

Offerings describe the set of destinations/resources an agent can route jobs to.

#### `sync_offerings`

Replace the complete list of offerings for an agent with the supplied set.

```
sync_offerings <destinations>
```

#### `add_offerings`

Add one or more offerings to an agent.

```
add_offerings <destinations>
```

#### `remove_offerings`

Remove one or more offerings from an agent.

```
remove_offerings <destinations>
```

#### `get_offerings`

Retrieve the current list of offerings from an agent.

```
get_offerings
```

Returns: `Destinations`

---

## Complete Instruction Reference

| Command | Arguments | Returns | Description |
|---------|-----------|---------|-------------|
| `submit` | `<destination> <instruction>` | — | Route instruction to a specific destination |
| `create_project` / `create_award` | `<project_id> <details_json>` | — | Create a project |
| `update_project` / `update_award` | `<project_id> <details_json>` | — | Update project metadata |
| `get_project` | `<project_id>` | `ProjectDetails` | Retrieve project details |
| `get_projects` | `<portal_id>` | `Vec<ProjectDetails>` | List all projects for a portal |
| `get_award` | `<project_id>` | `ProjectDetails` | Retrieve award details for a project |
| `get_awards` / `list_awards` | `<portal_id>` | `Vec<ProjectDetails>` | List award details for all projects |
| `add_project` | `<project_id>` | — | Add project to agent scope |
| `remove_project` | `<project_id>` | — | Remove project from agent scope |
| `is_existing_project` | `<project_id>` | `bool` | Check if project exists |
| `get_users` | `<project_id>` | `Vec<UserIdentifier>` | List users in a project |
| `add_user` | `<user_id>` | — | Add user to project |
| `remove_user` | `<user_id>` | — | Remove user from project |
| `is_protected_user` | `<user_id>` | `bool` | Check if user is protected |
| `is_existing_user` | `<user_id>` | `bool` | Check if user account exists |
| `get_user_mapping` | `<user_id>` | `UserMapping` | Get local mapping for user |
| `get_project_mapping` | `<project_id>` | `ProjectMapping` | Get local mapping for project |
| `get_home_dir` | `<user_id>` | `String` | Get user home directory path |
| `get_user_dirs` | `<user_id>` | `Vec<String>` | Get user directories *(not yet parseable)* |
| `get_project_dirs` | `<project_id>` | `Vec<String>` | Get project directories |
| `update_homedir` | `<user_id> <path>` | — | Notify agent of user home directory |
| `add_local_user` | `<user_mapping>` | — | Create local user account |
| `remove_local_user` | `<user_mapping>` | — | Remove local user account |
| `add_local_project` | `<project_mapping>` | — | Create local project group |
| `remove_local_project` | `<project_mapping>` | — | Remove local project group |
| `get_local_home_dir` | `<user_mapping>` | `String` | Get local user home dir |
| `get_local_user_dirs` | `<user_mapping>` | `Vec<String>` | Get local user dirs *(not yet parseable)* |
| `get_local_project_dirs` | `<project_mapping>` | `Vec<String>` | Get local project dirs |
| `get_usage_report` | `<project_id> [<date_range>]` | `ProjectUsageReport` | Usage report for project |
| `get_usage_reports` | `<portal_id> [<date_range>]` | `Vec<ProjectUsageReport>` | Usage reports for all portal projects |
| `get_local_usage_report` | `<project_mapping> [<date_range>]` | `ProjectUsageReport` | Local usage report |
| `get_storage_report` | `<project_id> [<date_range>]` | `ProjectStorageReport` | Storage quota report for project (default: today; filesystem agent only supports today) |
| `get_storage_reports` | `<portal_id> [<date_range>]` | `StorageReport` | Storage quota reports for all portal projects (default: today) |
| `get_local_storage_report` | `<project_mapping> [<date_range>]` | `ProjectStorageReport` | Local storage quota report (filesystem agent only; errors if range ≠ today) |
| `set_limit` | `<project_id> <seconds>` | — | Set compute limit for project |
| `get_limit` | `<project_id>` | `Usage` | Get compute limit for project |
| `set_local_limit` | `<project_mapping> <seconds>` | — | Set local compute limit |
| `get_local_limit` | `<project_mapping>` | `Usage` | Get local compute limit |
| `set_project_quota` | `<project_id> <volume> <limit>` | — | Set project storage quota |
| `get_project_quota` | `<project_id> <volume>` | `Quota` | Get project storage quota |
| `clear_project_quota` | `<project_id> <volume>` | — | Clear project storage quota |
| `get_project_quotas` | `<project_id>` | `HashMap<Volume,Quota>` | Get all project quotas |
| `set_user_quota` | `<user_id> <volume> <limit>` | — | Set user storage quota |
| `get_user_quota` | `<user_id> <volume>` | `Quota` | Get user storage quota |
| `clear_user_quota` | `<user_id> <volume>` | — | Clear user storage quota |
| `get_user_quotas` | `<user_id>` | `HashMap<Volume,Quota>` | Get all user quotas |
| `set_local_project_quota` | `<project_mapping> <volume> <limit>` | — | Set local project quota |
| `get_local_project_quota` | `<project_mapping> <volume>` | `Quota` | Get local project quota |
| `clear_local_project_quota` | `<project_mapping> <volume>` | — | Clear local project quota |
| `get_local_project_quotas` | `<project_mapping>` | `HashMap<Volume,Quota>` | Get all local project quotas |
| `set_local_user_quota` | `<user_mapping> <volume> <limit>` | — | Set local user quota |
| `get_local_user_quota` | `<user_mapping> <volume>` | `Quota` | Get local user quota |
| `clear_local_user_quota` | `<user_mapping> <volume>` | — | Clear local user quota |
| `get_local_user_quotas` | `<user_mapping>` | `HashMap<Volume,Quota>` | Get all local user quotas |
| `sync_offerings` | `<destinations>` | — | Replace all offerings |
| `add_offerings` | `<destinations>` | — | Add new offerings |
| `remove_offerings` | `<destinations>` | — | Remove offerings |
| `get_offerings` | *(none)* | `Destinations` | Get current offerings |

---

## Examples

```
# Add a user to a project
add_user alice.myproject.waldur

# Remove a user from a project
remove_user alice.myproject.waldur

# Get usage report for last month
get_usage_report myproject.waldur last_month

# Get usage report for an explicit date range
get_usage_report myproject.waldur 2024-01-01:2024-03-31

# Get usage reports for all waldur projects this week (date_range omitted = this_week)
get_usage_reports waldur

# Set a 5 TB storage quota for a project on the scratch volume
set_project_quota myproject.waldur scratch 5TB

# Clear a user's home volume quota
clear_user_quota alice.myproject.waldur home

# Set a compute limit of 1 000 000 node-seconds for a project
set_limit myproject.waldur 1000000

# Create a local user account using a mapping
add_local_user alice.myproject.waldur:alice_hpc:hpc_myproject

# Route an instruction to a specific agent path
submit portal.provider.cluster_platform.cluster_a add_user alice.myproject.waldur

# Create a project with full metadata
create_project myproject.waldur {"name":"My Project","template":"cpu-cluster","allocation":"1000 NHR","start_date":"2024-01-01","end_date":"2024-12-31"}

# Sync available cluster offerings
sync_offerings [portal.provider.cluster_a, portal.provider.cluster_b]

# Set a local project quota with a mapped name
set_local_project_quota myproject.waldur:hpc_myproject scratch 2TB

# Get all quotas for a user
get_user_quotas alice.myproject.waldur

# Get a point-in-time storage report for a project (all volumes, all users) — defaults to today
get_storage_report myproject.waldur

# Get a storage report for a project for the current month (will error — only today is supported)
get_storage_report myproject.waldur this_month

# Get storage reports for all projects in a portal
get_storage_reports waldur
```

---

## Implementation Notes

- The canonical implementation is in `templemeads/src/grammar.rs` (`Instruction` enum,
  `Instruction::parse()`, and `Instruction::fmt()`).
- Supporting types are in `templemeads/src/storage.rs` (`Volume`, `QuotaLimit`,
  `StorageSize`), `templemeads/src/usagereport.rs` (`Usage`), and
  `templemeads/src/storagereport.rs` (`ProjectStorageReport`, `StorageReport`).
- Routing types (`Destination`, `Destinations`) are in
  `templemeads/src/destination.rs`.
- Instructions are serialised to/from JSON transparently as their string
  representation (via `Serialize`/`Deserialize` impls that delegate to
  `to_string()`/`parse()`).
- Two instructions (`get_user_dirs`, `get_local_user_dirs`) exist in the enum and
  are emitted by `Display` but are not yet handled by `parse()`, meaning they
  cannot be deserialised from a string.
- `get_local_storage_report` is an internal instruction: it is sent by the cluster
  instance agent to the filesystem agent and is not intended to be issued by
  external callers. The filesystem agent calls back to the sender with `get_users`
  to retrieve the project member list. The filesystem agent rejects any date range
  other than `today` with an error.
