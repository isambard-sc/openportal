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
trusted_proxy   = "<ip-or-cidr-list>"
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
| `ip` | string | IP address to bind the WebSocket listener to. A single IPv4 or IPv6 address (not a range or list - see the note below). |
| `port` | integer | Port to bind the WebSocket listener to. |
| `heathcheck_port` | integer (optional) | If set, a minimal HTTP health endpoint is exposed on this port (responds `200 OK` to `GET /`). |
| `proxy_header` | string (optional) | HTTP header to read the real client IP from when behind a reverse proxy (e.g. `X-Forwarded-For`). Only honoured together with `trusted_proxy` (see below). |
| `trusted_proxy` | string (optional) | IP address(es)/range(s) of reverse proxies whose `proxy_header` may be trusted - same comma-separated IP/CIDR syntax as a client's `ip`. A forwarded client address is honoured **only** when the real TCP peer matches this list; otherwise the header is ignored (fail-closed). Required for `proxy_header` to have any effect. For a Cloudflare tunnel or in-cluster ingress on loopback, use e.g. `"127.0.0.0/8"`. See [security-review.md](security-review.md) F3/F6. |
| `agent` | string | Agent type tag stored in the config. Set automatically by `init`. |
| `encryption` | table (optional) | Encryption scheme for secrets stored in the config file. See [security-model.md](security-model.md) §5. |

This `ip` (the listener's own bind address) is always a single address,
IPv4 or IPv6 (`10.1.2.3` or `2001:db8::1`) - never a range or a list. See
[security-model.md](security-model.md) §4.1 for the one thing IPv6
support here doesn't cover: dual-stack listening.

A `[[clients]]` entry's `ip` (§1.2) is more flexible: a single address, a
CIDR range (`10.0.0.0/24` or `2001:db8::/32`), or a comma-separated list
of several addresses and/or ranges, any one of which is allowed to match -
e.g. `ip = "127.0.0.1,10.0.0.0/24,2001:db8::/32"`. IPv4 and IPv6 entries
can be freely mixed in the same list.

### 1.2 Peer Lists

Each agent maintains two peer lists in its config:

```toml
[[clients]]
name      = "<peer-name>"
ip        = "<ip-or-cidr>"
zone      = "<zone>"
inner_key = "<hex>"
outer_key = "<hex>"
proxy     = "<relay-agent-name>"    # optional
type      = "<agent-type>"          # optional

[[servers]]
name      = "<peer-name>"
url       = "<wss://...>"
zone      = "<zone>"
inner_key = "<hex>"
outer_key = "<hex>"
proxy     = "<relay-agent-name>"    # optional
type      = "<agent-type>"          # optional
```

Clients are **inbound** connections (agents that connect to this agent). Servers
are **outbound** connections (agents that this agent connects to). These lists
are managed via CLI commands — do not edit them by hand.

`type` declares the agent type this peer is **expected** to present itself as -
one of `portal`, `provider`, `platform`, `instance`, `bridge`, `account`,
`filesystem`, `scheduler`, `virtual`. When set, a peer that registers claiming
any other type is refused and the mismatch is logged as an error.

Both lists' `type` is populated by the CLI, from opposite ends: a `[[clients]]`
entry's comes from `client --add --type` (§3), and a `[[servers]]` entry's comes
from the `type` the issuing agent wrote into the invite. Neither needs
hand-editing.

This matters because a peer's role otherwise arrives entirely over the wire, and
the framework makes real authorization decisions from it — which peer a portal
will accept a `Submit` from, which peer may restart an agent or read its
diagnostics, which peer an instance routes account operations to. Declaring the
expectation out-of-band, alongside the keys, means a compromised peer cannot
claim more authority than it was provisioned with. See
[security-review-2.md](security-review-2.md) (finding R3).

`type` is **optional and unset by default**, and an entry without it is not
checked — so existing configs keep working unchanged and the check can be
adopted one peer at a time. An unset peer's claimed type is logged at debug
level, naming the value to add, so the remaining gaps are discoverable. An
unrecognised value is logged as an error and treated as unset rather than
rejecting the peer.

`proxy` is set only when this peer can only be reached through a blind
relay proxy (an `op-proxy` agent) rather than directly — see
[§3.11](#311-blind-relay-proxy-op-proxy) and
[blind-relay-proxy-design.md](../plans/archive/blind-relay-proxy-design.md). A
relayed `[[clients]]` entry has no `ip` (authentication comes from
completing the relayed handshake, not an IP allowlist); a relayed
`[[servers]]` entry has no meaningful `url` (it is reached via the named
proxy instead). Each relayed peer names its own proxy independently — a
service can freely mix relayed and directly-connected peers, and can use
*different* proxies for different relayed peers, as long as each named
proxy is itself a known `servers` entry.

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
             [--healthcheck-port <port>] [--proxy-header <header>]
             [--trusted-proxy <ip-or-cidr-list>] [--force]
```

| Flag | Description |
|------|-------------|
| `--service` | Agent name |
| `--url` | Public WebSocket URL |
| `--ip` | Listen IP |
| `--port` | Listen port |
| `--healthcheck-port` | Optional health check port |
| `--proxy-header` | Optional reverse proxy client-IP header |
| `--trusted-proxy` | IP/range(s) of trusted reverse proxies whose `--proxy-header` may be believed (required for it to have any effect). For the bridge, also gates the HTTP layer's forwarded-IP trust. See [security-review.md](security-review.md) F3/F6. |
| `--force` | Overwrite existing config file |

### `client`

Manage inbound peers (agents that connect to this one).

```
<agent> client --add <name> --ip <ip-or-cidr> [--zone <zone>] [--type <agent-type>]
<agent> client --add <name> --proxy <relay-name> [--zone <zone>] [--type <agent-type>]
<agent> client --remove <name> [--zone <zone>]
<agent> client --list
<agent> client --rotate <name> [--zone <zone>]
```

`--add` generates fresh keys and writes an invite file
(`invite_<issuer>_<zone>.toml`) to the current directory, where `<issuer>` is
**this agent's own service name** - the one it was given by `init --service`
- and *not* the name of the client being added. Give this file to the remote
agent operator to import.

Because the filename depends only on the issuer, every invite an agent writes
in the same directory and zone lands on the same path: issuing invites for
several clients one after another overwrites the earlier files. Run each
`client --add` in its own directory (or move the file away) before issuing
the next.

`--type <agent-type>` records the `type` this client must present itself as
(§1.2) - e.g. `--type bridge`. You get this value by **asking the operator of
that agent**; it is deliberately never derived from anything the client sends,
since the whole purpose of the check is to have a local, out-of-band statement of
what the peer is meant to be. The value is validated when you run the command, so
a typo fails immediately rather than being written to the config and then ignored
at startup. If omitted, this peer's claimed type is not checked, and a warning
says so.

The generated invite also declares **this** agent's own type, so the importing
side's expectation of *you* needs no manual step. Note the asymmetry: the invite
carries what the issuer is, never what the client is expected to be.

`--proxy <relay-name>` introduces a client that can only reach this agent
through a blind relay proxy (`relay-name` must already be a known
`servers` entry - i.e. an `op-proxy` invite already imported via
`server --add`) rather than a direct IP allowlist; `--ip` is not required
and is ignored if given alongside `--proxy`. The generated invite carries
the relay's name, so the importing side's `server --add` (below) picks it
up automatically. See [§3.11.1](#3111-connecting-two-real-agents-through-a-proxy)
for a full worked example.

`--rotate` generates new keys and writes a rotation invite file
(`rotate_<issuer>_<zone>.toml`), named after the issuing agent on the same
convention as `--add`.

### `server`

Manage outbound peers (agents that this one connects to).

```
<agent> server --add <invite-file>
<agent> server --remove <name> [--zone <zone>]
<agent> server --list
<agent> server --rotate <invite-file>
```

`--add` imports the invite file produced by the remote agent's `client --add`
command. No separate flag is needed for a relayed peer - if the invite was
created with `client --add --proxy`, it already names the relay, and this
agent picks it up automatically (as long as that same relay is already a
known `servers` entry here too).

The same applies to `type`: the invite declares the issuing agent's own type, so
the resulting `[[servers]]` entry is populated with it and the check is active
immediately. There is no `--type` flag here, and none is needed. An invite
written by a version before this was added simply has no `type`, imports
normally, and leaves that peer unchecked.

### `encryption`

Set config file encryption for secrets stored in the `extras` map (e.g. a
FreeIPA bind password or Slurm token added with `secret`).

```
<agent> encryption --simple
<agent> encryption --environment <ENV_VAR_NAME>
```

- `--environment` (**recommended for production**): derives the encryption key
  from the value of the named environment variable, via a strong salted Argon2
  derivation. Its strength is that of the operator-supplied secret.
- `--simple`: derives the key from the agent's own (non-secret) name. This is
  **obfuscation, not encryption** - anyone who can read the config file can
  re-derive the key - and is intended only for development/low-security use.

Secrets are written in a versioned format; re-running `secret` after upgrading
re-encrypts a value with the current (strong) scheme. See
[security-model.md](security-model.md) §5 and
[security-review.md](security-review.md) F2 for details.

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
| `freeipa-server` | `extra` | Hostname(s) of FreeIPA server(s). Comma-separated for multiple. The same server may be listed multiple times to allow concurrent connections. Each entry must name an individual server - see the note on replication below. |
| `freeipa-password` | `secret` | FreeIPA admin password (encrypted at rest). |

**Optional extras:**

| Key | Set via | Default | Description |
|-----|---------|---------|-------------|
| `freeipa-user` | `extra` | `admin` | FreeIPA admin username. |
| `freeipa-write-server` | `extra` | first entry of `freeipa-server` | Which server takes all writes. Must be one of the `freeipa-server` entries. |
| `freeipa-replication-window` | `extra` | `30` | Seconds that replication is assumed to need to converge. Writes are not moved to another server until the write server has been confirmed down for at least this long, nor moved back until it has been up again for that long. |
| `system-groups` | `extra` | `""` | Comma-separated list of FreeIPA groups to add all users to automatically. |
| `instance-groups` | `extra` | `""` | Per-instance group mappings. Format: `instance-name:group1,group2;...` |

**Multi-master topologies:**

Reads are spread over every server in `freeipa-server`; writes all go to one,
because FreeIPA's multi-master replication cannot reconcile two independent
`ADD`s of the same DN. When that happens 389-ds keeps one copy, renames the
other to `nsuniqueid=<uuid>+uid=<user>,...` and flags it `nsds5ReplConflict`.
Such entries are invisible to ordinary LDAP searches and cannot be removed
with `ipa user-del`, so they accumulate silently and cleaning them up is
manual work as Directory Manager.

Two things follow for how this is configured:

- Every `freeipa-server` entry must name an **individual** server. A VIP or a
  round-robin DNS alias is several masters behind one name, so pinning writes
  to it pins nothing.
- Writes only ever go to one server at a time, but *which* server can change:
  if the write server is confirmed down for longer than
  `freeipa-replication-window`, one replacement is elected, in configuration
  order. "Confirmed down" means a refused connection, a rejected or timed-out
  login, or a run of `3` consecutive calls that went unanswered - never a
  single slow call, because one timeout is indistinguishable from a write that
  landed and whose response was lost. A server that is listening but not
  answering also has its session discarded after each timeout, so the next call
  has to log in again and that login becomes an independent check on whether it
  is alive. It reverts once the original has been up again for a full window -
  recovery waits for the same reason failover does, since a server that has
  just come back may not have caught up with what stood in for it. If nothing
  is fit to take writes, calls fail rather than being sent to a server that may
  be behind.
- The number of times a server is listed is the number of concurrent
  connections it gets. Since any server may end up taking the writes, list
  them all the same number of times - the write server's slots are the limit on
  write concurrency, and after a failover that will be a different server's
  slots. A server listed once serialises writes.

`op-freeipa` also checks every configured server before concluding that a user
or group does not exist, since a master that has not yet received a recent add
would say it does not. Any check it could not complete is logged with the
`REPLICATION-RISK` marker, as is a failover away from the write server. Run
`scripts/check-replication-conflicts.sh` to look for conflict entries that
already exist.

**Example setup:**

```bash
op-freeipa init --service freeipa --url wss://freeipa-host:8046
op-freeipa encryption --environment OPENPORTAL_SECRET
op-freeipa extra --key freeipa-server --value https://ipa1.example.com,https://ipa2.example.com
op-freeipa extra --key freeipa-write-server --value https://ipa1.example.com
op-freeipa extra --key freeipa-user --value admin
op-freeipa secret --key freeipa-password --value 'secret'
```

**Typical peer relationships:**
- **Server:** one `cluster` (instance) agent

---

### 3.6.1 Local Account (`op-localaccount`)

> **⚠️ Testing agent only.** `op-localaccount` is intended for **testing** —
> specifically for managing accounts in a containerised test Slurm cluster. Use
> `op-freeipa` for production account management. The agent logs a warning on
> every startup to make a mistaken production deployment obvious. It is
> nonetheless written to fail safe against a real system: it only removes
> accounts and groups it manages — a user must be in the managed group before it
> is deleted, and a group must have a normal (non-system) GID and not be a
> configured system/managed group before it is deleted (see
> [security-review.md](security-review.md) F13).

The local account agent manages user and project accounts using standard Unix
commands (`useradd`, `groupadd`, etc.). It implements the same Account agent
interface as `op-freeipa` but is intended for testing — particularly inside a
Slurm Docker container where the commands can be prefixed with
`docker exec slurmctld` to run with the necessary privileges.

| Default | Value |
|---------|-------|
| Name | `localaccount` |
| Config file | `~/.config/openportal/localaccount-config.toml` |
| WebSocket port | `8047` |
| Agent type | `Account` |

**Optional extras:**

| Key | Set via | Default | Description |
|-----|---------|---------|-------------|
| `useradd` | `extra` | `"useradd"` | Command to add a user. |
| `userdel` | `extra` | `"userdel"` | Command to remove a user. |
| `groupadd` | `extra` | `"groupadd"` | Command to add a group. |
| `groupdel` | `extra` | `"groupdel"` | Command to remove a group. |
| `usermod` | `extra` | `"usermod"` | Command to modify a user. |
| `getent` | `extra` | `"getent"` | Command to query the user/group database. |
| `gpasswd` | `extra` | `"gpasswd"` | Command to remove a user from a group (used by `unblock_user`). |
| `managed-group` | `extra` | `"openportal"` | Name of the Unix group added to every managed user (used to distinguish agent-created users from pre-existing system accounts). |
| `system-groups` | `extra` | `""` | Comma-separated list of Unix groups to add all managed users to. |
| `instance-groups` | `extra` | `""` | Per-instance group mappings. Format: `"instance:group,instance:group2,..."` |

All command strings may include a full prefix such as
`"docker exec slurmctld useradd"` to redirect execution into a container.

**Example setup (Slurm Docker container):**

```bash
op-localaccount init --service localaccount --url wss://localhost:8047
op-localaccount extra --key useradd   --value "docker exec slurmctld useradd"
op-localaccount extra --key userdel   --value "docker exec slurmctld userdel"
op-localaccount extra --key groupadd  --value "docker exec slurmctld groupadd"
op-localaccount extra --key groupdel  --value "docker exec slurmctld groupdel"
op-localaccount extra --key usermod   --value "docker exec slurmctld usermod"
op-localaccount extra --key getent    --value "docker exec slurmctld getent"
op-localaccount extra --key gpasswd   --value "docker exec slurmctld gpasswd"
```

**Group management:**

For each user, the agent ensures the following groups exist before adding the
user to them:

1. The project group (e.g. `brics.aiproject`)
2. The managed group (default `openportal`)
3. An auto-generated per-instance group `op-<instance-name>` (non-alphanumeric
   characters replaced with `_`)
4. Any groups listed in `system-groups`
5. Any groups listed in `instance-groups` for the relevant instance

**Blocking and unblocking:**

The agent supports `block_user` and `unblock_user` instructions (and the
convenience `block_project` / `unblock_project` instructions handled by the
cluster agent). Blocking a user:

- Adds the user to the `{managed-group}.blocked` group (e.g. `openportal.blocked`),
  which is created automatically on first use. This group is the source of truth
  for blocked status.
- Locks the Unix account with `usermod -L`, preventing password-based login.

Unblocking reverses both steps: `gpasswd -d` removes the user from the blocked
group and `usermod -U` re-enables the account. `add_user` will not re-enable a
blocked user — only `unblock_user` can do that.

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

Unlike most agents, the filesystem agent uses a **typed config block** (not
`extras`) embedded directly in the TOML file. The config is described below.

One optional extra *is* supported:

| Key | Set via | Default | Description |
|-----|---------|---------|-------------|
| `exec-prefix` | `extra` | `""` | Space-separated command prefix prepended to all filesystem operations (mkdir, chown, chmod, mv, ln, touch, rm). When set, every operation runs via an external command instead of native Rust stdlib. Example: `"docker exec slurmctld"`. Leave empty (default) to use native Rust calls. |

**Example (redirect filesystem operations into a Slurm container):**

```bash
op-filesystem extra --key exec-prefix --value "docker exec slurmctld"
```

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

**Typical peer relationships (filesystem agent):**
- **Server:** one `cluster` (instance) agent

**Example full config (Lustre):**

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

#### 3.7.5 Linux Quota Engine

Uses the standard Linux `setquota` / `repquota` utilities to manage per-user
and per-group quotas on any filesystem that supports the kernel quota interface
(ext4, xfs, etc.). Both commands are configurable so they can be prefixed for
container execution.

> **Note:** Linux quotas require a real Linux kernel with `quotactl` support.
> Overlay filesystems (e.g. Docker on Mac) do not support this engine.
> Use the Fake engine (§3.7.6) for local Mac/Docker testing instead.

```toml
[quota_engines.linuxquota]
type       = "linux"
filesystem = "/dev/sda1"         # device or mount point
setquota   = "docker exec slurmctld setquota"   # optional, default "setquota"
repquota   = "docker exec slurmctld repquota"   # optional, default "repquota"
```

| Field | Default | Description |
|-------|---------|-------------|
| `filesystem` | (required) | Filesystem device or mount point to manage quotas on (as seen inside the container when using exec-prefix). |
| `setquota` | `"setquota"` | Command to set quotas. May include an exec prefix. |
| `repquota` | `"repquota"` | Command to report quotas. May include an exec prefix. |

Block limits are specified in kilobytes (`0` = unlimited). Inode limits use the
per-volume `default_inode_limit` setting (`0` = unlimited).

**Example full config (Linux quotas, Slurm container):**

```toml
[quota_engines.linuxquota]
type       = "linux"
filesystem = "/home"
setquota   = "docker exec slurmctld setquota"
repquota   = "docker exec slurmctld repquota"

[user_volumes.home]
roots        = ["/home"]
subpath      = "{project}/{user}"
permissions  = "0755"
is_home      = true
quota_engine = "linuxquota"
default_quota = "100.00 GB"
mount_point  = "/home"
```

---

#### 3.7.6 Fake Quota Engine

A test-only quota engine that stores quota limits as plain-text files on the
agent host and measures disk usage with `du`.  No real quota enforcement
happens — it just records the configured limits and reports current usage
against them.  Useful for testing the full OpenPortal quota plumbing on Mac /
Docker setups where real quota filesystems are unavailable.

```toml
[quota_engines.fakequota]
type      = "fake"
quota_dir = "/tmp/openportal-fakequota"   # host-side directory for limit files
du        = "docker exec slurmctld du"    # optional, default "du"
```

| Field | Default | Description |
|-------|---------|-------------|
| `quota_dir` | `"/tmp/openportal-fakequota"` | Host-side directory where quota limit files are written by this agent. Created automatically if absent. |
| `du` | `"du"` | Command used to measure disk usage (`du -sk`). May include an exec prefix to run inside a container. |

Quota limit files are named `user_<local-user>` and `group_<local-group>` and
contain a single quota size string (e.g. `100 GB` or `unlimited`).

**Example full config (fake quotas, Mac + Docker testing):**

```toml
[quota_engines.fakequota]
type      = "fake"
quota_dir = "/tmp/openportal-fakequota"
du        = "docker exec slurmctld du"

[user_volumes.home]
roots        = ["/home"]
subpath      = "{project}/{user}"
permissions  = "0755"
is_home      = true
quota_engine = "fakequota"
default_quota = "100.00 GB"

[project_volumes.projects]
roots        = ["/projects"]
subpath      = "{project}"
permissions  = "2770"
quota_engine = "fakequota"
default_quota = "1.00 TB"
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

### 3.9 Cloud Account (`op-cloudaccount`)

The cloud account agent represents a single cloud account (e.g. one AWS
account) assigned to a project. Unlike the cluster/slurm split, it is a
single agent that merges the Instance and Scheduler roles: there is no
cloud-side API yet to record project/user assignment or to query usage, so
this agent is both the source of truth for assignment and the thing that
turns whatever cost-report files the cloud operators drop into a
`ProjectUsageReport`. See `docs/plans/archive/op-cloudaccount-design.md` for the
full design and rationale.

| Default | Value |
|---------|-------|
| Name | `cloudaccount` |
| Config file | `~/.config/openportal/cloudaccount-config.toml` |
| WebSocket port | `8049` |
| Agent type | `Instance` |

**Optional extras:**

| Key | Set via | Default | Description |
|-----|---------|---------|-------------|
| `state-dir` | `extra` | `~/.config/openportal/cloudaccount-state` | Directory holding one JSON file per assigned project, recording which projects/users have been added to this cloud account. Written to atomically; safe to inspect or edit by hand while debugging. |
| `accounting-dir` | `extra` | `~/.config/openportal/cloudaccount-accounting` | Directory the cloud operators' cron job drops cost-report JSON files into (same shape as `cost_payload_example.json`). Read-only to this agent - files here are never modified or deleted. |
| `currency` | `extra` | `"USD"` | Expected currency code. A cost-report file reporting a different currency is logged as a warning, not converted (no FX support). |

**Usage reporting:**

`Usage` (normally a count of compute-seconds elsewhere in OpenPortal) is
reinterpreted here as micro-currency-units: 1 `Usage` second = 1e-6 of the
configured currency. Cost-report totals are cumulative from
`time_period.start`, so usage reports are reconstructed by diffing
consecutive reports and spreading the delta evenly across the calendar
days it spans - see the design doc for the full algorithm. Anything
consuming a cloud account's `ProjectUsageReport` needs to know to divide
by 1e6 and format as currency rather than using `Usage`'s default
duration-formatting `Display` impl.

**Typical peer relationships:**
- **Server:** one `provider` agent (this agent plays the Instance role
  directly - it has no platform agent equivalent to `op-clusters` yet, and
  no separate account/filesystem/scheduler peers)

---

### 3.10 Cloud Portal (`op-cloudportal`)

The cloud portal agent is a self-contained `Portal` agent representing the
"cloud" side of a portal-to-portal relationship (e.g. a central "airr"
portal creating Awards on it). There is no real portal management
software (no Waldur) behind it - like `op-cloudaccount`, it is a
deliberately rough prototype that stores Award state itself instead of
relaying to a bridge. See `docs/plans/archive/op-cloudportal-design.md` for the
full design and rationale.

| Default | Value |
|---------|-------|
| Name | `cloudportal` |
| Config file | `~/.config/openportal/cloudportal-config.toml` |
| WebSocket port | `8050` |
| Agent type | `Portal` |

**Optional extras:**

| Key | Set via | Default | Description |
|-----|---------|---------|-------------|
| `state-dir` | `extra` | `~/.config/openportal/cloudportal-state` | Directory holding one JSON file per Award, recording its `AwardDetails`, approval `status` (`pending`/`approved`/`rejected`), and which members have been provisioned so far. Written to atomically; read fresh from disk on every instruction (no in-memory cache - see the design doc §5). |
| `offerings` | `extra` | `""` | Comma-separated `template:peer` pairs mapping an `AwardDetails.template` value to the `op-cloudaccount` peer name that offering should provision against, e.g. `"aws:cloudaccount-aws,azure:cloudaccount-azure"`. `create_project` fails with a clear error if `template` is missing or not present in this table - there's no sensible default cloud provider to fall back to. |

**Addressing model:** `airr` (or whichever upstream portal) addresses
`cloudportal` directly - a plain, ordinary portal-to-portal connection,
no different in kind from any other pair of connected agents. There is
no virtual-resource/offering indirection (`op-portal`'s
`sync_offerings`/`virtual_resource_runner` mechanism does not work for
this - see the design doc §4 for why); which cloud provider an Award
targets is carried entirely in `AwardDetails.template`.

**Approval workflow:** Award creation (`create_project`) and
infrastructure provisioning are deliberately decoupled - there is a
human in the loop, since provisioning spends real money on a real cloud
account. Three bespoke CLI subcommands, alongside the common ones in
§2, manage this (they are pure state-file edits and never touch the
network themselves):

```bash
op-cloudportal list-pending
op-cloudportal approve --project someproject.cloud
op-cloudportal reject  --project someproject.cloud --reason "..."
```

`approve` only flips the Award's status - the actual `add_project`/
`add_user` calls against the resolved `op-cloudaccount` are made by a
background poller inside the running `op-cloudportal run` process
(checks every 30 seconds), so provisioning naturally retries if it
partially fails.

**Typical peer relationships:**
- **Server:** the upstream portal (e.g. `airr`)
- **Client:** one or more `op-cloudaccount` agents, resolved per-Award via
  the `offerings` table

---

### 3.11 Blind Relay Proxy (`op-proxy`)

Unlike every other agent in this document, `op-proxy` is **not** built on
`templemeads::agent_core` - it depends only on `paddington`, has no
`Domain`, no Jobs, and its own bespoke CLI (not the common CLI in §2). It
exists purely to relay encrypted traffic between a pair of agents that can
each only make outbound connections (neither can open a port the other can
reach); it never decrypts what it forwards. See
[blind-relay-proxy-design.md](../plans/archive/blind-relay-proxy-design.md) for the
full design.

| Default | Value |
|---------|-------|
| Name | `proxy` |
| Config file | `~/.config/openportal/proxy-config.toml` |
| WebSocket port | `8060` |

**CLI commands:**

```bash
# Initialise the proxy service
op-proxy init [--url <url>] [--ip <ip>] [--port <port>] [--config-file <path>]

# Introduce an agent that will connect to this proxy directly - one of the
# two real hops in a relayed pair. This is NOT the relay policy itself.
# --ip is required (there is no "match everyone" default).
op-proxy client --add <name> --ip <ip-or-cidr> [--zone <zone>]
                [--invitation <path>] [--config-file <path>]

# Allow two introduced agents to be relayed between each other.
# Default-deny: no pair is relayed unless explicitly allowed here.
op-proxy allow <a> <b> [--config-file <path>]

# Run the proxy
op-proxy run [--config-file <path>]
```

`op-proxy` has no `--type` flag, and records no agent type in either
direction. A blind relay is not an agent in the framework's sense: it has no
`agent::Type` of its own to declare in the invites it issues, and it never
inspects the roles of the peers it relays between - its authorization is the
explicit `allow` pair list below, not anything about what those peers are.

`client --add` writes an invite file the same way any other agent's
`client --add` does - give it to the introduced agent's operator so they
can import it with their own agent's `server --add`. Introducing an agent
to the proxy is a separate step from allowing it to be relayed to another
agent: `client --add` only lets that agent connect to the proxy itself;
`allow` is what actually lets traffic flow between two introduced agents.

**Relay policy:** stored as a `[policy]` table in the same config file
(`pairs = [["airr", "brics"]]`) - a flat list of allowed `(from, to)`
pairs, checked in both directions, managed via the `allow` subcommand
above. There is no CLI to remove a pair; edit the config file's `[policy]`
table by hand and restart to revoke one.

**Typical peer relationships:**
- **Client:** every agent it relays for (both the relayed "server" and
  relayed "client" role connect to the proxy the same way - as an ordinary
  paddington client)

#### 3.11.1 Connecting two real agents through a proxy

Every other agent in this document (all built on the common CLI in §2)
can act as one of the two relayed peers - the `client`/`server`
subcommands take a `--proxy <relay-name>` flag for exactly this. Worked
example: `airr` (an `op-portal`) and `brics` (an `op-cloudportal`) can
each only make outbound connections, so they talk through a shared
`proxy` (`op-proxy`).

```bash
# 1. Both airr and brics are introduced to the proxy like any other
#    client - this secures each real agent<->proxy hop, and is separate
#    from allowing airr and brics to be relayed to each other
op-proxy client --add airr  --ip <airr-ip>  --invitation invite_airr.toml
op-proxy client --add brics --ip <brics-ip> --invitation invite_brics.toml
airr  server --add invite_airr.toml
brics server --add invite_brics.toml

# 2. Allow the proxy to relay between them (default-deny otherwise)
op-proxy allow airr brics

# 3. airr and brics exchange their own pre-shared key pair - the proxy
#    never sees this one. --proxy here names the *local* servers entry
#    airr already has for the proxy (added in step 1, "proxy" by
#    default) - airr becomes the relayed "server" (it waits).
airr client --add brics --proxy proxy
# -> writes ./invite_airr_default.toml (named after airr, the issuer -
#    same convention as an ordinary client --add)

# 4. brics imports that invite - no --proxy flag needed here: the
#    invite itself already carries which relay to use (embedded in step
#    3), so this is auto-detected. brics becomes the relayed "client"
#    (it initiates the bootstrap).
brics server --add invite_airr_default.toml
```

Once both are running (`airr run` / `brics run`, alongside `proxy run`),
`brics` bootstraps a session with `airr` automatically at startup and
re-bootstraps (with fresh session keys) on every reconnect - nothing
templemeads-level (`Register`, `Sync`, Jobs, Notifications, health
cascades, ...) needs to know a proxy is involved at all.

Validated end-to-end with real compiled `op-proxy`/`op-portal`/
`op-cloudportal` processes: the bootstrap completes, both sides log their
synthesised `Connected` event, and `Register` (part of every agent's
normal post-handshake sequence) is relayed and processed correctly.

**Resilience:** bootstrap retries indefinitely (the same cadence a direct
connection retries at) if the other side isn't connected to the proxy
yet, so agents can be started in any order. If `airr` (the relayed
*server*, which only ever waits) restarts and loses its session state,
`brics` finds out automatically the next time it sends anything - `airr`
tells it to redo the handshake rather than silently dropping the message
- so no operator action is needed either way; see
[security-model.md](security-model.md) §7.1 and
[wire-protocol.md](wire-protocol.md) §7.3.

**Multiple proxies:** an agent isn't limited to one proxy - `airr` could
just as easily relay to `brics` via `proxy1` and to a third peer via
`proxy2`, alongside any number of ordinary direct connections, all at the
same time. Each relayed peer entry names its own proxy independently.

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
| Cloud Account | `op-cloudaccount` | 8049 |
| Cloud Portal | `op-cloudportal` | 8050 |
| Blind Relay Proxy | `op-proxy` | 8060 |

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
# → produces invite_provider_default.toml (named after the issuer,
#   op-provider, not after the client "waldur" it admits)
op-portal   server --add invite_provider_default.toml

# 3. Wire bridge → portal (portal is the server, bridge is the client)
op-portal  client --add bridge --ip <bridge-ip>
# → produces invite_waldur_default.toml (the portal was initialised as
#   --service waldur in step 1)
op-bridge  server --add invite_waldur_default.toml

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
| Cloud account main (option names) | `cloudaccount/src/main.rs` |
| Cloud account assignment state | `cloudaccount/src/state.rs` |
| Cloud account usage-report reconstruction | `cloudaccount/src/accounting.rs` |
| Cloud portal main (option names, CLI subcommands, poller) | `cloudportal/src/main.rs` |
| Cloud portal Award state | `cloudportal/src/state.rs` |
| Cloud portal email/UserIdentifier mapping | `cloudportal/src/identity.rs` |
| Portal one-shot CLI mode | `templemeads/src/portal.rs` |
| Blind relay proxy main (CLI subcommands) | `proxy/src/main.rs` |
| Blind relay protocol, `RelayPolicy` | `paddington/src/relay.rs` |
| `proxy` config field, `add_relayed_client`, auto-detecting `add_server` | `paddington/src/config.rs` |
| Invite `proxy` field | `paddington/src/invite.rs` |
| Real-agent relay wiring (`run_with_relay`) | `templemeads/src/handler.rs` |
| `client --add --proxy` CLI flag | `templemeads/src/agent_core.rs` |
| Relay fallback for ordinary sends, skip-dial for relayed servers | `paddington/src/exchange.rs`, `paddington/src/eventloop.rs` |
