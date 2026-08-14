<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Wire Protocol Specification

This document specifies the wire protocol used for inter-agent communication in
OpenPortal. It describes the full protocol stack from the application-level
`Envelope` and `Command` objects through to the encrypted bytes sent over the
network.

Everything at the Templemeads layer - `Envelope<L>`, `Command<L>`,
`Notification<L>` - is generic over a `Domain` (see
[writing-a-domain.md](writing-a-domain.md)) and applies unchanged to any
`Domain`; only the `job` payload the `Envelope` carries and the `notification`
payload of a `Notify` command vary by `Domain`. This document uses `L`
freely and calls out `greatwestern` (`L = Hpc`, the reference `Domain` every
built-in OpenPortal agent uses) only where the wire format itself differs.

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

`Envelope<L>` is the top-level application object. It wraps a `Job<L>` (defined
in [json-types.md](json-types.md)) with routing metadata and is the value that
agents hand to the Paddington layer for delivery. `L` is the agent's `Domain`
- `Hpc` (`greatwestern`) for every built-in OpenPortal agent - and only
affects the shape of the nested `job` object below, not this wrapper.

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

The Templemeads `Command<L>` enum is the JSON payload carried in every regular
Paddington message. It encodes agent-level operations on the distributed job
board. As with `Envelope`, `L` is the agent's `Domain` and only affects the
nested `Envelope<L>`/`Notification<L>` payloads carried by some variants, not
the variant structure itself.

**Source file:** `templemeads/src/command.rs`

Each variant's fields are listed below. On the wire, serde's default
externally-tagged enum representation wraps them as `{"<Variant>": {
...fields... }}` (e.g. `{"Put": {"job": {...}}}`) - the `"type": "..."` shown
in each block below is illustrative of the fields present, not a literal
top-level key.

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
identity, engine name and version, and (since templemeads 0.33.0) the
`Domain` it is compiled against and that `Domain`'s version.

```json
{
  "agent":          "<agent-type-string>",
  "engine":         "<engine-name-string>",
  "version":        "<engine-semver-string>",
  "domain":         "<domain-name-string>" | null,
  "domain_version": "<domain-semver-string>" | null,
  "supports_portal_routes": <boolean>
}
```

| Field | Type | Description |
|-------|------|-------------|
| `agent` | string | The sender's `agent::Type` (Portal, Provider, Instance, ...) |
| `engine` | string | Always `"templemeads"` today - `env!("CARGO_PKG_NAME")` resolved inside templemeads itself |
| `version` | string | The templemeads crate's own semver, e.g. `"0.33.0"` - **not** the wire protocol version (that's the unrelated integer `2` in `PeerDetails`, see §4.3) |
| `domain` | string or null | The sender's `Domain::name()`, e.g. `"greatwestern"`. `null` (and absent from the JSON entirely) from a peer running templemeads <= 0.32.2, from before this field existed |
| `domain_version` | string or null | The sender's `Domain::version()`, alongside `domain` |
| `supports_portal_routes` | boolean | Whether the sender understands `PortalRoutes` (§below). `#[serde(default)]`, so a peer that predates the field reads as `false`. Used in both directions: routes are not pushed to a peer that would not understand them, and a route is not enforced against a peer that could never have sent one. See [portal-route-discovery-design.md](../plans/portal-route-discovery-design.md) §7 |

#### `PortalRoutes`

Advertises (or retracts) the routes by which portals reach the sender. Pushed
*downstream* - away from portals - on connection, and again whenever the
sender's own table changes.

```json
{
  "routes": [
    { "portal": "<portal-name>", "route": "<portal>.<hop>.<...>.<sender>" }
  ],
  "withdrawn": ["<portal-name>"]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `routes` | array | Each entry is a portal and the route by which it reaches **the sender**. Every route therefore ends with the sender's own name, which is what the receiver checks before accepting it |
| `withdrawn` | array | Portals whose routes the sender is retracting, by name. `#[serde(default)]` |

The receiver appends its own name to each accepted route to obtain its own, and
propagates that onward to every peer except the one it learned from and any peer
declared `type = "portal"`. Two different routes for the same portal name is a
**collision** - impossible in a single-path topology, and therefore the signature
of an agent whose config has been changed - after which that portal name is
refused until an operator intervenes.

Instructions naming a portal are then checked against the stored route by prefix
match, which is strictly stronger than comparing only the first agent of the
destination. Never sent to a peer that did not advertise
`supports_portal_routes`. See
[portal-route-discovery-design.md](../plans/portal-route-discovery-design.md) and
[security-review-2.md](security-review-2.md) §4.1.

**Backwards compatibility.** `domain`/`domain_version` are `#[serde(default)]`,
so a `Register` from a pre-0.33.0 peer (which simply doesn't have these keys
in its JSON at all) still deserialises, with both fields `None`. The
receiving agent then asks its own `Domain` to resolve a legacy assumption via
`Domain::assume_legacy_domain_version(engine_version)` - `greatwestern`
resolves any `version <= "0.32.2"` to `domain = "greatwestern"`,
`domain_version = "0.32.2"`, since templemeads never had a separable domain
before that release, so any peer at or below it was unambiguously speaking
today's `greatwestern` vocabulary at that same version. A `Domain` with no
such historical claim simply returns `None`, leaving the peer's domain
genuinely unknown. See [writing-a-domain.md](writing-a-domain.md#1-the-domain-trait) for the
trait methods involved.

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

#### `Notify`

Carries a fire-and-forget `Notification` — a one-way event signal routed along
a destination path. Unlike `Put`/`Update`, no acknowledgement or result is ever
sent back. The notification is **not** stored on any job board.

```json
{
  "type":         "Notify",
  "notification": { <Notification> }
}
```

See [notification-protocol.md](notification-protocol.md) for the full
specification of the `Notification` object and `NotificationEvent` grammar.

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

As with the Templemeads `Command` above, these are serialised with serde's
default externally-tagged representation (`{"<Variant>": {...fields...}}`),
not a literal `"type"` key - the blocks below list fields only.

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

### 3.5 Ongoing Traffic Payload: `NoncedPayload`

Once the handshake completes, what actually gets serialised as `T` in the
wire frame above for ordinary application traffic (§5) is not a bare
payload string but a small wrapper carrying a replay-protection nonce (see
[security-model.md](security-model.md) §9 for why):

```json
{
  "nonce":   <integer>,
  "payload": "<payload-string>"
}
```

`nonce` is a per-sender, monotonically increasing counter starting at 0
for the first message on a fresh connection/session, checked by the
receiver against a sliding anti-replay window
([security-model.md](security-model.md) §9) before the payload is passed
on to templemeads. Applies uniformly to direct connections and relayed
sessions (§7) - the same wrapper, checked at `Connection::send_message`/
the post-handshake receive loops, and at `relay::send`/
`handle_incoming_envelope`'s ongoing-traffic branch respectively - and to
every kind of ongoing message alike (`Register`/`Sync`/`Put`/`Update`/
`Delete`/`Notify`/keepalives), since all of them are just the `payload`
string as far as this layer is concerned.

This wrapper does not apply to the handshake-phase `Handshake`/
`PeerDetails` messages (§4) or to relayed bootstrap messages (§7.1/§7.3) -
nonce protection for those is deferred (see
[replay-protection-design.md](../plans/replay-protection-design.md) §2,
§9).

A payload deserialising as a bare JSON string rather than this object
shape (i.e. no `nonce` field at all) is accepted without a nonce check.
Since the design was revised to support a gradual rollout (design doc §5),
this bare-string shape **is** now a compatibility path a sender can
deliberately opt into: a sender only emits the `{nonce, payload}` object
to a specific peer once that peer's `PeerDetails` (§4.3) or relayed
bootstrap message (§7.1) has confirmed `supports_nonce: true`; otherwise it
sends the payload as a bare string, byte-identical to what a peer that
predates this whole feature already expects. So a bare string on the wire
means one of two things indistinguishable to the receiver - and
indistinguishable deliberately, since either way the correct handling is
identical: either the sender hasn't confirmed the receiver supports
nonces, or the sender is itself not yet upgraded and has no nonce concept
at all.

**Source file:** `paddington/src/anti_replay.rs` (`NoncedPayload`)

---

## 4. Connection Handshake

Connections are established as WebSocket upgrades over HTTP/TLS. The handshake
proceeds in three phases.

### 4.1 Salt Exchange (HTTP headers)

When the client initiates the WebSocket upgrade, two per-connection 32-byte
salts are exchanged via HTTP headers, along with a marker declaring their
encoding:

| Header | Direction | Value |
|--------|-----------|-------|
| `openportal-salt-format` | client → server | `"plain"` (current clients) or absent (legacy clients) |
| `openportal-inner-salt` | client → server | `hex(client_inner_salt)` (plain) or `hex(client_inner_salt XOR pre_shared_inner_key)` (legacy) |
| `openportal-outer-salt` | client → server | `hex(client_outer_salt)` (plain) or `hex(client_outer_salt XOR pre_shared_outer_key)` (legacy) |

HKDF salts are **public by design**, so current clients send them in the clear
and set `openportal-salt-format: plain`. Message security does not rely on salt
secrecy — a fresh random per-message `info` is mixed into every derivation
regardless (§3.3).

For backward compatibility, a client that omits `openportal-salt-format` is
treated as legacy: it XOR-masks each salt with the corresponding pre-shared key,
and the server un-XORs to recover the real salt. The server detects the format
per connection from the header, so an upgraded server interoperates with both
old and new clients. Because the client commits to an encoding in this first
message (before any negotiation is possible), servers must be upgraded before
clients — see [security-review.md](security-review.md) F15. (The XOR-masking was
never load-bearing: the masked value on the wire is `salt XOR key`, and the real
salt is never transmitted, so an observer cannot recover the key from it.)

### 4.2 Session Key Negotiation

After the WebSocket connection is established, the client sends a `Handshake`
object (encrypted using the pre-shared keys with the exchanged salts):

```json
{
  "session_key": "<hex-encoded-32-byte-key>",
  "engine":      "<engine-name-string>",
  "version":     "<engine-version-string>",
  "nonce":       <integer>,
  "epoch":       <integer>
}
```

The server responds with a new session inner key (replacing the pre-shared
inner key for the remainder of the connection):

```json
{
  "session_key": "<hex-encoded-32-byte-key>",
  "engine":      "<engine-name-string>",
  "version":     "<engine-version-string>",
  "nonce":       <integer>,
  "epoch":       <integer>
}
```

After the key exchange, both parties use the negotiated session keys for all
subsequent messages on this connection.

`nonce` is a replay-protection nonce, checked against a per-peer window
that - unlike §3.5's ongoing-traffic window - persists across reconnects
rather than resetting, since `Handshake` is encrypted (at least partly)
under the *permanent* pre-shared key pair, which itself never changes
across reconnects. `#[serde(default)] Option<u64>`, so a pre-upgrade
peer's `Handshake` (which predates this field) is read as `nonce: None` -
skip the check, accept unconditionally. See
[security-model.md](security-model.md) §9 and
[replay-protection-design.md](../plans/replay-protection-design.md) §10.

`epoch` identifies the *sending process incarnation*: a random 64-bit value
generated once per process start. The nonce counter lives only in memory
(agents deliberately keep no on-disk state), so it returns to zero when a
process restarts - and a receiver holding a single persistent window would
reject every reconnect as a replay until the counter climbed back past the old
high-water mark. The receiver therefore keeps **one window per epoch**, bounded
and evicted least-recently-used: a new epoch gets a fresh window (so a restart
reconnects immediately), while the superseded epoch's window is *retained* (so
a captured message replayed later is still rejected). Keeping the old window
rather than clearing it is what makes this no weaker than a single window, and
per-epoch separation is also what lets client HA work, since several processes
legitimately share one peer identity and each contributes an epoch
([highavailability.md](highavailability.md) §2). Also
`#[serde(default)] Option<u64>`, so a pre-epoch peer reads as `epoch: None`
and gets its own window, behaving exactly as before. See
[security-review-2.md](security-review-2.md) (finding R10).

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
  },
  "supports_nonce": <boolean>,
  "nonce": <integer>,
  "epoch": <integer>
}
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Agent name as registered in configuration |
| `zone` | string | Zone this connection belongs to |
| `version` | integer | Protocol version; must be `2` |
| `standby_status` | object | High-availability standby state (see §4.4) |
| `supports_nonce` | boolean | Whether the sender understands the `{nonce, payload}` shape for ongoing traffic (§3.5) - `#[serde(default)]`, so a pre-upgrade peer's `PeerDetails` (which predates this field) is read as `false`. See [security-model.md](security-model.md) §9 and [replay-protection-design.md](../plans/replay-protection-design.md) §5. |
| `nonce` | integer, optional | Replay-protection nonce for this `PeerDetails` message itself (distinct from `supports_nonce` above, which is about *ongoing* traffic) - `#[serde(default)] Option<u64>`, checked against the same persistent per-peer window `Handshake`'s nonce uses (§4.2). See [replay-protection-design.md](../plans/replay-protection-design.md) §10. |
| `epoch` | integer, optional | Sending process incarnation, so the receiver can tell a restart from a replay - see §4.2. `#[serde(default)] Option<u64>`. See [security-review-2.md](security-review-2.md) (finding R10). |

Once both `PeerDetails` have been exchanged successfully, the Templemeads layer
is notified via a Paddington `Connected` control command and the `Register` /
`Sync` sequence begins. Each side now remembers whether the *other* side's
`PeerDetails` confirmed `supports_nonce` for the lifetime of this connection
(`Connection::peer_supports_nonce`) - this is what §3.5's send path checks
before choosing whether to wrap outgoing traffic. Unlike that per-connection
capability flag, the `Handshake`/`PeerDetails` nonce window itself
(`HANDSHAKE_NONCE_STATE`, keyed by `"{name}@{zone}"`) persists for the life
of the process, not just this one connection - see §10.

### 4.4 High-Availability Standby

OpenPortal supports active/standby pairs of redundant clients connecting to
one server: whichever physical connection under a given identity is
registered *first* is primary, and any further connection under that same
identity is told it's secondary. The `standby_status` field in
`PeerDetails` communicates which role each side occupies:

| Field | Meaning |
|-------|---------|
| `server_is_secondary` | `true` if the server-side peer is in standby mode (currently unused - see [highavailability.md](highavailability.md) §2.3) |
| `client_is_secondary` | `true` if the client-side peer is in standby mode |

Standby peers receive job-board synchronisation but do not actively process
jobs unless the primary becomes unavailable. See
[highavailability.md](highavailability.md) for the full mechanism,
failover timing, and how the same building blocks give server-side
redundancy too via the blind relay proxy.

---

## 5. Post-Handshake Message Flow

Once the handshake completes, the following sequence occurs:

1. **`Register`** — the newly-connected Templemeads agent sends a `Register`
   command identifying itself.
2. **`Sync`** — the agent sends a `Sync` command containing its current job
   board state, so the remote side can reconcile any jobs that may have been
   in-flight when a previous connection dropped.
3. **Normal operation** — agents exchange `Put`, `Update`, and `Delete` commands
   as jobs are created, progress, and complete. `Notify` commands may also be
   sent at any time during normal operation.
4. **Keepalives** — periodic `KEEPALIVE` messages (and Paddington `Watchdog`
   control messages) maintain the connection and detect failures.

---

## 6. Protocol Version

The current wire protocol version is **2**, carried in both the `Handshake` and
`PeerDetails` objects. Version negotiation is not currently implemented; a
version mismatch causes the connection to be refused.

---

## 7. Blind Relay Protocol (`op-proxy`)

Two agents that can each only make outbound connections (neither can open a
port the other can reach) can still talk to each other via an `op-proxy`
agent that both connect to as ordinary paddington clients. See
[blind-relay-proxy-design.md](../plans/archive/blind-relay-proxy-design.md) for the
full design and rationale; this section covers only the wire format.

The proxy relays a single, opaque payload type - `RelayEnvelope` - between
the two real hops without ever needing to understand what is inside it:

```json
{
  "from":       "<relayed-peer-name>",
  "to":         "<relayed-peer-name>",
  "zone":       "<zone-string>",
  "ciphertext": "<opaque-string>"
}
```

`RelayEnvelope` is sent as the `payload` of an ordinary `Message` (§2.1) on
each agent's real, direct connection to the proxy - it is not a new
paddington `Command` variant. `from`/`to` name the two relayed peers, never
the proxy itself; the proxy's `proxy_handler` reads only `from`/`to`/`zone`
to enforce its `RelayPolicy` (see [security-model.md](security-model.md)
§7.1) and forwards `ciphertext` unmodified.

`zone` here is the zone of the *relayed relationship itself* (e.g. the
zone `brics` was introduced to `airr` under), carried end-to-end and used
for the synthesised `Message` on arrival (§7.2) - it is **not** the same
thing as the zone of the real, direct `Message` this `RelayEnvelope` is
wrapped in when sent to the proxy, which is whichever zone each side's own
`servers`/`clients` entry for the proxy itself uses (very often, but not
necessarily, the same zone). Getting this distinction wrong means
addressing a real paddington `Message` with a zone the recipient's
connection registry has no entry for - see
`paddington::relay::RelayedPeer`'s `zone` vs `relay_zone` fields.

### 7.1 Bootstrap: `StartRelayedConnection` / `RelayedConnectionAccepted`

Before any real traffic flows, the relayed *client* (the side holding a
`servers` entry with a `proxy` set) initiates a bootstrap with the relayed
*server* (the side holding a `clients` entry with a `proxy` set), mirroring
§4.2's mutual session-key contribution but carried inside `ciphertext`
rather than at the transport level:

```json
// client → server, via proxy (ciphertext, once decrypted)
{
  "type": "Start",
  "session_outer_key": "<hex-encoded-32-byte-key>",
  "inner_key_salt":     "<hex-encoded-32-byte-salt>",
  "outer_key_salt":     "<hex-encoded-32-byte-salt>",
  "magic":              "<hex-encoded-32-byte-random-string>",
  "engine":             "<engine-name-string>",
  "version":            "<engine-version-string>",
  "supports_nonce":     <boolean>,
  "nonce":              <integer>
}
```

```json
// server → client, via proxy (ciphertext, once decrypted)
{
  "type": "Accepted",
  "session_inner_key": "<hex-encoded-32-byte-key>",
  "magic":             "<same-magic-as-Start>",
  "engine":             "<engine-name-string>",
  "version":            "<engine-version-string>",
  "supports_nonce":     <boolean>,
  "nonce":              <integer>
}
```

`supports_nonce` is the relayed-bootstrap equivalent of `PeerDetails`'
field of the same name (§4.3): whether the sender understands the
`{nonce, payload}` shape for ongoing traffic (§3.5/§7.2) over the session
this bootstrap establishes. `#[serde(default)]`, so a pre-upgrade peer's
message (which predates this field) is read as `false`. Each side learns
the other's confirmed support once, at the point its own bootstrap
function (`bootstrap()` for the client, `handle_start()` for the server)
completes, and stores it as `RelayedSession::peer_supports_nonce` for that
session's lifetime - see
[replay-protection-design.md](../plans/replay-protection-design.md) §5.

`nonce` is a *separate* mechanism from `supports_nonce` above - a
replay-protection nonce for the bootstrap message itself, checked against
a per-peer window that persists across every bootstrap attempt for that
peer (`BOOTSTRAP_NONCE_STATE`, keyed by peer name), unlike
`RelayedSession`'s ongoing-traffic nonce state, which resets on every
fresh bootstrap. Unlike `supports_nonce`/§3.5's ongoing-traffic nonce, this
field is a plain required `u64`, not negotiated - `op-proxy` isn't deployed
yet, so there is no not-yet-upgraded relayed peer to stay compatible with.
`handle_start()` checks an incoming `Start`'s nonce *before* generating any
session key material or touching the session table, since this message
alone is otherwise sufficient to reset a peer's live session - see
[replay-protection-design.md](../plans/replay-protection-design.md) §10.1
for why this (and `SessionUnknown`, §7.3) are the bootstrap messages where
this actually matters, versus `Accepted`, whose replay is already prevented
by `magic`'s single-use correlation below.

Both messages are internally tagged (`type: "Start"` / `"Accepted"`) so
that a successful decryption can be identified as one specific bootstrap
message unambiguously. Both are encrypted with the **permanent pre-shared
key pair** the two relayed peers exchanged out-of-band (never seen by the
proxy) - the same double-envelope scheme as §3.4, but using a fixed,
non-secret salt (there is no live connection to derive a per-connection
salt from; safety comes from the random per-message `info` value §3.3
always mixed in regardless of salt, not from the salt itself).

The client contributes `session_outer_key` and both salts; the server
contributes `session_inner_key`. Neither side alone determines the
resulting session key pair - this is what gives each relayed session a
**fresh** key pair, exactly as the real handshake's `Handshake` message
does in §4.2. Note this is per-session key freshness, **not** forward
secrecy: both halves are key-transported under the permanent pre-shared
keys, not agreed in-band (there is deliberately no Diffie-Hellman), so it
does not protect past traffic against later compromise of the permanent
keys - see [security-model.md](security-model.md) §2.5. `magic` correlates
the `Accepted` response with its `Start`; a
response with unrecognised `magic` is dropped (stale or forged, not a
protocol error).

Once bootstrapped, both sides hold an identical
`{inner_key: session_inner_key, outer_key: session_outer_key,
inner_key_salt, outer_key_salt}` tuple - the relayed equivalent of a
direct connection's negotiated session state - and each independently
synthesises the same `Connected` control event (§4.3) that a direct
connection would produce, so the higher `templemeads` layers cannot
distinguish a relayed connection from a direct one.

### 7.2 Ongoing Traffic

Once a session exists, every subsequent message between the pair is a
`RelayEnvelope` whose `ciphertext` is the real payload string encrypted
with the negotiated **session** key pair (§7.1) and per-connection-style
salts, using the same `envelope_message`/`deenvelope_message` procedure as
§3.4. The receiving side first attempts to decrypt an incoming
`RelayEnvelope`'s `ciphertext` with the permanent pre-shared key (to
recognise a `BootstrapMessage`); if that fails, it falls through to the
established session keys for `envelope.from`. Neither attempt ever
succeeds with the other key pair, so a proxy - or anyone else without
either key pair - cannot distinguish bootstrap traffic from ongoing
traffic, let alone read either.

### 7.3 Recovery: `SessionUnknown`

If a relayed peer's process restarts, it loses its in-memory session
state (there is nothing else to lose it from - sessions are never
persisted to disk). A restarted relayed *client* self-heals: its own
startup path re-bootstraps every relayed peer it initiates towards,
unprompted. A restarted relayed *server* has no equivalent - it only ever
waits - so without something else, its peer's still-cached session would
silently fail to decrypt on every send, forever, with neither side finding
out why.

```json
// either direction, via proxy (ciphertext, once decrypted)
{
  "type": "SessionUnknown",
  "nonce": <integer>
}
```

Whichever side receives ongoing traffic (§7.2) it cannot match to a
session sends this back to `envelope.from`, encrypted with the same
**permanent pre-shared key** as `Start`/`Accepted` above (so the proxy
cannot forge it any more than it can forge a genuine bootstrap). On
receipt, the recipient checks `nonce` against the same persistent
per-peer window `Start`/`Accepted` use (§7.1) before acting on it - this
is one of the two bootstrap messages (with `Start`) where nonce-checking
closes a real, repeatable disruption: without it, a single captured
`SessionUnknown` could be replayed indefinitely to force constant
re-bootstrap churn between two peers, long after whatever restart
originally produced it. If the nonce checks out, the recipient clears its
own cached session for that peer and, if it holds the relayed *client*
role for it (the only role that can initiate), immediately re-bootstraps
rather than waiting for its next scheduled retry. See
[replay-protection-design.md](../plans/replay-protection-design.md) §10.1.

**Source file:** `paddington/src/relay.rs`

---

## 8. Source File Reference

| Concept | Source file |
|---------|-------------|
| `Envelope<L>`, `Job<L>`, `Status` (generic, domain-agnostic) | `templemeads/src/job.rs` |
| Templemeads `Command<L>` | `templemeads/src/command.rs` |
| `Domain` trait, including `name()`/`version()`/`assume_legacy_domain_version()` | `templemeads/src/domain.rs` |
| `agent::peer_domain()`, `agent::ensure_domain_matches()` (connection-level, opt-in disconnect-on-mismatch; always accepts a known `Erased` peer) | `templemeads/src/agent.rs` |
| `agent::ensure_job_domain_matches()`, `agent::ensure_notification_domain_matches()` (per-message, opt-in - see [writing-a-domain.md](writing-a-domain.md#1-the-domain-trait)) | `templemeads/src/agent.rs` |
| `templemeads::erased::Erased` (domain-oblivious `Domain` for routing-only agents) | `templemeads/src/erased.rs` |
| `Notification<L>`, `NotificationEnvelope<L>` (generic) | `templemeads/src/notification.rs` |
| `NotificationEvent` (`greatwestern`'s concrete vocabulary) | `greatwestern/src/notification.rs` |
| `Instruction` (`greatwestern`'s concrete vocabulary, i.e. `Job<Hpc>`'s payload) | `greatwestern/src/grammar.rs` |
| Paddington `Message` | `paddington/src/message.rs` |
| Paddington `Command` | `paddington/src/command.rs` |
| `Key`, `Salt`, encryption | `paddington/src/crypto.rs` |
| Wire framing, handshake | `paddington/src/connection.rs` |
| Post-connect control flow | `templemeads/src/control_message.rs` |
| Message dispatch | `templemeads/src/handler.rs` |
| Agent type definitions | `templemeads/src/agent.rs` |
| Blind relay protocol (`RelayEnvelope`, bootstrap, `RelayPolicy`) | `paddington/src/relay.rs` |
| Anti-replay window, `NoncedPayload` | `paddington/src/anti_replay.rs` |
