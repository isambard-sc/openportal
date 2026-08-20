<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Python API Reference

The `openportal` Python module is a compiled Rust extension (built with
[pyo3](https://pyo3.rs)) that wraps the bridge HTTP API in a synchronous,
blocking Python interface. It communicates with a running `op-bridge` agent
over localhost HTTP.

## Installation

Build and install the Python module from the workspace root:

```bash
make python
# or
maturin develop -m python/Cargo.toml
```

This installs the `openportal` module into the current Python environment.

## A worked example

[`python/examples/site_portal/`](../../python/examples/site_portal/) is a complete, small,
heavily commented site portal built on this module - the two `signal_url`
endpoints, every instruction handler, and a test suite that runs without a
bridge. It is the fastest way to see how the pieces below fit together.

## Initialisation

Before calling any other function you must load the bridge configuration
file that was created when the bridge agent was initialised.

```python
import openportal

# Enable logging to stdout (optional but recommended during development)
openportal.initialize_tracing()

# Load the bridge config (default path: ~/.config/openportal/bridge.toml)
openportal.load_config("/path/to/bridge.toml")

# Check the config loaded successfully
assert openportal.is_config_loaded()
```

---

## Top-level functions

### Configuration

| Function | Signature | Description |
|---|---|---|
| `load_config` | `(config_file: str \| Path) → None` | Load the bridge TOML config and connect to the running `op-bridge` agent. Raises `OSError` on failure. |
| `is_config_loaded` | `() → bool` | Return `True` if a valid config has been loaded. |
| `initialize_tracing` | `() → None` | Enable tracing/logging output to stdout. |

### Running jobs

| Function | Signature | Description |
|---|---|---|
| `run` | `(command: str, max_ms: int = 0) → Job` | Submit a command to OpenPortal and return a `Job`. If `max_ms > 0`, blocks until the job finishes or the timeout elapses. If `max_ms < 0`, blocks indefinitely. If `max_ms == 0` (default), returns immediately without waiting. |
| `status` | `(job: Job) → Job` | Fetch the latest version of the given job from the bridge. |
| `get` | `(job_id: str \| Uuid) → Job` | Fetch the job with the specified ID. Raises `OSError` if the job does not exist. |
| `error_from_message` | `(message: str) → OpenPortalError` | Build the typed exception described by an OpenPortal error message. Accepts the raw `RuntimeError{…}` form or the bare `"<ClassName>: <message>"`. |
| `notify` | `(command: str) → None` | Send a fire-and-forget notification into the OpenPortal agent network. `command` is a notification string: `<destination> <event> [<argument>]`. Returns immediately — no result or acknowledgement is ever received. Raises `OSError` if the portal is not connected or the destination is invalid. See [notification-protocol.md](notification-protocol.md) for the full notification grammar and routing rules. |

### Bridge board (portal callbacks)

These functions are used when OpenPortal needs the portal to take action
(the OpenPortal → portal direction). See [bridge-api.md](bridge-api.md)
for the full two-direction communication model.

| Function | Signature | Description |
|---|---|---|
| `fetch_jobs` | `() → list[Job]` | Fetch all jobs that OpenPortal has queued for the portal to handle. |
| `fetch_job` | `(job_id: str \| Uuid) → Job` | Fetch a single queued job by ID. |
| `fetch_notification` | `(notification_id: str \| Uuid) → Notification` | Fetch a pending notification from the bridge by UUID. Called from the `notification_url` handler after the bridge sends its GET signal. Raises `OSError` if the UUID is not found. |
| `send_result` | `(job: Job) → None` | Send the completed or errored result of a bridge-board job back to OpenPortal. |
| `get_portal` | `() → PortalIdentifier` | Return the `PortalIdentifier` of the portal connected to the bridge. |

### Offerings

Offerings are `Destination` paths that this portal advertises as available
to the OpenPortal network. They are used by the provider and platform agents
to know which jobs can be routed to this portal.

| Function | Signature | Description |
|---|---|---|
| `sync_offerings` | `(offerings: list[Destination]) → list[Destination]` | Atomically replace the set of current offerings with the provided list. Returns the new active list. |
| `add_offerings` | `(offerings: list[Destination]) → list[Destination]` | Add destinations to the current offerings. Returns the updated list. |
| `remove_offerings` | `(offerings: list[Destination]) → list[Destination]` | Remove destinations from the current offerings. Returns the updated list. |
| `get_offerings` | `() → list[Destination]` | Return the current list of active offerings. |

### Operations

| Function | Signature | Description |
|---|---|---|
| `health` | `() → Health` | Return the health status of the bridge and connected agents. |
| `diagnostics` | `(destination: str) → Diagnostics` | Fetch a diagnostics report from the agent at `destination` (dot-path, e.g. `"portal.clusters"`). Pass `""` to query the bridge itself. |
| `restart` | `(restart_type: str, destination: str) → RestartResponse` | Request a restart of the agent at `destination`. `restart_type` is `"soft"` (graceful) or `"hard"` (immediate). Pass `""` to restart the bridge itself. |

---

## Classes

### `Job`

Represents a unit of work in the OpenPortal system.

**Properties (read-only):**

| Property | Type | Description |
|---|---|---|
| `id` | `Uuid` | Unique job identifier |
| `destination` | `Destination` | Full routing path (e.g. `portal.provider.clusters.cluster`) |
| `forwarded_for` | `Destination \| None` | Original destination before the portal rewrote it for the bridge (e.g. `remote.local.resource`). Set on bridge-board jobs created by the portal's virtual resource runner; `None` on all other jobs. Identifies the true originating portal. |
| `instruction` | `Instruction` | The parsed instruction (e.g. `AddUser`). `str(i)` returns the full instruction string; supports `==` / `!=` against another `Instruction` or a plain string. |
| `state` | `Status` | Current job state |
| `version` | `int` | Monotonically increasing version counter |
| `created` | `datetime` | UTC creation time |
| `changed` | `datetime` | UTC time of last state change |
| `is_finished` | `bool` | `True` if the job is in a terminal state (complete, error, expired, or duplicate) |
| `is_error` | `bool` | `True` if the job failed with an error |
| `is_expired` | `bool` | `True` if the job expired before completion |
| `is_duplicate` | `bool` | `True` if the job was detected as a duplicate of another pending job |
| `result` | `Any` | The deserialized job result once finished. Raises `OSError` if the job is not yet finished, or if the job is in an error state (use `error_message` instead). Returns `None` if the job completed with no result value. |
| `error_message` | `str` | Error description if `is_error`, otherwise `""`. The raw string, including any `<ClassName>: ` prefix. |
| `error` | `OpenPortalError \| None` | The failure as a typed exception, or `None` if the job did not fail. |
| `error_kind` | `str` | The machine-readable kind of the failure (e.g. `"award_pending"`, `"expired"`), or `""` if the job did not fail. Branch on this when the exception classes are not granular enough — notably for a kind contributed by a domain this module has no class for. Reconstructed from the message when the failure came from a peer that predates structured errors. |
| `progress_message` | `str` | In-progress status message if set, otherwise `""` |

**Methods:**

| Method | Signature | Description |
|---|---|---|
| `update` | `() → None` | Refresh this job in-place by fetching its latest status from the bridge. No-op if already finished. |
| `wait` | `(max_ms: int = 1000) → bool` | Block until the job is finished or `max_ms` milliseconds elapse. Pass a negative value to wait indefinitely. Returns `True` if the job is now finished. |
| `completed` | `(result) → Job` | Return a new copy of this job marked as complete with the given result. `result` may be a `str`, `bool`, `UserIdentifier`, `ProjectIdentifier`, `AwardDetails`, `ProjectUsageReport`, `UsageReport`, `ProjectStorageReport`, `StorageReport`, `Quota`, `Volume`, `StorageSize`, `StorageUsage`, `QuotaLimit`, `ProjectTemplate`, `DateRange`, or a `list` or `dict` of those types. Used when handling bridge-board jobs. |
| `errored` | `(error: str \| BaseException) → Job` | Return a new copy of this job marked as failed. Pass one of the exception classes below and the class travels with the message, so the caller recovers it; pass a plain string for an untyped failure. Used when handling bridge-board jobs. |
| `raise_for_error` | `() → None` | Raise this job's error as the matching exception class. Does nothing if the job did not fail. |
| `to_json` | `() → str` | Serialise the job to a JSON string. |
| `from_json` | `(json: str) → Job` | *(static)* Deserialise a job from a JSON string. |

**Usage pattern for a portal-side job:**

```python
# Submit and wait up to 30 seconds
job = openportal.run("portal.provider.clusters.mycluster add_user alice.myproject.myportal",
                     max_ms=30_000)

if job.is_error:
    print(f"Failed: {job.error_message}")
elif job.is_finished:
    print("Done")
else:
    print("Timed out, job still running")
```

**Usage pattern for a bridge-board job (OpenPortal → portal):**

```python
jobs = openportal.fetch_jobs()
for job in jobs:
    instruction = str(job.instruction)
    if instruction.startswith("GetProject "):
        project_id = instruction.split(" ", 1)[1]
        details = look_up_project(project_id)   # portal-side business logic
        completed_job = job.completed(details)
        openportal.send_result(completed_job)
    else:
        errored_job = job.errored(f"Unknown instruction: {instruction}")
        openportal.send_result(errored_job)
```

---

### `Notification`

A fire-and-forget notification received from the OpenPortal network. Construct
one from the JSON body that the bridge POSTs to `notification_url`, or parse
from a notification command string.

**Constructors:**

| Method | Signature | Description |
|---|---|---|
| `Notification` | `(command: str) → Notification` | Parse from `"<destination> <event> [<args>]"` string. Raises `OSError` on invalid input. |
| `Notification.parse` | `(command: str) → Notification` | Same as the constructor. |
| `Notification.from_json` | `(json: str) → Notification` | Deserialise from the JSON body posted to `notification_url`. |

**Properties (read-only):**

| Property | Type | Description |
|---|---|---|
| `id` | `str` | UUID string. For logging only — not stored anywhere. |
| `destination` | `str` | Dot-separated routing path, e.g. `"portal.clusters.shared"`. |
| `event` | `str` | Full event string including all arguments, e.g. `"user_added chris.p.portal"`. |
| `event_type` | `str` | The event keyword alone, e.g. `"user_added"`. Use this for dispatch. |
| `event_argument` | `str` | Everything after the event keyword. Empty string if the event carries no arguments. For multi-argument events this will include all arguments and their spaces. |

**Methods:**

| Method | Signature | Description |
|---|---|---|
| `to_json` | `() → str` | Serialise to a JSON string. |

**Usage pattern — pull model:**

The bridge signals your `notification_url` endpoint with a GET request
carrying the notification UUID. Your handler fetches the full notification
via `fetch_notification`, processes it, and returns HTTP 200:

```python
import openportal

# Django view for GET <notification_url>?notification_id=<uuid>
def notification_signal(request):
    notification_id = request.GET.get("notification_id")
    if not notification_id:
        return HttpResponseBadRequest("missing notification_id")

    try:
        n = openportal.fetch_notification(notification_id)
    except OSError:
        # UUID not found — bridge may have already removed it; ignore
        return HttpResponse(status=200)

    match n.event_type:
        case "user_added":
            provision_user(openportal.UserIdentifier(n.event_argument))
        case "user_removed":
            deprovision_user(openportal.UserIdentifier(n.event_argument))
        case "project_added":
            create_project(openportal.ProjectIdentifier(n.event_argument))
        case "project_removed":
            delete_project(openportal.ProjectIdentifier(n.event_argument))
        case "award_added" | "award_changed":
            sync_award(openportal.ProjectIdentifier(n.event_argument))
        case "award_removed":
            remove_award(openportal.ProjectIdentifier(n.event_argument))
        case _:
            pass  # ignore events we don't handle

    return HttpResponse(status=200)
```

Returning a non-2xx response causes the bridge to retry up to 3 times with a
2-second delay between attempts, so transient failures are handled
automatically. Make your handler idempotent — the same notification may be
delivered more than once if a retry races with a successful fetch.

`str(notification)` returns the full notification string:
`"<destination> <event_type> <event_argument>"`.

---

### `Status`

Represents the state of a job. String representation matches the job state
names used throughout the protocol.

**Static constructors:** `Status.created()`, `Status.pending()`,
`Status.running()`, `Status.complete()`, `Status.error()`, `Status.duplicate()`

There is no `Status.expired()` — expiry is not one of the six states. A job
expires by passing its `expires` timestamp while still unfinished, which is why
it is read from `job.is_expired` rather than compared against a state.

`Status("running")` constructs from a string. `str(s)` returns the lowercase
state name, while the JSON on the wire is capitalised (`"Running"`); compare
against the lowercase form and let the binding handle it. Supports `==` and
`!=` against another `Status` or a plain string (e.g. `job.state ==
"complete"`). Usable as a `dict` key or in a `set`.

---

### `Health`

Return type of `health()`.

| Property | Type | Description |
|---|---|---|
| `status` | `str` | `"healthy"`, `"degraded"`, or `"error"` |
| `detail` | `HealthInfo \| None` | Detailed health data if available |

---

### `Diagnostics`

Return type of `diagnostics()`.

**Properties:**

| Property | Type | Description |
|---|---|---|
| `status` | `str` | `"ok"` or an error description |
| `detail` | `DiagnosticsReport \| None` | Full diagnostics report if available |

**Methods:**

| Method | Signature | Description |
|---|---|---|
| `is_healthy` | `() → bool` | `True` if `status == "ok"` |
| `logs` | `(max: int = 0, level: str \| None = None, search: str \| None = None) → list[LogEntry]` | Return log entries from the contained report. See [`DiagnosticsReport.logs`](#diagnosticsreport) for full details. Returns `[]` if no report is available. |

See [notes.md](notes.md) for the provisional `HealthInfo` and
`DiagnosticsReport` schemas (these types are still evolving).

---

### `DiagnosticsReport`

Full diagnostics data for a single agent. Returned by `Diagnostics.detail` and
passed directly when iterating cached reports.

**Properties:**

| Property | Type | Description |
|---|---|---|
| `agent_name` | `str` | Name of the agent that generated the report |
| `generated_at` | `datetime` | UTC time the report was generated |
| `failed_jobs` | `list[FailedJobEntry]` | Recent failed jobs (deduplicated) |
| `slowest_jobs` | `list[SlowJobEntry]` | Slowest successful jobs (>10 s) |
| `expired_jobs` | `list[ExpiredJobEntry]` | Recent expired jobs (deduplicated) |
| `running_jobs` | `list[RunningJobEntry]` | Currently running jobs |
| `warnings` | `list[str]` | Auto-generated alert strings |
| `notification_statistics` | `NotificationStatistics` | All-time notification counters |

**Methods:**

| Method | Signature | Description |
|---|---|---|
| `logs` | `(max: int = 0, level: str \| None = None, search: str \| None = None) → list[LogEntry]` | Return captured log entries in chronological order (oldest first). |

`logs` parameters:

- **`max`** — maximum number of entries to return; `0` (default) returns all captured entries. Applied *after* any filtering, so `.logs(50, level="ERROR")` returns the 50 most recent errors.
- **`level`** — filter by log level (case-insensitive):
  - `"INFO"` — exact match; only INFO entries.
  - `"INFO+"` — threshold; INFO and above (INFO, WARN, ERROR).
  - `"WARN"` and `"WARNING"` are accepted interchangeably.
  - Available levels, from lowest to highest: `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`.
- **`search`** — case-insensitive substring match against the message text.

All supplied filters are ANDed together.

```python
d = openportal.diagnostics("brics")

d.logs()                               # all captured entries, oldest first
d.logs(100)                            # last 100 entries
d.logs(level="ERROR")                  # all errors
d.logs(level="WARN+")                  # warnings and errors
d.logs(50, level="WARN+")             # last 50 warnings/errors
d.logs(level="WARN+", search="timeout")  # warning/error messages containing "timeout"
```

---

### `NotificationStatistics`

All-time notification counters for a single agent. Returned by
`DiagnosticsReport.notification_statistics`.

| Property | Type | Description |
|---|---|---|
| `total_received` | `int` | Notifications received by this agent from the network |
| `total_sent` | `int` | Notifications successfully delivered (to next hop or web portal) |
| `total_failed` | `int` | Notifications dropped after all delivery attempts failed |

```python
report = openportal.diagnostics("brics.aip1.clusters.shared").detail()
ns = report.notification_statistics
print(f"received={ns.total_received} sent={ns.total_sent} failed={ns.total_failed}")
# or just
print(ns)  # NotificationStatistics(received=12, sent=12, failed=0)
```

A non-zero `total_failed` also appears as a string in `report.warnings`.

---

### `LogEntry`

A single log message captured from the agent's tracing framework.

| Property | Type | Description |
|---|---|---|
| `timestamp` | `datetime` | UTC time the message was logged |
| `level` | `str` | Log level: `"TRACE"`, `"DEBUG"`, `"INFO"`, `"WARN"`, or `"ERROR"` |
| `target` | `str` | Rust module path that produced the message (e.g. `"templemeads::agent"`) |
| `message` | `str` | The log message text |

---

### `Destination`

A dot-separated routing path identifying an agent, e.g.
`myportal.clusters.shared`. Used for `offerings` and for constructing
job commands.

`Destination("myportal.clusters.shared")` constructs from a string. `str(d)`
returns the dot-path. Supports `==` / `!=` against another `Destination` or a
plain string. Usable as a `dict` key or in a `set`.

| Property | Type | Description |
|---|---|---|
| `agents` | `list[str]` | The path split into agent names |

`agents` is how a portal reads the two ends that matter on a bridge-board job:
for a `forwarded_for` of `allocator.site.cluster1`, `agents[0]` is the awarding
portal that asked (`allocator`) and `agents[-1]` is the offering it came in
through (`cluster1`). See
[site-portal-api.md §1.2](site-portal-api.md).

---

### `UserIdentifier`

A triple `username.project.portal` that uniquely identifies a user within
the OpenPortal network.

`UserIdentifier("alice.myproject.myportal")` constructs from a string.
`str(uid)` returns the dot-triple. Supports `==` / `!=` against another
`UserIdentifier` or a plain string. Usable as a `dict` key or in a `set`.

---

### `ProjectIdentifier`

A pair `project.portal` that uniquely identifies a project.

`ProjectIdentifier("myproject.myportal")` constructs from a string.
`str(pid)` returns the dot-pair. Supports `==` / `!=` against another
`ProjectIdentifier` or a plain string. Usable as a `dict` key or in a `set`.

---

### `PortalIdentifier`

The name of a portal, e.g. `myportal`.

`PortalIdentifier("myportal")` constructs from a string. `str(pid)` returns
the portal name. Supports `==` / `!=` against another `PortalIdentifier` or a
plain string. Usable as a `dict` key or in a `set`.

---

### `ProjectMapping`

The pairing of two portals' names for the same thing:
`<their project id>:<our project id>`, e.g. `"myaward1.allocator:myproject1.site"`.

**This is a string, not an object**, and it is what `create_award`,
`update_award`, `remove_award` and `get_project_mapping` must return — see
[site-portal-api.md §4.1.1](site-portal-api.md), which explains why it
matters.

| Property | Type | Description |
|---|---|---|
| `project` | `ProjectIdentifier` | The identifier the awarding portal used |
| `local_group` | `str` | The receiving portal's own identifier for it |

`ProjectMapping("myaward1.allocator:myproject1.site")` constructs from a string, and
raises `OSError` if either half is invalid. `str(m)` returns the pair. Supports
`==` / `!=` against another `ProjectMapping`.

The second half is named `local_group` for historical reasons — elsewhere in the
network it does name a Unix group — but at the portal layer it is the receiving
portal's own `ProjectIdentifier`, a full `<project>.<their-portal>`. It is the
join key: usage recorded against it is reported against the *first* half, and
`ProjectUsageReport.remap_project` is the translation. It is restricted to
`A-Za-z0-9._-` (no leading `-`, no leading or trailing `.`, no `..`).

`remove_award` answers with the literal `None` in this slot, since there is no
project of the receiving portal's left to name.

---

### `UserMapping`

The pairing of a user identifier with their local account and group:
`<user_id>:<local_user>:<local_group>`, e.g.
`"alice.myproject.myportal:alice@example.ac.uk:myproject.myportal"`. Also a
string rather than an object, and the return type of `get_users`.

| Property | Type | Description |
|---|---|---|
| `user` | `UserIdentifier` | The portal-level user identifier |
| `local_user` | `str` | The local account name, **or an email address** |
| `local_group` | `str` | The local group |

`UserMapping("alice.p.portal:alice@example.ac.uk:p.portal")` constructs from a
string. `str(m)` returns the triple. Supports `==` / `!=`.

At the portal layer the member's **email address is the `local_user`** — a
portal has no Unix accounts to name. This is supported explicitly, and the
address grammar is deliberately narrower than RFC 5321: local part from
`A-Za-z0-9._+-`, then a hostname of at least two labels, because the same field
carries Unix account names elsewhere in the network. A mapping whose address
does not fit is rejected outright, so substitute a sanitised form rather than
letting a whole `get_users` response fail.

---

### `Allocation`

A quantity of compute granted to an award — a size paired with a unit, e.g.
`"1000 GBH"`. Carried by `AwardDetails.allocation`.

**Constructors:**

| Method | Signature | Description |
|---|---|---|
| `Allocation` | `() → Allocation` | An empty allocation |
| `from_size_and_units` | `(size: float, units: str) → Allocation` | *(static)* From a number and a unit name |
| `parse` / `from_string` | `(allocation: str) → Allocation` | *(static)* From a string such as `"1000 GBH"` |
| `from_node_hours` … `from_billing_hours` | `(usage: Usage, node: Node) → Allocation` | *(static)* Convert a `Usage` against a `Node`'s shape. One per unit family: `node`, `cpu`, `core`, `gpu`, `gb`, `billing` |

**Properties (read-only):**

| Property | Type | Description |
|---|---|---|
| `size` | `float \| None` | The numeric size |
| `units` | `str \| None` | The canonical unit name |
| `is_empty` | `bool` | `True` if no size or units are set |
| `is_node_hours`, `is_cpu_hours`, `is_core_hours`, `is_gpu_hours`, `is_gb_hours`, `is_billing_hours` | `bool` | Which unit family this allocation is in |

**Methods:**

| Method | Signature | Description |
|---|---|---|
| `to_node_hours` | `(node: Node) → Usage` | Convert to a `Usage` against a node's shape |
| `canonicalize` | `(units: str) → str` | *(static)* The canonical spelling of a unit name |

The recognised units, and what each `is_*` predicate matches:

| Canonical | Also accepted | Predicate |
|---|---|---|
| `NHR` | `node hours`, `node hour`, `nhr` | `is_node_hours` |
| `CPUHR` | `cpu hours`, `cpu hour`, `cpuhr` | `is_cpu_hours` |
| `COREHR` | `core hours`, `core hour`, `corehr` | `is_core_hours` |
| `GPUHR` | `gpu hours`, `gpu hour`, `gpuhr` | `is_gpu_hours` |
| `GBHR` | `gb hours`, `gb hour`, `gbhr` | `is_gb_hours` |
| `BHR` | `billing hours`, `billing hour`, `bhr` | `is_billing_hours` |

Matching is case-insensitive, and **an unrecognised unit is not an error** — it
is stored lower-cased and every predicate returns `False` for it. So
`from_size_and_units(100, "GBH")` (note the missing `R`) yields a valid
`Allocation` in units `"gbh"` that nothing recognises. Check
`Allocation.canonicalize(units)` against the table if you accept unit names from
elsewhere.

`str(a)` returns the size and units. Supports `==` / `!=`.

---

### `DateRange`

An inclusive span of dates, and the second argument to every report
instruction. `str(r)` returns the wire form, which is either an explicit range
or one of the keywords below.

**Constructors:**

| Method | Signature | Description |
|---|---|---|
| `DateRange` | `(start_date: date, end_date: date) → DateRange` | An explicit range |
| `parse` | `(date_range: str) → DateRange` | *(static)* From the wire form, including the keywords |
| `today`, `yesterday`, `tomorrow` | `() → DateRange` | *(static)* Single-day ranges |
| `this_week`, `last_week`, `next_week` | `() → DateRange` | *(static)* |
| `this_month`, `last_month`, `next_month` | `() → DateRange` | *(static)* |
| `this_year`, `last_year`, `next_year` | `() → DateRange` | *(static)* |
| `week`, `month`, `year` | `(date: date) → DateRange` | *(static)* The week/month/year containing `date` |

**Properties (read-only):**

| Property | Type | Description |
|---|---|---|
| `start_date`, `end_date` | `date` | The bounds, inclusive |
| `start_time`, `end_time` | `datetime` | The bounds as UTC instants |
| `days` | `list[date]` | Every date in the range |
| `months`, `weeks`, `years` | `list[DateRange]` | The range split into whole months, weeks or years — useful for querying a per-month accounting store, as the reference portal does |

When the argument is omitted from a report instruction the grammar fills in
`this_week`, so a handler always receives one.

---

### `Link`

A reference to something outside OpenPortal — an award record, a funding call, a
project page. Carried by `AwardDetails.award`, `.call`, `.project_link` and
`.renewal`.

| Property | Type | Description |
|---|---|---|
| `id` | `str \| None` | An identifier in the far system |
| `url` | `str \| None` | A URL |

`Link()` constructs an empty one; both fields have setters and a
`clear_id()` / `clear_url()`. `is_empty()` returns `True` when neither is set.
Supports `==` / `!=`.

---

### `Note`

A timestamped, attributed comment on an award. `AwardDetails.notes` is a list of
these, and it is the one field `merge` **accumulates** rather than replaces —
it is an audit trail, so notes from both sides survive a merge, de-duplicated
and sorted by timestamp.

| Property | Type | Description |
|---|---|---|
| `timestamp` | `datetime` | UTC, set at construction |
| `author` | `str` | Who wrote it |
| `text` | `str` | What it says |

`Note("alice@example.com", "approved by the allocation panel")` constructs one.
Supports `==` / `!=`.

---

### `AwardDetails`

Details about an award (and the project it creates), including the project
identifier, template, member users, award identifiers, and resource allocation.
See [json-types.md](json-types.md) for the full JSON schema.

`openportal.ProjectDetails` is an alias for `AwardDetails` for backward
compatibility; both refer to the same class.

**Constructors:**

| Method | Signature | Description |
|---|---|---|
| `AwardDetails` | `(details: str = "{}") → AwardDetails` | Parse from JSON. With no argument, an empty award to fill in with the setters below. Raises `OSError` on malformed JSON. |
| `from_json` | `(json: str) → AwardDetails` | *(static)* Same as passing JSON to the constructor. |

**Methods:**

| Method | Signature | Description |
|---|---|---|
| `to_json` | `() → str` | Serialise to a JSON string. |
| `merge` | `(other: AwardDetails) → AwardDetails` | Return a copy with `other`'s set fields applied over this one's. Fields absent from `other` are left alone; `members` and `allowed_domains` are **replaced** wholesale when present, while `notes` and `breakdown` accumulate. Raises `OSError` if the two name different templates. |

**Member management methods:**

| Method | Signature | Description |
|---|---|---|
| `add_member` | `(email: str, role: str) → None` | Add or update one member. Validates that `email` is a well-formed address and is permitted by `allowed_domains`. Raises `OSError` if either check fails. |
| `add_members` | `(members: dict[str, str]) → None` | Atomically add or update multiple members (email → role). All entries are validated before any change is applied; raises `OSError` and leaves members unchanged if any entry is invalid. |
| `set_members` | `(members: dict[str, str]) → None` | Atomically replace all members. All entries are validated before any change is applied; raises `OSError` and leaves members unchanged if any entry is invalid. |
| `remove_member` | `(email: str) → None` | Remove one member by email. No-op if not present. |
| `clear_members` | `() → None` | Remove all members. |

**Domain allow-list methods:**

| Method | Signature | Description |
|---|---|---|
| `is_domain_allowed` | `(domain: str) → bool` | Return `True` if the bare domain (e.g. `"example.com"`) is permitted by the allow-list. Email patterns in the list are ignored. |
| `is_email_allowed` | `(email: str) → bool` | Return `True` if the full email address is permitted. Checks exact email patterns and domain patterns against the address's domain. |
| `add_allowed_domain` | `(domain: str \| DomainPattern) → None` | Append one entry to the allow-list. Accepts a domain pattern (`"example.com"`, `"*.example.com"`) or an exact email address (`"user@example.com"`). |
| `set_allowed_domains` | `(domains: list[str \| DomainPattern] \| None) → None` | Replace the allow-list with exactly what is given. Pass `[]` to permit nobody, or `None` to remove all restrictions. |
| `clear_allowed_domains` | `() → None` | Remove the allow-list (all emails become permitted). |

**Allow-list behaviour:**

- `allowed_domains` is `None` (unset) — all email addresses are permitted.
- `allowed_domains` is `[]` (empty list) — no email addresses are permitted.
- Otherwise — an email is permitted if it matches at least one entry: either an
  exact email pattern matches the full address (case-insensitive), or a domain
  pattern matches the domain part of the address.

`add_member`, `add_members`, and `set_members` enforce the allow-list at call
time. Existing members are never retroactively removed if the allow-list changes
after they were added.

All three states are reachable and distinct, and they survive a JSON round trip:
`details.allowed_domains = []` permits nobody, `= None` (or
`clear_allowed_domains()`) removes the restriction.

`update_award` **replaces** the allow-list rather than merging into it: the
awarding portal owns the set, so whatever it sends is the whole set afterwards.
An update naming fewer domains removes the rest, and an update sending `[]`
permits nobody. An update that omits the field entirely changes nothing, as with
every other field.

**Related types used in `AwardDetails`:**

- **`ProjectTemplate`** — `ProjectTemplate("standard")` constructs from a
  string. `str(pt)` returns the template name. Supports `==` / `!=` against
  another `ProjectTemplate` or a plain string. Usable as a `dict` key or in a `set`.
- **`MembershipControl`** — controls whether the receiving portal may change
  project membership or roles. Values: `MembershipControl.Open` (default),
  `MembershipControl.MembersOnly`, `MembershipControl.RolesOnly`,
  `MembershipControl.Locked`. `MembershipControl.from_string("locked")`
  constructs from a string. `str(mc)` returns the snake_case name. Supports
  `==` / `!=` against another `MembershipControl` or a plain string.
  Usable as a `dict` key or in a `set`.
- **`DomainPattern`** — `DomainPattern("*.example.com")` or
  `DomainPattern("user@example.com")` constructs from a string. `str(dp)`
  returns the pattern. Supports `==` / `!=` against another `DomainPattern`
  or a plain string. Usable as a `dict` key or in a `set`.

---

### `Usage`

A compute-time quantity (internally stored as an integer number of seconds).

**Properties:**

| Property | Type | Description |
|---|---|---|
| `seconds` | `int` | Raw value in seconds |

**Methods:**

| Method | Signature | Description |
|---|---|---|
| `in_hours` | `() → str` | Return a human-readable string with all values expressed in hours (e.g. `"2.000 hours"`). Useful for consistent unit display when comparing across days. |

`str(usage)` auto-scales to the most appropriate unit (seconds, minutes, or
hours) with up to 3 decimal places, e.g. `"2.000 hours"`, `"40.433 minutes"`,
`"1 second"`.

---

### `DailyProjectUsageReport`

Compute usage for a single project on a single calendar day, broken down by
local username. Arithmetic operators (`+`, `+=`) are supported for merging
reports.

**Properties:**

| Property | Type | Description |
|---|---|---|
| `num_jobs` | `int` | Total number of jobs that started on this day (scalar total across all users) |
| `total_wait_seconds` | `int` | Total queue wait time in seconds for all jobs that started on this day |
| `average_wait_seconds` | `float` | Mean queue wait time in seconds per job (`0.0` if `num_jobs == 0`) |
| `is_complete` | `bool` | `True` if all usage data for the day has been collected |

**Methods:**

| Method | Signature | Description |
|---|---|---|
| `num_jobs_for_user` | `(user: str) → int` | Number of jobs started by the named local user. Returns `0` for unknown users or legacy data without per-user counts. |
| `wait_seconds_for_user` | `(user: str) → int` | Total queue wait seconds for the named local user. Returns `0` for unknown users or legacy data. |
| `average_wait_seconds_for_user` | `(user: str) → float` | Mean queue wait seconds per job for the named local user. Returns `0.0` if the user has no jobs or data is unavailable. |
| `in_hours` | `() → str` | Return a multi-line human-readable string with all usage values expressed in hours, including per-user job counts and average wait times. |

`str(report)` auto-scales usage units per user and includes per-user job
counts and average wait times.

---

### `ProjectUsageReport`

Compute usage for a single project over a date range, indexed by calendar
date. Arithmetic operators (`+`, `+=`) are supported.

**Properties:**

| Property | Type | Description |
|---|---|---|
| `total_wait_seconds` | `int` | Total queue wait time in seconds across all days in this report |
| `average_wait_seconds` | `float` | Mean queue wait time in seconds per job across the whole report (`0.0` if no jobs) |
| `users` | `list[UserIdentifier]` | Sorted list of portal users with mappings in this report |
| `user_mapping` | `dict[UserIdentifier, str]` | Map of portal user identifier → local username |

**Methods:**

| Method | Signature | Description |
|---|---|---|
| `daily_reports` | `(with_usage_only: bool = True) → list[DailyProjectUsageReport]` | Return the daily reports sorted by date. If `with_usage_only=True` (default), only days with non-zero usage are returned; pass `False` to include all days. |
| `in_hours` | `() → str` | Return a multi-line human-readable string with all usage values expressed in hours, including per-user breakdowns, job counts, and average wait times. |
| `filter` | `(range: DateRange) → ProjectUsageReport` | Return a copy of this report containing only days that fall within `range` (inclusive on both ends). |
| `remap_project` | `(new_project: ProjectIdentifier) → None` | Replace the project identifier and rebuild all `UserIdentifier` keys so that `username.old_project.old_portal` becomes `username.new_project.new_portal`. This is how a portal answers a request about *their* project with figures recorded against *its own* — see [site-portal-api.md §4.1.1](site-portal-api.md). Local usernames are unchanged. |
| `remap_portal` | `(new_portal: PortalIdentifier) → None` | Swap the portal while keeping each project name unchanged, e.g. `project.portal` → `project.new_portal`. |
| `remap_users` | `(new_usermapping: dict[UserIdentifier, str]) → None` | Update local username strings for the specified users. Raises `OSError` if the remapping would merge two distinct users into the same local username. |

`str(report)` auto-scales usage units per user per day.

---

### `UsageReport`

Portal-level aggregate report containing `ProjectUsageReport` objects for all
active projects. Arithmetic operators (`+`, `+=`) are supported.

**Properties (read-only):**

| Property | Type | Description |
|---|---|---|
| `site` | `PortalIdentifier` | The portal this report covers |
| `projects` | `list[ProjectIdentifier]` | Sorted list of projects with reports |
| `user_mapping` | `dict[UserIdentifier, str]` | Combined portal user → local username map across all contained project reports |

**Methods:**

| Method | Signature | Description |
|---|---|---|
| `get_report` | `(project: ProjectIdentifier) → ProjectUsageReport` | Return the usage report for `project`, or an empty report if not present |
| `get_component` | `(component: str) → UsageReport` | Return a new `UsageReport` containing only the named component's usage |
| `filter` | `(range: DateRange) → UsageReport` | Return a copy of this report with every contained `ProjectUsageReport` filtered to only days that fall within `range` (inclusive on both ends). |
| `combine` | `(reports: list[UsageReport]) → UsageReport` | *(static)* Merge a list of portal-level reports |
| `remap_portal` | `(new_portal: PortalIdentifier) → None` | Update `self.portal` and remap every contained project to the new portal, e.g. `project.portal` → `project.new_portal`. |
| `remap_project` | `(old_project: ProjectIdentifier, new_project: ProjectIdentifier) → None` | Remap a single contained project from `old_project` to `new_project`. Does nothing if `old_project` is not present. |
| `remap_users` | `(new_usermapping: dict[UserIdentifier, str]) → None` | Update local username strings across all contained project reports. Raises `OSError` on clash within any project. |
| `to_json` | `() → str` | Serialise to a JSON string |
| `from_json` | `(json: str) → UsageReport` | *(static)* Deserialise from a JSON string |

See [json-types.md](json-types.md) for full schemas.

---

### `ProjectStorageReport`

Returned by `job.result` after a `get_storage_report` or `get_local_storage_report`
call. Reflects the current (point-in-time) storage quota state for a single project.

**Properties (read-only):**

| Property | Type | Description |
|---|---|---|
| `project` | `ProjectIdentifier` | The project this report covers |
| `generated_at` | `datetime` | UTC timestamp when the report was generated |
| `project_quotas` | `dict[Volume, Quota]` | Project-level quotas keyed by volume |
| `user_quotas` | `dict[UserIdentifier, dict[Volume, Quota]]` | Per-user quotas keyed by user identifier then volume |
| `users` | `list[UserIdentifier]` | Sorted list of portal users with mappings in this report |
| `user_mapping` | `dict[UserIdentifier, str]` | Map of portal user identifier → local username |

**Methods:**

| Method | Signature | Description |
|---|---|---|
| `is_empty` | `() → bool` | `True` if the top-level snapshot has no quota data (historical entries are not considered) |
| `daily_reports` | `(with_usage_only: bool = True) → list[ProjectStorageReport]` | Return all snapshots sorted by date (oldest first), including both historical entries and the current top-level snapshot. When `with_usage_only=True` (default), only snapshots with quota data are returned. When `False`, every calendar date between the earliest and latest snapshot is included (empty reports for missing days), mirroring `ProjectUsageReport.daily_reports()`. |
| `get_report` | `(date: datetime.date) → ProjectStorageReport` | Return the snapshot for a specific date. Returns the top-level data if `date` matches the current snapshot's date, or an empty report if not found. |
| `filter` | `(range: DateRange) → ProjectStorageReport` | Return a copy of this report containing only historical snapshots whose date falls within `range` (inclusive). The top-level (current) snapshot fields are preserved unchanged. |
| `combine` | `(reports: list[ProjectStorageReport]) → ProjectStorageReport` | *(static)* Merge a list of reports for the same project using the merge semantics: newest snapshot wins at the top level; older snapshots are retained in history (one per date, newest wins). |
| `remap_project` | `(new_project: ProjectIdentifier) → None` | Replace the project identifier and rebuild all `UserIdentifier` keys (in `users`, `user_quotas`, and historical snapshots) so that `username.old_project.old_portal` becomes `username.new_project.new_portal`. |
| `remap_portal` | `(new_portal: PortalIdentifier) → None` | Swap the portal while keeping the project name unchanged. |
| `remap_users` | `(new_usermapping: dict[UserIdentifier, str]) → None` | Update local username strings for the specified users. Raises `OSError` if the remapping would merge two distinct users into the same local username. |
| `to_storage_report` | `() → StorageReport` | Wrap this single-project report in a portal-level `StorageReport`. The mirror of `ProjectUsageReport.to_usage_report`; use it to lift per-project reports before combining them for `get_storage_reports`. |
| `to_json` | `() → str` | Serialise to a JSON string |
| `from_json` | `(json: str) → ProjectStorageReport` | *(static)* Deserialise from a JSON string |

`+` and `+=` operators merge two `ProjectStorageReport` objects using the same
semantics as `combine`.

`str(report)` returns a human-readable multi-line summary including a list of
historical snapshot dates if any are present.

**Example:**

```python
import datetime

job = openportal.run("portal.provider.clusters.mycluster get_storage_report myproject.myportal",
                     max_ms=30_000)
if job.is_finished and not job.is_error:
    report = job.result   # ProjectStorageReport
    for volume, quota in report.project_quotas.items():
        print(f"  {volume}: {quota}")
    for user, local in report.user_mapping.items():
        vol_quotas = report.user_quotas.get(user, {})
        for volume, quota in vol_quotas.items():
            print(f"  {user} ({local}) — {volume}: {quota}")

# Accumulate reports fetched on different days
combined = report_day1 + report_day2 + report_day3
for snap in combined.daily_reports():   # oldest first, only days with data
    print(f"  {snap.generated_at}: {snap.project_quotas}")

# Zip with a usage report (both fill every date in the range)
for usage, storage in zip(usage_report.daily_reports(with_usage_only=False),
                          storage_report.daily_reports(with_usage_only=False)):
    print(f"  usage={usage.total_usage}  storage={storage.project_quotas}")

# Retrieve a specific day
snap = combined.get_report(datetime.date(2024, 3, 10))

# Translate a report from one portal to another
old_project = ProjectIdentifier.parse("myproject.myportal")
new_project = ProjectIdentifier.parse("myproject.newportal")
report.remap_project(new_project)

# Remap local usernames (e.g. unix names → email addresses)
uid = UserIdentifier.parse("alice.myproject.newportal")
report.remap_users({uid: "alice@example.com"})
```

---

### `StorageReport`

Returned by `job.result` after a `get_storage_reports` call. Portal-level
aggregate of `ProjectStorageReport` objects for all active projects.

**Properties (read-only):**

| Property | Type | Description |
|---|---|---|
| `site` | `PortalIdentifier` | The portal this report covers |
| `projects` | `list[ProjectIdentifier]` | Sorted list of projects with reports |
| `user_mapping` | `dict[UserIdentifier, str]` | Combined portal user → local username map across all contained project reports |

**Methods:**

| Method | Signature | Description |
|---|---|---|
| `get_report` | `(project: ProjectIdentifier) → ProjectStorageReport` | Return the storage report for `project`, or an empty report if not present |
| `is_empty` | `() → bool` | `True` if there are no project reports |
| `filter` | `(range: DateRange) → StorageReport` | Return a copy of this report with every contained `ProjectStorageReport` filtered to only historical snapshots that fall within `range` (inclusive). Top-level snapshot fields of each project report are preserved unchanged. |
| `combine` | `(reports: list[StorageReport]) → StorageReport` | *(static)* Merge a list of portal-level reports, merging per-project history |
| `remap_portal` | `(new_portal: PortalIdentifier) → None` | Update `self.portal` and remap every contained project to the new portal. |
| `remap_project` | `(old_project: ProjectIdentifier, new_project: ProjectIdentifier) → None` | Remap a single contained project from `old_project` to `new_project`. Does nothing if `old_project` is not present. |
| `remap_users` | `(new_usermapping: dict[UserIdentifier, str]) → None` | Update local username strings across all contained project reports. Raises `OSError` on clash within any project. |
| `to_json` | `() → str` | Serialise to a JSON string |
| `from_json` | `(json: str) → StorageReport` | *(static)* Deserialise from a JSON string |

`+` and `+=` operators merge two `StorageReport` objects, combining the
per-project reports using `ProjectStorageReport` merge semantics.

`str(report)` returns a human-readable multi-line summary.

---

### `StorageSize` / `StorageUsage` / `QuotaLimit` / `Quota` / `Volume`

Storage and quota types returned by filesystem-related instructions.
See [json-types.md](json-types.md) for full schemas.

`QuotaLimit` supports `==` / `!=` against another `QuotaLimit` or a plain
string (e.g. `limit == "unlimited"`, `limit == "100GB"`). `Volume` similarly
supports string comparison (e.g. `vol == "home"`) and is usable as a `dict`
key or in a `set`.

---

### `Uuid`

A UUID value, usable wherever a job ID is required. `Uuid("…")` and
`Uuid.from_string("…")` both construct from a string. `str(u)` returns the
canonical UUID string. Supports `==` / `!=` against another `Uuid` or a plain
string (e.g. `job.id == "abc123…"`). Usable as a `dict` key or in a `set`.

---

## Error handling

Every exception this module raises derives from `OpenPortalError`, which derives
from `OSError`. Existing code that catches `OSError` therefore keeps working
unchanged, and code that wants to know *which* failure it was can catch a
specific class instead.

| Class | Base | Meaning |
|---|---|---|
| `OpenPortalError` | `OSError` | Base of the hierarchy. Catch this to catch everything. |
| `OpenPortalOtherError` | `OpenPortalError` | A failure with no more specific class — including anything the module could not classify. |
| `OpenPortalUnsupportedCommandError` | `OpenPortalError` | The portal that received the instruction does not implement it. |
| `ManagedProjectPermissionError` | `OpenPortalError` | Base for the two award decisions below. |
| `ManagedProjectPendingError` | `ManagedProjectPermissionError` | The award is accepted but not in place yet, typically awaiting human approval. **Not a fault — ask again later.** |
| `ManagedProjectRejectedError` | `ManagedProjectPermissionError` | The award was refused. Re-sending it unchanged will fail again. |

A job carries its failure as one string, so the class is encoded into it as
`"<ClassName>: <message>"`, and the portal agent wraps that as
`RuntimeError{…}` in transit. The module does both halves for you:

```python
# Answering a job — the class travels with the message
job = job.errored(openportal.ManagedProjectPendingError("awaiting approval"))
openportal.send_result(job)

# Reading a job you submitted — the same class comes back
job = openportal.run("allocator.site.cluster1 create_award myaward1.allocator {…}", max_ms=30_000)

if job.is_error:
    match job.error:
        case openportal.ManagedProjectPendingError():
            pass                      # expected; retry on the next cycle
        case openportal.ManagedProjectRejectedError() as e:
            give_up(str(e))           # terminal; do not retry
        case e:
            log(str(e))
```

`job.result` raises the typed error too, so `try: … except
ManagedProjectPendingError:` around a result access works directly. There is
also `job.raise_for_error()` when you want to re-raise without reading a result,
and `openportal.error_from_message(text)` to build the exception from a raw
message you already hold.

The class is chosen from the structured `kind` the failing agent attached, and
only falls back to reading the message when the job came from a peer that
predates it — so the class you catch is the class that was raised, not a guess.
`job.error_kind` exposes that kind directly, which is the way to handle a kind
this module has no class for (it arrives as `OpenPortalOtherError` with its text
intact). The kinds are listed in [json-types.md](json-types.md).

Which error a site portal should return, and what an awarding portal does
with each, is specified in
[site-portal-api.md §3.3](site-portal-api.md).

---

## Thread safety

The module is safe to call from multiple threads. Each call makes an
independent HTTP request to the bridge. However, `job.wait()` and
`job.update()` modify the `Job` object in-place, so a single `Job` instance
should not be shared between threads without external locking.
