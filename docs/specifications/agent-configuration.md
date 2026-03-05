<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Agent Configuration Reference

This document describes the configuration file format and CLI commands for every
OpenPortal agent. All configuration files are TOML and are typically stored in
`~/.config/openportal/` (or the OS-appropriate equivalent returned by
`dirs::config_local_dir()`).

---

## 1. Common Configuration (all agents)

Every agent's configuration file contains the fields listed below. They are
managed by Paddington's `ServiceConfig` and are shared across all agent types.

### 1.1 Top-Level Fields

```toml
name     = "<agent-name>"
url      = "<wss://...>"
ip       = "<listen-ip>"
port     = <listen-port>

# Optional
heathcheck_port = <port>
proxy_header    = "<header-name>"
agent           = "<AgentType>"

# Optional config file encryption at rest
[encryption]
type = "Environment"
key  = "ENV_VAR_NAME"
# or
# type = "Simple"
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Agent name. Alphanumeric, `-`, `_` only. Used as the agent's identity in the network. |
| `url` | string | Public WebSocket URL peers will connect to, e.g. `wss://hpc.example.com:8042`. |
| `ip` | string | IP address to bind the WebSocket listener to. |
| `port` | integer | Port to bind the WebSocket listener to. |
| `heathcheck_port` | integer (optional) | If set, a minimal HTTP health endpoint is exposed on this port (responds `200 OK` to `GET /`). |
| `proxy_header` | string (optional) | HTTP header to read the real client IP from when behind a reverse proxy (e.g. `X-Forwarded-For`). |
| `agent` | string | Agent type tag stored in the config. Set automatically by `init`. |
| `encryption` | table (optional) | Encryption scheme for secrets stored in the config file. See [security-model.md](security-model.md) §5. |

### 1.2 Peer Lists

Each agent maintains two peer lists in its config:

```toml
[[clients]]
name      = "<peer-name>"
ip        = "<ip-or-cidr>"
zone      = "<zone>"
inner_key = "<hex>"
outer_key = "<hex>"

[[servers]]
name      = "<peer-name>"
url       = "<wss://...>"
zone      = "<zone>"
inner_key = "<hex>"
outer_key = "<hex>"
```

Clients are **inbound** connections (agents that connect to this agent). Servers
are **outbound** connections (agents that this agent connects to). These lists
are managed via CLI commands — do not edit them by hand.

### 1.3 Extras (agent-specific key-value options)

Agents that need additional configuration (e.g. FreeIPA credentials, Slurm
settings) use a flat key-value map in the config:

```toml
[extras]
some-option    = "plaintext value"
some-password  = "<encrypted-hex>"    # stored via 'secret' CLI command
```

Plain options are set with the `extra` subcommand; secrets are stored encrypted
with the `secret` subcommand (see §2).

---

## 2. Common CLI Commands (all agents)

All agents built on `agent_core` share the following subcommands. Run
`<agent-binary> --help` for the full list.

```
<agent> [--config-file <path>] <subcommand>
```

### `init`

Create and write a new configuration file.

```
<agent> init [--service <name>] [--url <url>] [--ip <ip>] [--port <port>]
             [--healthcheck-port <port>] [--proxy-header <header>] [--force]
```

| Flag | Description |
|------|-------------|
| `--service` | Agent name |
| `--url` | Public WebSocket URL |
| `--ip` | Listen IP |
| `--port` | Listen port |
| `--healthcheck-port` | Optional health check port |
| `--proxy-header` | Optional reverse proxy client-IP header |
| `--force` | Overwrite existing config file |

### `client`

Manage inbound peers (agents that connect to this one).

```
<agent> client --add <name> --ip <ip-or-cidr> [--zone <zone>]
<agent> client --remove <name> [--zone <zone>]
<agent> client --list
<agent> client --rotate <name> [--zone <zone>]
```

`--add` generates fresh keys and writes an invite file
(`invite_<name>_<zone>.toml`) to the current directory. Give this file to
the remote agent operator to import.

`--rotate` generates new keys and writes a rotation invite file
(`rotate_<name>_<zone>.toml`).

### `server`

Manage outbound peers (agents that this one connects to).

```
<agent> server --add <invite-file>
<agent> server --remove <name> [--zone <zone>]
<agent> server --list
<agent> server --rotate <invite-file>
```

`--add` imports the invite file produced by the remote agent's `client --add`
command.

### `encryption`

Set config file encryption for secrets stored in the `extras` map.

```
<agent> encryption --simple
<agent> encryption --environment <ENV_VAR_NAME>
```

See [security-model.md](security-model.md) §5 for details.

### `extra`

Store a plaintext key-value option in the config.

```
<agent> extra --key <key> --value <value>
```

### `secret`

Store an encrypted key-value secret in the config. The config file's
`encryption` scheme must be configured first.

```
<agent> secret --key <key> --value <plaintext-value>
```

The value is encrypted and stored in `extras`. Read back at runtime via
`config.secret("<key>")`.

### `run`

Start the agent.

```
<agent> run
<agent> run --one-shot "<command>" [--repeat <n>] [--sender <name>] [--zone <zone>]
```

`--one-shot` submits one or more OpenPortal instructions at startup and exits
when all complete. Useful for scripting or testing. `--repeat` repeats each
command `n` times.

---

## 3. Agent-Specific Configuration

### 3.1 Portal (`op-portal`)

The portal agent routes requests between bridge/virtual agents and downstream
providers.

| Default | Value |
|---------|-------|
| Name | `portal` |
| Config file | `~/.config/openportal/portal-config.toml` |
| WebSocket port | `8040` |
| Agent type | `Portal` |

No additional `extras` options beyond the common set.

**Typical peer relationships:**
- **Client:** one or more `bridge` agents (they connect inbound to the portal)
- **Server:** one or more `provider` agents (the portal connects out to them)

---

### 3.2 Provider (`op-provider`)

The provider agent routes jobs from portals to platform agents.

| Default | Value |
|---------|-------|
| Name | `provider` |
| Config file | `~/.config/openportal/provider-config.toml` |
| WebSocket port | `8041` |
| Agent type | `Provider` |

No additional `extras` options beyond the common set.

**Typical peer relationships:**
- **Server:** one or more `portal` agents (portals connect inbound)
- **Client:** one or more `clusters` (platform) agents (provider connects out to them)

---

### 3.3 Bridge (`op-bridge`)

The bridge agent additionally runs an HTTP API server (see
[bridge-api.md](bridge-api.md)). Its `init` subcommand accepts extra flags for
the HTTP server:

```
op-bridge init ... --bridge-url <url> --bridge-ip <ip> --bridge-port <port>
                   --signal-url <url>
```

| Default | Value |
|---------|-------|
| Name | `bridge` |
| Config file | `~/.config/openportal/bridge-config.toml` |
| WebSocket port | `8044` |
| HTTP API port | `3000` |
| Agent type | `Bridge` |

**Additional config fields (under `[bridge]`):**

```toml
[bridge]
url        = "http://localhost:3000"
ip         = "127.0.0.1"
port       = 3000
key        = "<hex>"               # random API key, generated on init
signal_url = "http://localhost/signal"
```

| Field | Description |
|-------|-------------|
| `url` | Public base URL of the HTTP API server |
| `ip` | IP address to bind the HTTP API listener to |
| `port` | Port to bind the HTTP API listener to |
| `key` | 32-byte random HMAC key for authenticating API callers (see [bridge-api.md](bridge-api.md) §2) |
| `signal_url` | URL called by the bridge to notify the portal software of new jobs |

**Additional CLI subcommand:**

```
op-bridge bridge --config <invite-file>
op-bridge bridge --regenerate
```

`--config` writes the bridge invite file (URL + API key) for the portal
software client. `--regenerate` generates a new API key (requires distributing
a new invite file to all API clients).

**Environment variable:**

| Variable | Effect |
|----------|--------|
| `OPENPORTAL_ALLOW_INVALID_SSL_CERTS` | Set to `true` to skip TLS verification when calling `signal_url` (development only) |

**Typical peer relationships:**
- **Server:** one `portal` agent (portal connects inbound)

---

### 3.4 Clusters (`op-clusters`)

The clusters agent is a platform agent that manages multiple cluster instances.

| Default | Value |
|---------|-------|
| Name | `clusters` |
| Config file | `~/.config/openportal/clusters-config.toml` |
| WebSocket port | `8045` |
| Agent type | `Platform` |

No additional `extras` options beyond the common set.

**Typical peer relationships:**
- **Server:** one or more `provider` agents
- **Client:** one or more `cluster` (instance) agents

---

### 3.5 Cluster (`op-cluster`)

The cluster agent is an instance agent that manages a single cluster. It
coordinates account agents (FreeIPA) and filesystem agents.

| Default | Value |
|---------|-------|
| Name | `cluster` |
| Config file | `~/.config/openportal/cluster-config.toml` |
| WebSocket port | `8046` |
| Agent type | `Instance` |

No additional `extras` options beyond the common set.

**Typical peer relationships:**
- **Server:** one `clusters` (platform) agent
- **Client:** one `freeipa` agent, one `filesystem` agent, one `slurm` agent

---

### 3.6 FreeIPA (`op-freeipa`)

The FreeIPA agent manages user and project accounts in FreeIPA.

| Default | Value |
|---------|-------|
| Name | `freeipa` |
| Config file | `~/.config/openportal/freeipa-config.toml` |
| WebSocket port | `8046` |
| Agent type | `Account` |

**Required extras:**

| Key | Set via | Description |
|-----|---------|-------------|
| `freeipa-server` | `extra` | Hostname(s) of FreeIPA server(s). Comma-separated for multiple. The same server may be listed multiple times to allow concurrent connections. |
| `freeipa-password` | `secret` | FreeIPA admin password (encrypted at rest). |

**Optional extras:**

| Key | Set via | Default | Description |
|-----|---------|---------|-------------|
| `freeipa-user` | `extra` | `admin` | FreeIPA admin username. |
| `system-groups` | `extra` | `""` | Comma-separated list of FreeIPA groups to add all users to automatically. |
| `instance-groups` | `extra` | `""` | Per-instance group mappings. Format: `instance-name:group1,group2;...` |

**Example setup:**

```bash
op-freeipa init --service freeipa --url wss://freeipa-host:8046
op-freeipa encryption --environment OPENPORTAL_SECRET
op-freeipa extra --key freeipa-server --value ipa.example.com
op-freeipa extra --key freeipa-user --value admin
op-freeipa secret --key freeipa-password --value 'secret'
```

**Typical peer relationships:**
- **Server:** one `cluster` (instance) agent

---

### 3.7 Filesystem (`op-filesystem`)

The filesystem agent creates and manages user and project directories on a
shared filesystem, and optionally manages storage quotas.

| Default | Value |
|---------|-------|
| Name | `filesystem` |
| Config file | `~/.config/openportal/filesystem-config.toml` |
| WebSocket port | `8047` |
| Agent type | `Filesystem` |

Unlike other agents, the filesystem agent uses a **typed config block** (not
`extras`) embedded directly in the TOML file. The config is described below.

#### 3.7.1 Filesystem Config Structure

```toml
[quota_engines.<engine-name>]
type = "lustre"
# ... engine-specific fields

[user_volumes.<volume-name>]
roots       = ["/home"]
subpath     = "{project}/{user}"
permissions = "0755"
is_home     = true
quota_engine = "<engine-name>"    # optional
max_quota    = "1.00 TB"          # optional
default_quota = "100.00 GB"       # optional
mount_point  = "/mnt/lustre"      # optional
default_inode_limit = 1000000     # optional

[project_volumes.<volume-name>]
roots       = ["/projects"]
subpath     = "{project}"
permissions = "2770"
quota_engine = "<engine-name>"    # optional
max_quota    = "10.00 TB"         # optional
default_quota = "1.00 TB"         # optional
mount_point  = "/mnt/lustre"      # optional
default_inode_limit = 1000000     # optional
links        = [""]               # optional symlinks, one per root
```

#### 3.7.2 User Volume Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `roots` | array of strings | (required) | Root directory paths for this volume. Multiple roots share the same quota. |
| `subpath` | string | `{project}/{user}` | Directory path template within each root. Placeholders: `{project}`, `{user}`. |
| `permissions` | string or array | `"0755"` | Octal directory permissions. Provide a single value or one per root. |
| `is_home` | boolean | auto | Whether this is the primary home volume. Auto-set to `true` when only one user volume exists. At most one user volume can be the home. |
| `quota_engine` | string | (none) | Name of a `quota_engines` entry to use for quota management. |
| `max_quota` | size string | unlimited | Maximum allowed quota for any user. |
| `default_quota` | size string | unlimited | Default quota assigned to new users. |
| `mount_point` | string | (none) | Filesystem mount point (required by some quota engines). |
| `default_inode_limit` | integer | (engine default) | Default number of files/directories allowed. |

#### 3.7.3 Project Volume Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `roots` | array of strings | (required) | Root directory paths. |
| `subpath` | string | `{project}` | Directory path template. Placeholder: `{project}`. |
| `permissions` | string or array | `"2770"` | Octal directory permissions (SGID bit typical for shared directories). |
| `quota_engine` | string | (none) | Quota engine to use. |
| `max_quota` | size string | unlimited | Maximum allowed quota for any project. |
| `default_quota` | size string | unlimited | Default quota for new projects. |
| `mount_point` | string | (none) | Filesystem mount point. |
| `default_inode_limit` | integer | (engine default) | Default inode limit. |
| `links` | array of strings | `[]` | Symlink templates to create alongside each root. Empty string = no link for that root. Placeholder: `{project}`. |

#### 3.7.4 Lustre Quota Engine

```toml
[quota_engines.lustre]
type                   = "lustre"
lfs_command            = "lfs"
max_runners            = 4
command_timeout_secs   = 30
recursive_timeout_secs = 18000

[quota_engines.lustre.id_strategies]
home     = "{UID-1483800000}01"
scratch  = "{UID-1483800000}02"
projects = "{GID}"
```

| Field | Default | Description |
|-------|---------|-------------|
| `lfs_command` | `"lfs"` | Command to invoke `lfs`. May include a path, `sudo`, or container exec (e.g. `"sudo lfs"`). |
| `max_runners` | `4` | Maximum concurrent `lfs` commands (excluding recursive project operations). |
| `command_timeout_secs` | `30` | Timeout in seconds for standard `lfs` commands. |
| `recursive_timeout_secs` | `18000` | Timeout for `lfs project -srp` (recursive). Default is 5 hours to accommodate large directory trees. |
| `id_strategies` | (required) | Map of volume name → ID format string. |

**ID strategy format strings:**

Each volume that uses this engine needs an `id_strategies` entry. The format
string computes a numeric Lustre quota ID from the user's UID or group's GID:

| Variable | Value |
|----------|-------|
| `UID` | User's Unix UID |
| `GID` | Group's Unix GID |

Arithmetic expressions in `{...}` are evaluated: `{GID+1000}`, `{UID-100000}`.
Literals outside braces are appended: `"{UID-100000}01"` for UID 100125 →
`12501`.

**Typical peer relationships:**
- **Server:** one `cluster` (instance) agent

**Example full config:**

```toml
[quota_engines.lustre]
type         = "lustre"
lfs_command  = "sudo lfs"
max_runners  = 4

[quota_engines.lustre.id_strategies]
home     = "{UID-1483800000}01"
projects = "{GID}"

[user_volumes.home]
roots        = ["/home"]
subpath      = "{project}/{user}"
permissions  = "0755"
is_home      = true
quota_engine = "lustre"
default_quota = "100.00 GB"
mount_point  = "/mnt/lustre"

[project_volumes.projects]
roots        = ["/projects"]
subpath      = "{project}"
permissions  = "2770"
quota_engine = "lustre"
default_quota = "1.00 TB"
mount_point  = "/mnt/lustre"
```

---

### 3.8 Slurm (`op-slurm`)

The Slurm agent manages accounts, limits, and usage reporting in a Slurm
cluster. It can operate via the `sacctmgr` command-line tool or via the Slurm
REST API (`slurmrestd`). Which mode is used depends on whether `slurm-server`
is set.

| Default | Value |
|---------|-------|
| Name | `slurm` |
| Config file | `~/.config/openportal/slurm-config.toml` |
| WebSocket port | `8048` |
| Agent type | `Scheduler` |

#### 3.8.1 Options (sacctmgr mode — `slurm-server` not set)

| Key | Set via | Default | Description |
|-----|---------|---------|-------------|
| `slurm-default-node` | `extra` | (required) | JSON object describing the default Slurm node type. Used when calculating job cost. |
| `slurm-cluster` | `extra` | `""` | Slurm cluster name (for multi-cluster deployments). |
| `slurm-partition` | `extra` | `""` | Slurm partition name. |
| `parent-account` | `extra` | `"root"` | Parent Slurm account that all project accounts are created under. |
| `sacct` | `extra` | `"sacct"` | Path or command for `sacct`. |
| `sacctmgr` | `extra` | `"sacctmgr"` | Path or command for `sacctmgr`. |
| `scontrol` | `extra` | `"scontrol"` | Path or command for `scontrol`. |
| `scancel` | `extra` | `"scancel"` | Path or command for `scancel`. |
| `max-slurm-runners` | `extra` | `"5"` | Maximum concurrent Slurm command invocations. |

#### 3.8.2 Options (REST API mode — `slurm-server` is set)

All of the sacctmgr-mode options above apply, plus:

| Key | Set via | Default | Description |
|-----|---------|---------|-------------|
| `slurm-server` | `extra` | `""` | Base URL of `slurmrestd` (e.g. `http://slurm-host:6820`). Setting this switches to REST API mode. |
| `slurm-user` | `extra` | `""` | Slurm username for REST API authentication. |
| `token-command` | `extra` | (required in REST mode) | Shell command that prints a valid JWT token to stdout. |
| `token-lifespan` | `extra` | `"1800"` | JWT token lifespan in seconds (minimum 10). |

**Typical peer relationships:**
- **Server:** one `cluster` (instance) agent

---

## 4. Default Port Reference

| Agent | Binary | Default port |
|-------|--------|-------------|
| Portal | `op-portal` | 8040 |
| Provider | `op-provider` | 8041 |
| Bridge (WebSocket) | `op-bridge` | 8044 |
| Bridge (HTTP API) | `op-bridge` | 3000 |
| Clusters (platform) | `op-clusters` | 8045 |
| Cluster (instance) | `op-cluster` | 8046 |
| FreeIPA | `op-freeipa` | 8046 |
| Filesystem | `op-filesystem` | 8047 |
| Slurm | `op-slurm` | 8048 |

Note: `op-cluster` and `op-freeipa` share the same default port (8046) because
they are typically deployed on different machines. Adjust with `--port` if
collocated.

---

## 5. Typical Deployment Setup

A minimal portal-to-cluster deployment involves the following setup steps, in
order:

```
# 1. Initialise each agent
op-portal   init --service waldur   --url wss://portal-host:8040
op-provider init --service provider --url wss://provider-host:8041
op-bridge   init --service bridge   --url wss://portal-host:8044 \
                 --bridge-url http://portal-host:3000

# 2. Wire portal → provider (portal is the client, provider is the server)
op-provider client --add waldur  --ip <portal-ip>
# → produces invite_waldur_default.toml
op-portal   server --add invite_waldur_default.toml

# 3. Wire bridge → portal (portal is the server, bridge is the client)
op-portal  client --add bridge --ip <bridge-ip>
# → produces invite_bridge_default.toml
op-bridge  server --add invite_bridge_default.toml

# 4. Write bridge API invite for portal software
op-bridge bridge --config bridge-invite.toml

# 5. Add agent-specific options (e.g. FreeIPA)
op-freeipa encryption --environment OPENPORTAL_SECRET
op-freeipa extra   --key freeipa-server --value ipa.example.com
op-freeipa secret  --key freeipa-password --value 'secret'

# 6. Run agents
op-portal   run
op-provider run
op-bridge   run
op-freeipa  run
```

---

## 6. Source File Reference

| Concept | Source file |
|---------|-------------|
| Common `Config<T>`, `Defaults<T>`, CLI | `templemeads/src/agent_core.rs` |
| Bridge-specific config and CLI | `templemeads/src/agent_bridge.rs` |
| Paddington `ServiceConfig`, `ClientConfig`, `ServerConfig` | `paddington/src/config.rs` |
| Bridge HTTP server config | `templemeads/src/bridge_server.rs` |
| FreeIPA main (option names) | `freeipa/src/main.rs` |
| Slurm main (option names) | `slurm/src/main.rs` |
| Filesystem volume config | `filesystem/src/volumeconfig.rs` |
| Lustre quota engine | `filesystem/src/lustreengine.rs` |
