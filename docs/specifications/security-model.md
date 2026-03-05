<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Security Model

This document describes the security model of OpenPortal: the threat model it is
designed to address, how cryptographic keys are structured and provisioned, how
connections are authenticated, and how zone isolation limits the blast radius of
any compromise.

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
address or CIDR range. The first thing the server does after accepting a TCP
connection is check the client's IP against this list.

If no `ClientConfig` matches the connecting IP, the connection is immediately
rejected before any message processing occurs.

IP ranges are specified in CIDR notation (e.g. `10.0.0.0/24`). A reverse proxy
may be configured via `proxy_header` to extract the real client IP from a
header such as `X-Forwarded-For`.

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

The `ServiceConfig` (stored in TOML on disk) contains all peer keys. It can be
encrypted at rest using one of two schemes controlled by the `encryption` field:

### 5.1 Environment Variable Scheme

```toml
[encryption]
type = "Environment"
key  = "OPENPORTAL_SECRET_KEY"
```

The named environment variable is read at startup. Its value is passed through
**Argon2** key derivation (`Key::from_password`) to produce a 32-byte
encryption key, which is then used to encrypt/decrypt the config file contents
with XChaCha20-Poly1305. This is the recommended scheme for production.

### 5.2 Simple Scheme

```toml
[encryption]
type = "Simple"
```

The service's own name is used as the password for `Key::from_password`. This
provides obfuscation but not strong protection, since the "password" is not
secret. Suitable for development or low-security deployments only.

### 5.3 Password-Based Key Derivation

`Key::from_password` uses **Argon2** (via the `orion::kdf` module) with a
fixed application-defined salt and the following parameters:

| Parameter | Value |
|-----------|-------|
| Iterations | 3 |
| Memory | 8 blocks |
| Output length | 32 bytes |

The fixed salt ensures reproducible key derivation from the same password, which
is necessary so the config can be decrypted on restart without storing the
derived key.

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

## 9. Source File Reference

| Concept | Source file |
|---------|-------------|
| `Key`, `Salt`, `Signature`, encryption | `paddington/src/crypto.rs` |
| `Invite` (key provisioning file) | `paddington/src/invite.rs` |
| `ServiceConfig`, `ClientConfig`, `ServerConfig` | `paddington/src/config.rs` |
| Connection authentication sequence | `paddington/src/connection.rs` |
| Wire encryption format | `paddington/src/connection.rs` (`envelope_message`) |
| Zone enforcement | `paddington/src/connection.rs` (§726, §1255) |
