<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Bridge and Python Example

This example shows how to connect an OpenPortal agent network to portal
software written in Python using the `op-bridge` agent and the `openportal`
Python library.

It builds on the [command line example](../cmdline/README.md), which showed
how to set up a `portal` and `cluster` agent with standardised CLI arguments
and configuration files. Here we add a `bridge` agent alongside the `portal`,
and write a Python script that submits jobs and handles callbacks.

## Overview

The `op-bridge` agent runs as a sidecar next to the `op-portal` agent. It
exposes a local HTTP API that the `openportal` Python library calls. The
bridge translates these calls into OpenPortal jobs and relays the results back.

```
Python script
    ↕  openportal Python library (HTTP localhost)
op-bridge agent
    ↕  OpenPortal websocket protocol
op-portal agent
    ↕  OpenPortal websocket protocol
op-provider / op-cluster / ...
```

There are two directions of communication:

1. **Portal → OpenPortal**: Python calls `openportal.run(command)` which
   submits a job to the OpenPortal network via the bridge.

2. **OpenPortal → portal**: OpenPortal places jobs on the bridge board when
   it needs the portal to perform an action (e.g. look up a project). Python
   calls `openportal.fetch_jobs()` to retrieve these, processes them, and
   calls `openportal.send_result(job)` to return the result.

## Prerequisites

You need Rust (with `cargo`) and Python 3.8+ with
[maturin](https://www.maturin.rs) installed.

```bash
pip install maturin
```

## Step 1: Compile the binaries

From the workspace root, compile everything including the Python bindings:

```bash
make
make python
```

This produces `op-portal`, `op-bridge`, and an `op-cluster` (or similar)
executable in `target/debug/`, and installs the `openportal` Python module
into your current Python environment.

## Step 2: Initialise the portal agent

```bash
./target/debug/op-portal init
```

This writes a default configuration to `~/.config/openportal/portal.toml`
(or `portal.toml` in the current directory, depending on the defaults for
your build).

## Step 3: Initialise the bridge agent

```bash
./target/debug/op-bridge init
```

This writes a default bridge configuration. The bridge runs two servers:

- An OpenPortal websocket server on port 8044 (for connecting to the portal)
- A local HTTP server on port 3000 (for the Python library)

## Step 4: Connect the bridge to the portal

The bridge is a client of the portal. Ask the portal to create an invitation
for the bridge:

```bash
./target/debug/op-portal client --add bridge --ip 127.0.0.1
```

This writes `invite_portal_default.toml`. An invitation is named after the
agent that *issues* it and the zone - `invite_<issuer>_<zone>.toml` - not after
the agent being invited. Import it into the bridge:

```bash
./target/debug/op-bridge server --add invite_portal_default.toml
```

## Step 5: Connect the portal to a provider

For a real deployment you would connect the portal to an `op-provider` agent.
For this example we will re-use the `example-cluster` from the
[command line example](../cmdline/README.md) as a simple downstream agent.

Ask the portal to create an invitation for the downstream agent (here called
`provider`):

```bash
./target/debug/op-portal client --add provider --ip 127.0.0.1
```

Give the invitation to the provider/cluster agent. The portal issued it, so it
has the same name as the bridge's invitation in Step 4 -
`invite_portal_default.toml` - and has overwritten it. That is why each
invitation is imported before the next is created:

```bash
./target/debug/example-cluster server --add invite_portal_default.toml
```

## Step 6: Run all agents

Open three terminal windows and start each agent:

```bash
# Terminal 1
./target/debug/op-portal run
```

```bash
# Terminal 2
./target/debug/op-bridge run
```

```bash
# Terminal 3
./target/debug/example-cluster run
```

You should see the agents connect to each other in the logs.

## Step 7: Install the Python library

If you have not already done so:

```bash
make python
# or: maturin develop -m python/Cargo.toml
```

Verify it is installed:

```python
import openportal
print(openportal.__doc__)
```

## Step 8: Write a Python script

The Python library does not read the bridge agent's own configuration. It reads
a small client file holding just the bridge's HTTP address and its signing key,
which you ask the bridge to write:

```bash
./target/debug/op-bridge bridge --config bridge-client.toml
```

The file looks like this:

```toml
url = "http://localhost:3000/"

[key]
data = "..."    # the HMAC signing key
```

It holds the key in clear, so it is written readable only by you - keep it
that way. Load it before calling any other function:

```python
import openportal

# Enable logging (optional)
openportal.initialize_tracing()

# Load the client file written above - adjust the path to match your setup
openportal.load_config("bridge-client.toml")
assert openportal.is_config_loaded(), "Bridge config not loaded"
```

### Checking the health of the network

```python
health = openportal.health()
print(f"Health status: {health.status}")

if health.detail:
    info = health.detail
    print(f"Agent: {info.name}")
    print(f"Uptime: {info.uptime_seconds}s")
    print(f"Active jobs: {info.active_jobs}")
```

### Submitting a job (portal → OpenPortal)

```python
# Submit a job and wait up to 10 seconds for the result
job = openportal.run(
    "portal.example-cluster add_user alice.myproject.myportal",
    max_ms=10_000
)

if job.is_error:
    print(f"Job failed: {job.error_message}")
elif job.is_finished:
    print(f"Job completed: {job.state}")
else:
    print("Job still running after timeout")
    # You can continue polling manually:
    while not job.is_finished:
        job.wait(max_ms=1000)
    print(f"Final state: {job.state}")
```

### Polling without blocking

If you prefer a non-blocking style (e.g. in a web request handler):

```python
import time

job = openportal.run("portal.example-cluster add_user bob.myproject.myportal")

# Job is submitted but not necessarily finished yet.
# Poll in the background and check later.
for _ in range(30):
    job.update()
    if job.is_finished:
        break
    time.sleep(1)

print(f"Done: {job.is_finished}, error: {job.is_error}")
```

### Handling OpenPortal → portal callbacks

Some instructions ask the portal to look up or supply information (e.g.
`GetProject`, `GetProjects`). OpenPortal queues these on the bridge board.
Your portal software should poll `fetch_jobs()` periodically and respond.

```python
import openportal

def handle_bridge_jobs():
    jobs = openportal.fetch_jobs()
    for job in jobs:
        instruction = str(job.instruction)
        print(f"Handling: {instruction}")

        try:
            result = dispatch(job)
            completed = job.completed(result)
        except Exception as exc:
            completed = job.errored(str(exc))

        openportal.send_result(completed)

def dispatch(job):
    instruction = str(job.instruction)

    if instruction.startswith("GetProject "):
        project_id = instruction.split(" ", 1)[1]
        return look_up_project_details(project_id)

    if instruction == "GetProjects":
        return list_all_projects()

    raise ValueError(f"Unhandled instruction: {instruction}")
```

Call `handle_bridge_jobs()` in a periodic task, a background thread, or
a Django/FastAPI scheduled job.

### Managing offerings

Offerings tell OpenPortal which destinations this portal can handle. They
are the dot-path identifiers of the agents or resources managed by this
portal.

```python
from openportal import Destination

# Declare the destinations this portal manages
my_offerings = [
    Destination("myportal.brics.clusters.aip2"),
    Destination("myportal.brics.clusters.isambard"),
]

# Atomically sync (replaces any previous offerings)
active = openportal.sync_offerings(my_offerings)
print(f"Active offerings: {[str(d) for d in active]}")
```

### Fetching diagnostics

```python
# Diagnostics from the bridge itself
diag = openportal.diagnostics("")
print(f"Status: {diag.status}")
if diag.detail:
    report = diag.detail
    print(f"Agent: {report.agent_name}")
    if report.failed_jobs:
        for entry in report.failed_jobs:
            print(f"  Failed: {entry.instruction} @ {entry.destination} "
                  f"(seen {entry.count} times)")
```

## The two configuration files

There are two files, and they are easy to confuse.

**The bridge agent's own configuration** is what `op-bridge init` writes
(`~/.config/openportal/bridge.toml` by default). `op-bridge` reads it; the
Python library does not, and `load_config` fails on it. After Step 4 it looks
like this:

```toml
agent = "Bridge"

[service]
name = "bridge"
url = "ws://localhost:8044/"
ip = "127.0.0.1"
port = 8044
clients = []

[[service.servers]]
name = "portal"
url = "ws://localhost:8040/"
zone = "default"
type = "portal"
# ... and the keys for that connection

[bridge]
url = "http://localhost:3000/"
ip = "127.0.0.1"
port = 3000
signal_url = "http://localhost/signal"
notification_url = "http://localhost/notification"

[bridge.key]
data = "..."    # HMAC signing key
```

**The Python client file** is what `op-bridge bridge --config <file>` writes
from it (Step 8): only `url` and `[key]`, copied from the `[bridge]` section
above. This is the file `openportal.load_config` reads. Write it again
whenever the bridge's address or key changes.

## Troubleshooting

**`OSError: Could not parse config file`** from `load_config` — it has been
given the bridge agent's own configuration rather than the client file. Write
the client file with `op-bridge bridge --config <file>` and load that.

**`OSError: Failed to connect to bridge`** — the `op-bridge` agent is not
running, or the `url` in the client file does not match the address the bridge
is actually listening on.

**`OSError: HMAC verification failed`** — the key in the client file does not
match the running bridge, usually because the key was regenerated
(`op-bridge bridge --regenerate`) or the bridge re-initialised since the file
was written. Write the client file again with `op-bridge bridge --config <file>`
and reload it.

**Jobs never finish** — check that the downstream agents (`op-portal`,
`op-cluster`, etc.) are all running and connected. Use `openportal.health()`
or `openportal.diagnostics("")` to inspect the network state.

**`fetch_jobs()` always returns an empty list** — verify that the bridge is
connected to the portal (`op-portal client --list` should show `bridge`) and
that `sync_offerings` has been called with at least one destination.

## What next?

For the full protocol details see the
[specifications](../specifications/README.md) directory, in particular:

- [bridge-api.md](../specifications/bridge-api.md) — the HTTP API the Python
  library calls
- [python-api.md](../specifications/python-api.md) — complete Python API
  reference
- [instruction-protocol.md](../specifications/instruction-protocol.md) —
  all available instructions you can pass to `openportal.run()`
- [agent-configuration.md](../specifications/agent-configuration.md) —
  full configuration reference for the bridge and all other agents
