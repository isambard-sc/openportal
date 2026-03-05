<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Wire Protocol Specification

This document specifies the wire protocol used for inter-agent communication in
OpenPortal. It describes the full protocol stack from the application-level
`Envelope` and `Command` objects through to the encrypted bytes sent over the
network.

The stack has four layers:

```
┌──────────────────────────────────────────────┐
│  Templemeads: Envelope + Command              │  application layer
├──────────────────────────────────────────────┤
│  Paddington: Message                          │  framing layer
├──────────────────────────────────────────────┤
│  Paddington: Encryption                       │  confidentiality layer
├──────────────────────────────────────────────┤
│  WebSocket / TLS                              │  transport layer
└──────────────────────────────────────────────┘
```

---

## 1. Templemeads Application Layer

### 1.1 `Envelope`

The `Envelope` is the top-level application object. It wraps a `Job` (defined in
[json-types.md](json-types.md)) with routing metadata and is the value that
agents hand to the Paddington layer for delivery.

**Source file:** `templemeads/src/job.rs`

```json
{
  "recipient": "<destination-string>",
  "sender":    "<destination-string>",
  "zone":      "<zone-string>",
  "job":       { <Job object> }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `recipient` | string | Dot-delimited agent path of the intended recipient (see [instruction-protocol.md](instruction-protocol.md) §Destinations) |
| `sender` | string | Dot-delimited agent path of the originating agent |
| `zone` | string | Shared zone name; both parties must agree on the zone to accept a message |
| `job` | `Job` | The job being transmitted (see [json-types.md](json-types.md) §Job) |

The `Envelope` is serialised to JSON and placed in a Templemeads `Command`
(`Put`, `Update`, or `Delete`) before being handed to the Paddington layer.

---

### 1.2 Templemeads `Command`

The Templemeads `Command` enum is the JSON payload carried in every regular
Paddington message. It encodes agent-level operations on the distributed job
board.

**Source file:** `templemeads/src/command.rs`

All variants are serialised with a `"type"` discriminant field (serde
`tag = "type"`). The JSON schemas for each variant follow.

#### `Put`

Submit a new `Job` to a remote agent's job board.

```json
{
  "type": "Put",
  "job":  { <Envelope> }
}
```

#### `Update`

Update the state of an existing `Job` on a remote agent's job board.

```json
{
  "type": "Update",
  "job":  { <Envelope> }
}
```

#### `Delete`

Remove a `Job` from a remote agent's job board.

```json
{
  "type": "Delete",
  "job":  { <Envelope> }
}
```

#### `Register`

Sent immediately after a connection is established. Announces the agent's
identity, engine name, and protocol version.

```json
{
  "type":    "Register",
  "agent":   "<agent-name-string>",
  "engine":  "<engine-name-string>",
  "version": <integer>
}
```

#### `Sync`

Synchronise the current state of the sender's job board with the recipient.
`state` is an opaque JSON value (typically an array of `Envelope` objects)
representing all live jobs.

```json
{
  "type":  "Sync",
  "state": <json-value>
}
```

#### `HealthCheck`

Initiates a health-check sweep across the agent graph. `visited` accumulates
the list of agent names that have already responded, preventing cycles.

```json
{
  "type":    "HealthCheck",
  "visited": ["<agent-name>", ...]
}
```

#### `HealthResponse`

Reply to a `HealthCheck`. `health` is a `HealthInfo` object describing the
responding agent's status and the status of its direct dependencies.

```json
{
  "type":   "HealthResponse",
  "health": { <HealthInfo> }
}
```

#### `Restart`

Request that an agent (or the whole sub-graph below a given destination) restart.

```json
{
  "type":         "Restart",
  "restart_type": "<restart-type-string>",
  "destination":  "<destination-string>"
}
```

#### `DiagnosticsRequest`

Request a diagnostic report from the agent identified by `destination`.

```json
{
  "type":        "DiagnosticsRequest",
  "destination": "<destination-string>"
}
```

#### `DiagnosticsResponse`

Reply to a `DiagnosticsRequest`. `report` is a free-form JSON object.

```json
{
  "type":   "DiagnosticsResponse",
  "report": { <report-object> }
}
```

---

## 2. Paddington Framing Layer

### 2.1 `Message`

`Message` is the framing object used by Paddington. Every value that passes
over the wire is a `Message` (after encryption is removed).

**Source file:** `paddington/src/message.rs`

```json
{
  "sender":    "<string>",
  "recipient": "<string>",
  "zone":      "<string>",
  "payload":   "<string>"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `sender` | string | Name of the sending agent, or `""` for control messages |
| `recipient` | string | Name of the intended recipient agent |
| `zone` | string | Shared zone name, or `""` for control messages |
| `payload` | string | Message body (see below) |

There are three message types, distinguished by field values:

| Type | `sender` | `zone` | `payload` |
|------|----------|--------|-----------|
| **Control** | `""` | `""` | JSON-encoded Paddington `Command` |
| **Keepalive** | (any) | (any) | `"KEEPALIVE"` |
| **Regular** | `"<name>"` | `"<zone>"` | JSON-encoded Templemeads `Command` |

---

### 2.2 Paddington `Command` (control messages)

When `sender` and `zone` are both `""`, the `payload` is a JSON-encoded
Paddington `Command`. These are used for connection lifecycle management.

**Source file:** `paddington/src/command.rs`

All variants use a `"type"` discriminant field.

#### `Error`

Reports an error to the remote peer.

```json
{
  "type":  "Error",
  "error": "<error-message-string>"
}
```

#### `Connected`

Confirms that a connection has been accepted by the remote service. Carries
the remote agent's identity.

```json
{
  "type":    "Connected",
  "agent":   "<agent-name-string>",
  "zone":    "<zone-string>",
  "engine":  "<engine-name-string>",
  "version": <integer>
}
```

#### `Watchdog`

Periodic keepalive probe. The receiving agent must respond to show it is alive.

```json
{
  "type":  "Watchdog",
  "agent": "<agent-name-string>",
  "zone":  "<zone-string>"
}
```

#### `Disconnect`

Polite disconnect request; the sender intends to close the connection.

```json
{
  "type":  "Disconnect",
  "agent": "<agent-name-string>",
  "zone":  "<zone-string>"
}
```

#### `Disconnected`

Acknowledgement that the peer has disconnected.

```json
{
  "type":  "Disconnected",
  "agent": "<agent-name-string>",
  "zone":  "<zone-string>"
}
```

---

## 3. Paddington Encryption Layer

### 3.1 Key Material

Each peer-pair shares two 32-byte pre-shared keys stored in their configuration:

| Key | Purpose |
|-----|---------|
| `inner_key` | Encrypts the inner (message content) envelope |
| `outer_key` | Encrypts the outer (routing) envelope |

Per-connection session keys are derived from these pre-shared keys during the
handshake (see §4).

**Source file:** `paddington/src/crypto.rs`

### 3.2 AEAD Cipher

Encryption uses **XChaCha20-Poly1305** via the `orion` crate. This provides
authenticated encryption with associated data (AEAD). All encrypted values are
hex-encoded.

### 3.3 Key Derivation

Session sub-keys are derived from a base key using **HKDF-SHA512**:

```
derived_key = HKDF-SHA512(ikm=base_key, salt=salt, info=info)
```

The `info` value is a 32-byte context string that binds the derived key to a
specific message. Both sender and receiver independently derive the same key
using the same `salt` and `info`, so the `info` values are transmitted
alongside the ciphertext (see §3.4).

### 3.4 Wire Frame Format

Each encrypted frame is a flat string concatenation:

```
<inner_info_hex><outer_info_hex><ciphertext>
```

| Component | Length | Description |
|-----------|--------|-------------|
| `inner_info_hex` | 64 hex chars (32 bytes) | HKDF `info` used to derive the inner key |
| `outer_info_hex` | 64 hex chars (32 bytes) | HKDF `info` used to derive the outer key |
| `ciphertext` | variable | `outer_key_derived.encrypt(inner_key_derived.encrypt(json(Message)))` |

**Encryption procedure:**

1. Serialise the `Message` to JSON.
2. Choose a random 32-byte `inner_info`.
3. Choose a random 32-byte `outer_info`.
4. Derive `inner_key_session = inner_key.derive(salt=session_inner_salt, info=inner_info)`.
5. Derive `outer_key_session = outer_key.derive(salt=session_outer_salt, info=outer_info)`.
6. `inner_ciphertext = inner_key_session.encrypt(json_bytes)`.
7. `outer_ciphertext = outer_key_session.encrypt(inner_ciphertext)`.
8. Transmit `hex(inner_info) + hex(outer_info) + outer_ciphertext`.

**Decryption procedure:**

1. Read the first 64 hex chars → `inner_info` (32 bytes).
2. Read the next 64 hex chars → `outer_info` (32 bytes).
3. The remainder is `outer_ciphertext`.
4. Derive `outer_key_session` from `outer_key`, `session_outer_salt`, `outer_info`.
5. `inner_ciphertext = outer_key_session.decrypt(outer_ciphertext)`.
6. Derive `inner_key_session` from `inner_key`, `session_inner_salt`, `inner_info`.
7. `json_bytes = inner_key_session.decrypt(inner_ciphertext)`.
8. Deserialise `Message` from `json_bytes`.

**Source file:** `paddington/src/connection.rs` (`envelope_message` /
`deenvelope_message`)

---

## 4. Connection Handshake

Connections are established as WebSocket upgrades over HTTP/TLS. The handshake
proceeds in three phases.

### 4.1 Salt Exchange (HTTP headers)

When the client initiates the WebSocket upgrade, two per-connection 32-byte
salts are exchanged via HTTP headers:

| Header | Direction | Value |
|--------|-----------|-------|
| `openportal-inner-salt` | client → server | `hex(client_inner_salt XOR pre_shared_inner_key)` |
| `openportal-outer-salt` | client → server | `hex(client_outer_salt XOR pre_shared_outer_key)` |

The server XORs the received values with the same pre-shared keys to recover
`client_inner_salt` and `client_outer_salt`. This prevents an observer without
the pre-shared key from learning the session salts.

### 4.2 Session Key Negotiation

After the WebSocket connection is established, the client sends a `Handshake`
object (encrypted using the pre-shared keys with the exchanged salts):

```json
{
  "session_key": "<hex-encoded-32-byte-key>",
  "engine":      "<engine-name-string>",
  "version":     <integer>
}
```

The server responds with a new session inner key (replacing the pre-shared
inner key for the remainder of the connection):

```json
{
  "session_key": "<hex-encoded-32-byte-key>"
}
```

After the key exchange, both parties use the negotiated session keys for all
subsequent messages on this connection.

### 4.3 Peer Identity Exchange

After key negotiation, both sides exchange `PeerDetails` objects (as regular
encrypted messages):

```json
{
  "name":    "<agent-name-string>",
  "zone":    "<zone-string>",
  "version": 2,
  "standby_status": {
    "server_is_secondary": <boolean>,
    "client_is_secondary": <boolean>
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Agent name as registered in configuration |
| `zone` | string | Zone this connection belongs to |
| `version` | integer | Protocol version; must be `2` |
| `standby_status` | object | High-availability standby state (see §4.4) |

Once both `PeerDetails` have been exchanged successfully, the Templemeads layer
is notified via a Paddington `Connected` control command and the `Register` /
`Sync` sequence begins.

### 4.4 High-Availability Standby

OpenPortal supports active/standby pairs for each agent. When two peers
connect, the peer whose name is alphabetically earlier is designated the
primary; the other is the secondary. The `standby_status` field in
`PeerDetails` communicates which role each side occupies:

| Field | Meaning |
|-------|---------|
| `server_is_secondary` | `true` if the server-side peer is in standby mode |
| `client_is_secondary` | `true` if the client-side peer is in standby mode |

Standby peers receive job-board synchronisation but do not actively process
jobs unless the primary becomes unavailable.

---

## 5. Post-Handshake Message Flow

Once the handshake completes, the following sequence occurs:

1. **`Register`** — the newly-connected Templemeads agent sends a `Register`
   command identifying itself.
2. **`Sync`** — the agent sends a `Sync` command containing its current job
   board state, so the remote side can reconcile any jobs that may have been
   in-flight when a previous connection dropped.
3. **Normal operation** — agents exchange `Put`, `Update`, and `Delete` commands
   as jobs are created, progress, and complete.
4. **Keepalives** — periodic `KEEPALIVE` messages (and Paddington `Watchdog`
   control messages) maintain the connection and detect failures.

---

## 6. Protocol Version

The current wire protocol version is **2**, carried in both the `Handshake` and
`PeerDetails` objects. Version negotiation is not currently implemented; a
version mismatch causes the connection to be refused.

---

## 7. Source File Reference

| Concept | Source file |
|---------|-------------|
| `Envelope`, `Job`, `Status` | `templemeads/src/job.rs` |
| Templemeads `Command` | `templemeads/src/command.rs` |
| Paddington `Message` | `paddington/src/message.rs` |
| Paddington `Command` | `paddington/src/command.rs` |
| `Key`, `Salt`, encryption | `paddington/src/crypto.rs` |
| Wire framing, handshake | `paddington/src/connection.rs` |
| Post-connect control flow | `templemeads/src/control_message.rs` |
| Message dispatch | `templemeads/src/handler.rs` |
| Agent type definitions | `templemeads/src/agent.rs` |
