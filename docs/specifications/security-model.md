<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Security Model

This document describes the security model of OpenPortal: the threat model it is
designed to address, how cryptographic keys are structured and provisioned, how
connections are authenticated, and how zone isolation limits the blast radius of
any compromise.

> For an independent, code-level *evaluation* of this model — graded findings,
> known gaps, and residual risks — see the
> [security review](security-review.md). This document describes how the model
> is intended to work; the review assesses how strong it actually is.

---

## 1. Design Goals

OpenPortal is built around one central principle: **no agent should hold more
privilege than it needs**. In traditional infrastructure management, a portal
system is given a single privileged "god key" that can create accounts, manage
storage, and control every system it touches. Compromise of that credential
compromises everything.

OpenPortal instead uses a peer-to-peer agent hierarchy where:

- Each link in the hierarchy has its own independent symmetric key pair.
- An agent can communicate only with its direct neighbours — it cannot speak to
  arbitrary agents.
- Compromise of one agent's credentials does not expose credentials for any
  other peer relationship.
- There is no central credential store. Keys live only in the configuration
  files of the two peers that share them.

---

## 2. Key Structure

### 2.1 Key Type

All cryptographic keys are **32-byte random symmetric keys** stored as
`SecretBox<Key>` (using the `secrecy` crate). The `SecretBox` wrapper:

- Zeroises key material on drop.
- Prevents keys from appearing in `Debug` output (printed as `[[REDACTED]]`).
- Allows controlled exposure via `.expose_secret()`.

Keys are serialised to/from TOML configuration files as hex-encoded strings.

### 2.2 Key Pairs

Every peer relationship uses **two independent keys**:

| Key | Role |
|-----|------|
| `inner_key` | Encrypts the message content (inner envelope) |
| `outer_key` | Encrypts the routing wrapper (outer envelope) |

The double-envelope encryption scheme (described in
[wire-protocol.md](wire-protocol.md) §3) uses both keys so that an observer who
somehow obtains one key still cannot read message content or routing metadata
without the other.

### 2.3 Key Generation

New keys are generated using `orion::aead::SecretKey::default()`, which calls
the operating system's cryptographically secure random number generator. Keys
are never derived deterministically except during password-based config
encryption (see §5).

### 2.4 Key Derivation for Wire Messages

Pre-shared keys are **never used directly** to encrypt wire messages. Before
each message, per-message session sub-keys are derived via **HKDF-SHA512**:

```
session_key = HKDF-SHA512(ikm=pre_shared_key, salt=session_salt, info=random_info)
```

A fresh random 32-byte `info` value is generated for each message, ensuring
that no two messages are encrypted with the same key even if the session salt is
reused. See [wire-protocol.md](wire-protocol.md) §3 for the full wire frame
format.

### 2.5 Session Keys — and the Deliberate Absence of Forward Secrecy

Every connection (and every relayed-session bootstrap) uses a **fresh, randomly
generated session key pair**. These session keys are **key-transported** to the
peer — that is, sent already encrypted under the two peers' long-term pre-shared
keys — rather than agreed via an in-band exchange such as Diffie-Hellman.

This is a deliberate design decision: **OpenPortal provides no in-band mechanism
for agents to share or change key material themselves.** All key material is
provisioned out-of-band (§3), and the only key ever placed on the wire is a
random session key sealed under the permanent pre-shared keys. Adding a
Diffie-Hellman exchange to obtain forward secrecy would reintroduce exactly the
in-band key-agreement path the design excludes.

The consequences, stated plainly so nothing here is mistaken for forward secrecy:

- **There is no forward secrecy.** An attacker who records a connection's traffic
  *and* later obtains the permanent pre-shared keys could recover that
  connection's session keys and decrypt the captured traffic. Fresh session keys
  per connection mean one session's keys do not expose another's, but they do not
  protect past traffic against later compromise of the *permanent* keys.
- **The permanent keys are hard to attack from the wire.** They are only ever
  used to encrypt the initial, randomly generated, **high-entropy** session keys.
  There is no low-entropy or known plaintext sealed under a permanent key to act
  as a crib, so an attacker observing the wire sees only high-entropy plaintext
  under a high-entropy key — no leverage for reverse-guessing the permanent key.
  "Obtaining the permanent keys" therefore means compromising a config or invite
  file out-of-band, not cracking traffic.
- **Security rests on the secrecy of the permanent keys**, which is why key
  rotation is a first-class, out-of-band operation (§3.3): rotating periodically
  bounds the traffic any single key pair ever covers.

See also the [security review](security-review.md) F14, which records this as an
accepted, deliberate trade-off.

---

## 3. Key Provisioning: the Invite Model

Keys are provisioned out-of-band using **invite files**. No key material is
ever transmitted over the network in cleartext.

### 3.1 Procedure

The two agents that want to communicate are called the **server** (the side that
listens for connections) and the **client** (the side that initiates them).

**Step 1 — Server generates the invite.**

An operator calls `add_client` on the server, providing the client's name and
expected IP address (or CIDR range):

```
server$ openportal-agent add-client --name client-agent --ip 10.0.0.5
```

The server:
1. Generates a fresh `inner_key` and `outer_key` (32 bytes each, random).
2. Stores a `ClientConfig { name, ip, zone, inner_key, outer_key }` in its
   configuration.
3. Returns an `Invite` file:

```toml
name      = "server-agent"
url       = "wss://server.example.com:8042"
zone      = "default"
inner_key = "<hex>"
outer_key = "<hex>"
```

**Step 2 — Operator transfers the invite file out-of-band.**

The invite is transferred to the client machine using a secure channel (e.g.
`scp`, secrets management system, or manual copy). The invite contains the
keys, so it must be treated as a secret.

**Step 3 — Client imports the invite.**

```
client$ openportal-agent add-server --invite /path/to/invite.toml
```

The client stores a `ServerConfig { name, url, zone, inner_key, outer_key }`
derived from the invite. Both sides now hold identical key material.

### 3.2 Invite File Structure

```toml
name      = "<server-agent-name>"
url       = "<wss://...>"
zone      = "<zone-name>"
inner_key = "<64-hex-char key>"
outer_key = "<64-hex-char key>"
```

| Field | Description |
|-------|-------------|
| `name` | Name of the server agent (used to identify the remote peer) |
| `url` | WebSocket URL the client will connect to |
| `zone` | Zone both peers must agree on |
| `inner_key` | 32-byte key, hex-encoded |
| `outer_key` | 32-byte key, hex-encoded |

Invite files are validated on load: name and zone must be non-empty and
contain only alphanumeric characters, `-`, or `_`; keys must not be null.

### 3.3 Key Rotation

Keys can be rotated without downtime. The server calls `rotate_client_keys`,
which generates a fresh key pair and returns a new invite. The client imports
the new invite via `rotate_server_keys`. The old invite becomes invalid
immediately.

---

## 4. Connection Authentication

When a client connects to a server, four independent checks are applied in
sequence. **All four must pass** before the connection is accepted.

### 4.1 Layer 1: IP Address Allowlisting

The server maintains a list of `ClientConfig` entries, each with an expected IP
address, CIDR range, or comma-separated list of several of either. The first
thing the server does after accepting a TCP connection is check the client's
IP against this list - a connection is allowed if it matches *any* entry in
a multi-entry list, not all of them.

If no `ClientConfig` matches the connecting IP, the connection is immediately
rejected before any message processing occurs.

IP ranges are specified in CIDR notation (e.g. `10.0.0.0/24` or
`2001:db8::/32`) - both IPv4 and IPv6 addresses and ranges are supported,
with identical syntax either way, and can be freely mixed within a
comma-separated list (`IpOrRange`, `paddington/src/config.rs`; see
[ipv6-support-design.md](../plans/ipv6-support-design.md) for how IPv6
support was added, and [agent-configuration.md](agent-configuration.md)
§1.2 for the multi-entry list syntax). A reverse proxy may be configured
via `proxy_header` to extract the real client IP from a header such as
`X-Forwarded-For`.

**Dual-stack listening is outside OpenPortal's control.** Whether a
listener bound to an IPv6 address also accepts IPv4-mapped connections is
governed by the OS-level `IPV6_V6ONLY` socket option, which varies by
platform default and which plain socket binding (as used here) does not
expose a way to override. An operator who needs both families reachable
should either rely on their OS's dual-stack default or run two listeners
(one per family, on different ports or interfaces) - this is a deployment
decision, not something OpenPortal's allowlisting or binding code can
resolve on its own.

### 4.2 Layer 2: Cryptographic Authentication

After the IP check, the server attempts to decrypt the client's opening
`Handshake` message (see [wire-protocol.md](wire-protocol.md) §4) using the
keys associated with each matching `ClientConfig`.

The Handshake message is encrypted with the double-envelope scheme using the
per-connection salts exchanged in the HTTP upgrade headers. A client without
the correct `inner_key` and `outer_key` cannot construct a message that
decrypts successfully. The server rejects connections where decryption fails.

This means that even if an attacker spoofs the correct IP address, they cannot
authenticate without the pre-shared keys.

### 4.3 Layer 3: Zone Verification

After the cryptographic handshake, both sides exchange `PeerDetails` objects
(encrypted). Each includes the zone the sender believes the connection belongs
to. The server checks that the zone in `PeerDetails` matches the zone
configured for that peer:

```
if peer_details.zone() != expected_zone → reject connection
```

Zone mismatch causes the connection to be closed even if the cryptographic
authentication succeeded. This prevents a legitimate peer in zone `A` from
accidentally or maliciously connecting via a channel configured for zone `B`.

### 4.4 Layer 4: Name Verification

The peer name in `PeerDetails` is checked against the `ClientConfig` entry
selected in Layer 2. A mismatched name causes the connection to be rejected.

---

## 5. Configuration File Encryption at Rest

The `encryption` field controls how **secret values stored in the config's
`extras` map** (e.g. a FreeIPA bind password or Slurm token, added via the
`secret` CLI command) are encrypted at rest. (The pre-shared peer keys
themselves are stored as hex in the TOML and are protected by restrictive file
permissions - `0600` on Unix - rather than by this scheme.) Two schemes are
available:

### 5.1 Environment Variable Scheme

```toml
[encryption]
type = "Environment"
key  = "OPENPORTAL_SECRET_KEY"
```

The named environment variable is read at startup. Its value is used as the
password for **Argon2** key derivation to produce a 32-byte key, which encrypts
/decrypts each stored secret with XChaCha20-Poly1305. This is the recommended
scheme for production; its strength is that of the operator-supplied secret.

### 5.2 Simple Scheme

```toml
[encryption]
type = "Simple"
```

The service's own name is used as the password. Because the name is **not
secret** (it appears in this same config file, in every issued invite, and in
logs), this scheme is **obfuscation, not encryption** - anyone who can read the
config can re-derive the key. Suitable for development or low-security
deployments only; use `Environment` in production.

### 5.3 Password-Based Key Derivation

Secrets are stored in a **versioned format**. New secrets (v1,
`Key::from_password_with_salt`) use a fresh random per-secret salt stored
alongside the ciphertext, with strong Argon2 parameters:

| Parameter | Value |
|-----------|-------|
| Iterations | 3 |
| Memory | 19456 KiB (19 MiB) |
| Output length | 32 bytes |
| Salt | 16 random bytes, stored with the ciphertext |

The random salt means identical passwords no longer produce identical keys
across deployments. For backward compatibility, older (v0) secrets - which used
`Key::from_password` with a fixed application-defined salt and orion's minimum
Argon2 cost (3 iterations / 8 KiB) - are still decryptable; re-running the
`secret` command re-encrypts them in the v1 format. See
[security-review.md](security-review.md) F2 for the assessment that motivated
this.

---

## 6. Zone Isolation

**Zones** are named trust domains. Every peer relationship is assigned a zone
name (default: `"default"`). A zone name must match on both sides of a
connection and is enforced at the connection layer (§4.3) and the message layer
(every `Message` carries a `zone` field checked by the recipient).

Zones allow multiple logically independent OpenPortal networks to share the same
physical infrastructure without messages leaking between them. For example:

- A production deployment and a test deployment can share the same agents but
  operate in separate zones.
- A provider with two independent portals can enforce zone separation so that
  portal A cannot receive messages intended for portal B.

Zone names are validated to contain only alphanumeric characters, `-`, `_`,
`<`, and `>`.

---

## 7. Trust Topology

Each agent only holds keys for its direct neighbours in the agent hierarchy.
The topology is strictly bounded:

```
Portal ←—key-pair-A—→ Provider ←—key-pair-B—→ Platform ←—key-pair-C—→ Instance
                                                             ←—key-pair-D—→ Account
                                                             ←—key-pair-E—→ Filesystem
```

- A Portal knows key-pair-A. It cannot speak to the Platform, Instance, Account,
  or Filesystem agents directly.
- A Provider knows key-pair-A and key-pair-B. It cannot speak to Account or
  Filesystem directly.
- Compromise of key-pair-A does not expose key-pair-B through key-pair-E.
- There is no master key that would allow impersonating all agents.

---

## 7.1 Blind Relay Proxy Trust Model

An `op-proxy` agent (see
[blind-relay-proxy-design.md](../plans/archive/blind-relay-proxy-design.md)) lets a
pair of agents that can each only make *outbound* connections communicate,
without either becoming a listening server the other can reach. It does
**not** add itself as a trusted intermediary in the sense every other agent
in §7's topology is - it is deliberately kept blind:

- The relayed pair (say `airr` and `brics`) share their own pre-shared key
  pair, generated and exchanged exactly as in §3, transferred out-of-band
  exactly as any other invite file is. **The proxy never sees this key
  pair.** It is a separate trust relationship from either agent's key pair
  with the proxy itself.
- Each relayed peer additionally holds an ordinary key pair with the proxy
  (again, provisioned exactly as in §3) - this secures only the
  agent↔proxy hop, and authenticates each agent to the proxy as itself. It
  grants no ability to read agent↔agent traffic.
- On top of the permanent pre-shared key, every relayed session establishes
  a **fresh** session key pair via a mutual-contribution bootstrap (one
  side contributes `session_outer_key`, the other `session_inner_key`) -
  see [wire-protocol.md](wire-protocol.md) §7.1. Compromise of one past
  session's keys does not expose any other session between the same pair.
  This is per-session key freshness, **not** forward secrecy: the session
  keys are key-transported under the permanent pre-shared keys, not agreed
  in-band, so it does not protect past traffic against later compromise of
  the permanent keys - see §2.5.
- The proxy enforces an explicit, default-deny `RelayPolicy`: it forwards
  a `(from, to)` pair only if an operator has explicitly `allow`ed it.
  Every other pair is dropped and logged, never silently forwarded.
- The proxy's role is reduced to: verify the agent↔proxy hop (§4, applied
  twice, independently, once per hop), check `RelayPolicy`, and forward
  opaque ciphertext. It never attempts to decrypt agent↔agent traffic, and
  the ciphertext it forwards is, by construction, indistinguishable from
  random bytes without the relayed pair's own keys.
- The recovery signal a restarted relayed peer's counterpart sends
  ([wire-protocol.md](wire-protocol.md) §7.3, `SessionUnknown`) is
  encrypted with the same permanent pre-shared key as the bootstrap
  itself, so the proxy cannot forge one to force a peer into spurious
  re-bootstraps.

This means a compromised proxy can, at worst, deny service (drop or refuse
to relay) or observe *metadata* (which pairs communicate, message
timing/size) - it cannot read message content, and it cannot impersonate
either relayed agent to the other without their pre-shared key pair, which
it never possesses.

An agent is free to use a *different* proxy for each relayed peer (there
is no requirement to route everything through one proxy) - each relayed
pair's trust properties above stand entirely on that pair's own
out-of-band key exchange and are unaffected by how many different proxies
are involved elsewhere in the agent's own connection graph.

A side effect of this trust model, unrelated to security but worth noting
here: because the proxy relays purely by peer identity and never
distinguishes *which* physical process behind that identity it's talking
to, several redundant server processes can share one identity behind a
proxy and get automatic failover between them - see
[highavailability.md](highavailability.md) §3.

---

## 8. Memory Safety

All key material is managed with the `secrecy` crate:

- Keys are stored in `SecretBox<Key>`, which implements `Zeroize` on drop.
  Key bytes are overwritten with zeros when they go out of scope.
- `Debug` formatting of `SecretBox<Key>` outputs `[[REDACTED]]`.
- Access to key bytes requires an explicit `.expose_secret()` call, making
  accidental exposure visible in code review.

The Rust codebase enforces `unsafe_code = "forbid"`, `unwrap_used = "deny"`,
and `expect_used = "deny"` at the lint level, ruling out entire classes of
memory safety and error-handling bugs.

---

## 9. Replay Protection

See [replay-protection-design.md](../plans/replay-protection-design.md) for
the full design; this section is the trust-model summary.

Authentication and encryption (§2-§4) do not, on their own, stop an
attacker (or the blind relay proxy itself) from capturing a legitimate,
validly-encrypted ongoing message and replaying it later to re-trigger its
effect. The per-message `info` value mixed into key derivation (§2.4)
guards against key reuse between *different* messages; it does not
protect a single message against being resent, since a replayed message
carries its original `info` value with it and re-derives the same key.

Ongoing message traffic (post-handshake application messages - Jobs,
Notifications, keepalives) carries a monotonically increasing per-sender
nonce, checked against a receiver-side sliding window - the standard
IPsec/WireGuard-style anti-replay scheme: a
high-water-mark plus a fixed-size bitmap of recently-accepted values,
rejecting anything already seen or too old to have a slot in the window.
The nonce lives inside the AEAD-authenticated ciphertext (not a plaintext
field), so the proxy - which never holds either relayed peer's keys - can
no more forge or strip it than it can read the payload itself; the same
mechanism applies uniformly to direct and relayed connections
(`paddington::anti_replay`, wired into both `Connection` and
`RelayedSession`), without either the proxy or any higher-level code
needing to know it exists. Window state resets alongside the session keys
it protects on every reconnect/re-bootstrap, rather than being persisted
across one.

**Rollout is negotiated, not a coordinated flag-day.** Each peer
advertises `supports_nonce: bool` in its `PeerDetails` (direct connections)
or `StartRelayedConnection`/`RelayedConnectionAccepted` (relayed
bootstrap) - fields that were already structured, safely-extensible
messages, so adding this one is as safe as `domain`/`domain_version` on
`Register`. A not-yet-upgraded peer's message simply lacks the field and
so is read as `supports_nonce: false`, correctly. Each side remembers the
other's confirmed support for the lifetime of the connection/session
(`Connection::peer_supports_nonce`, `RelayedSession::peer_supports_nonce`)
and only sends the wrapped `{nonce, payload}` shape to a peer that
confirmed it; a peer that hasn't is sent exactly the bare-string shape it
already expects (`NoncedPayload::for_peer`), not a degraded encoding of the
new one. The trust-relevant consequence: **a pair where either end hasn't
been upgraded gets no replay protection for that pair** - exactly the
pre-nonce behaviour, not a weaker version of the new one - while every
pair where both ends have been upgraded gets full protection immediately,
independent of the rest of the fleet's rollout state. See the design doc
§5 for the full mechanism.

**Handshake and bootstrap messages** (`Handshake`/`PeerDetails` for direct
connections; `StartRelayedConnection`/`RelayedConnectionAccepted`/
`SessionUnknown` for relayed bootstrap) carry the same kind of nonce,
checked against a *separate*, longer-lived window per peer that - unlike
the ongoing-traffic window above - does **not** reset on reconnect, since
these messages are encrypted (at least partly) under the *permanent*
pre-shared key pair, which itself never changes across reconnects; a
window that reset per connection would accept a replay against any fresh
connection attempt. Tracing through what a captured message here can
actually be used for shows the real risk is narrower than "the whole
handshake is unforgeable": `StartRelayedConnection` and `SessionUnknown`
can each unilaterally cause the receiver to reset session state or
re-bootstrap from a single captured message with no further live input
needed, which nonce-checking now closes; `Handshake`/`PeerDetails` cannot
be used to hijack or impersonate a session even without this (each
connection's session keys are freshly random and never derivable from
captured bytes), and `RelayedConnectionAccepted` was already effectively
replay-proof via its single-use `magic` correlation. All five message
types are nonce-checked uniformly regardless. For direct connections, this
needed no capability negotiation at all (unlike ongoing traffic): `nonce`
is `#[serde(default)] Option<u64>` on messages that were already
structured objects, so an old peer's message simply lacks the field
(read as `None`, skip the check) and serde already ignores the field on
an old peer's *receiving* end - there is no wire-shape change to gate. For
relayed bootstrap, no backward compatibility was needed at all (`op-proxy`
isn't deployed yet), so `nonce: u64` there is a plain, required field. See
the design doc §10 for the full mechanism and its threat-model reasoning.

---

## 10. Source File Reference

| Concept | Source file |
|---------|-------------|
| `Key`, `Salt`, `Signature`, encryption | `paddington/src/crypto.rs` |
| `Invite` (key provisioning file) | `paddington/src/invite.rs` |
| `ServiceConfig`, `ClientConfig`, `ServerConfig` | `paddington/src/config.rs` |
| Connection authentication sequence | `paddington/src/connection.rs` |
| Wire encryption format | `paddington/src/connection.rs` (`envelope_message`) |
| Zone enforcement | `paddington/src/connection.rs` (§726, §1255) |
| Blind relay bootstrap, session keys, `RelayPolicy` | `paddington/src/relay.rs` |
| Anti-replay window, `NoncedPayload` | `paddington/src/anti_replay.rs` |
