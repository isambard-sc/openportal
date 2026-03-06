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

### Bridge board (portal callbacks)

These functions are used when OpenPortal needs the portal to take action
(the OpenPortal → portal direction). See [bridge-api.md](bridge-api.md)
for the full two-direction communication model.

| Function | Signature | Description |
|---|---|---|
| `fetch_jobs` | `() → list[Job]` | Fetch all jobs that OpenPortal has queued for the portal to handle. |
| `fetch_job` | `(job_id: str \| Uuid) → Job` | Fetch a single queued job by ID. |
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
| `diagnostics` | `(destination: str) → Diagnostics` | Fetch a diagnostics report from the agent at `destination` (dot-path, e.g. `"brics.clusters"`). Pass `""` to query the bridge itself. |
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
| `instruction` | `Instruction` | The parsed instruction (e.g. `AddUser`) |
| `state` | `Status` | Current job state |
| `version` | `int` | Monotonically increasing version counter |
| `created` | `datetime` | UTC creation time |
| `changed` | `datetime` | UTC time of last state change |
| `is_finished` | `bool` | `True` if the job is in a terminal state (complete, error, expired, or duplicate) |
| `is_error` | `bool` | `True` if the job failed with an error |
| `is_expired` | `bool` | `True` if the job expired before completion |
| `is_duplicate` | `bool` | `True` if the job was detected as a duplicate of another pending job |
| `result` | `Any` | The deserialized job result once finished. Raises `OSError` if the job is not yet finished, or if the job is in an error state (use `error_message` instead). Returns `None` if the job completed with no result value. |
| `error_message` | `str` | Error description if `is_error`, otherwise `""` |
| `progress_message` | `str` | In-progress status message if set, otherwise `""` |

**Methods:**

| Method | Signature | Description |
|---|---|---|
| `update` | `() → None` | Refresh this job in-place by fetching its latest status from the bridge. No-op if already finished. |
| `wait` | `(max_ms: int = 1000) → bool` | Block until the job is finished or `max_ms` milliseconds elapse. Pass a negative value to wait indefinitely. Returns `True` if the job is now finished. |
| `completed` | `(result) → Job` | Return a new copy of this job marked as complete with the given result. `result` may be a `str`, `bool`, `UserIdentifier`, `ProjectIdentifier`, `ProjectDetails`, `ProjectUsageReport`, `UsageReport`, `Quota`, `Volume`, `StorageSize`, `StorageUsage`, `QuotaLimit`, `ProjectTemplate`, `DateRange`, or a `list` or `dict` of those types. Used when handling bridge-board jobs. |
| `errored` | `(error: str) → Job` | Return a new copy of this job marked as failed with the given error message. Used when handling bridge-board jobs. |
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

### `Status`

Represents the state of a job. String representation matches the job state
names used throughout the protocol.

**Static constructors:** `Status.pending()`, `Status.running()`,
`Status.complete()`, `Status.error()`, `Status.expired()`, `Status.duplicate()`

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

| Property | Type | Description |
|---|---|---|
| `status` | `str` | `"ok"` or an error description |
| `detail` | `DiagnosticsReport \| None` | Full diagnostics report if available |

See [notes.md](notes.md) for the provisional `HealthInfo` and
`DiagnosticsReport` schemas (these types are still evolving).

---

### `Destination`

A dot-separated routing path identifying an agent, e.g.
`myportal.brics.clusters.aip2`. Used for `offerings` and for constructing
job commands.

---

### `UserIdentifier`

A triple `username.project.portal` that uniquely identifies a user within
the OpenPortal network.

---

### `ProjectIdentifier`

A pair `project.portal` that uniquely identifies a project.

---

### `PortalIdentifier`

The name of a portal, e.g. `myportal`.

---

### `ProjectDetails`

Details about a project, including its identifier, template, member users,
and associated mappings. See [json-types.md](json-types.md) for the full
JSON schema.

---

### `UsageReport` / `ProjectUsageReport` / `DailyProjectUsageReport`

Usage accounting types. `UsageReport` is keyed by `ProjectIdentifier`;
`ProjectUsageReport` is keyed by `UserIdentifier`; `DailyProjectUsageReport`
breaks usage down by day. All support `+`, `-`, `*`, `/` arithmetic operators.
See [json-types.md](json-types.md) for full schemas.

---

### `StorageSize` / `StorageUsage` / `QuotaLimit` / `Quota` / `Volume`

Storage and quota types returned by filesystem-related instructions.
See [json-types.md](json-types.md) for full schemas.

---

### `Uuid`

A UUID value, usable wherever a job ID is required. Supports `str(uuid)` and
equality comparison.

---

## Error handling

All functions raise `OSError` on failure. The error message contains the
underlying Rust error chain. There is no separate exception hierarchy — check
`job.is_error` and `job.error_message` for job-level failures, and catch
`OSError` for connectivity or protocol failures.

---

## Thread safety

The module is safe to call from multiple threads. Each call makes an
independent HTTP request to the bridge. However, `job.wait()` and
`job.update()` modify the `Job` object in-place, so a single `Job` instance
should not be shared between threads without external locking.
