<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Specifications

This directory contains formal specifications for the OpenPortal protocol and
its components. The documents are ordered from the highest level of abstraction
(what instructions agents send each other) down to the lowest (how bytes are
encrypted on the wire).

For a narrative introduction to OpenPortal — its design philosophy, agent
types, and worked examples — see the [docs overview](../README.md).

`templemeads` (Job/Notification transport, wire protocol, security model,
bridge API, agent configuration) is generic over a `Domain` — the compile-time
choice of instruction and notification vocabulary. Every built-in OpenPortal
agent is compiled against `greatwestern`, the reference `Domain`, which is
what [instruction-protocol.md](instruction-protocol.md),
[notification-protocol.md](notification-protocol.md), and
[json-types.md](json-types.md) document. If you want to bring your own
vocabulary instead, see
[writing-a-domain.md](writing-a-domain.md).

---

## Documents

### [writing-a-domain.md](writing-a-domain.md)
**Implementing your own command vocabulary**

For anyone reusing `paddington`/`templemeads` for infrastructure other than
HPC/Waldur: how to implement the `templemeads::domain::Domain` trait, define
your own `Instruction` and `NotificationEvent` types, and wire them up so your
own agents interoperate over the same secure peer-to-peer transport without
depending on `greatwestern` at all.

---

### [instruction-protocol.md](instruction-protocol.md)
**The `greatwestern` instruction text protocol**

Specifies the full grammar for the instruction strings that `greatwestern`
(the reference `Domain`) agents exchange: all 53 instructions, their argument
formats, and the identifier types (`UserIdentifier`, `ProjectIdentifier`,
`UserMapping`, `ProjectMapping`, `Destination`, etc.). This is the primary
reference for anyone implementing a portal or agent that needs to construct
or parse `greatwestern` commands. Building a different `Domain`? See
[writing-a-domain.md](writing-a-domain.md) instead.

---

### [notification-protocol.md](notification-protocol.md)
**The OpenPortal notification protocol**

Specifies the fire-and-forget `Notification` system — the lightweight,
unacknowledged signalling mechanism that complements the robust Job system.
`Notification` itself is generic over a `Domain`; this document covers both
the domain-agnostic parts (owned by `templemeads`) and `greatwestern`'s
concrete `NotificationEvent` vocabulary. Covers:

- The conceptual distinction between Jobs (TCP-like) and Notifications (UDP-like)
- The full `NotificationEvent` grammar: all 10 user and project events
  (`user_added`, `user_removed`, `user_changed`, `user_blocked`,
  `user_unblocked`, and the project equivalents)
- Wire representation (the `Notify` `Command` variant and `Notification` JSON)
- Per-hop routing behaviour (`Downstream` → forward, `Destination` → call
  notify runner)
- How to register a `notify_runner` in a Rust agent
- How to construct and send a notification
- Delivery guarantees and limitations

---

### [json-types.md](json-types.md)
**JSON serialisation of result types**

Specifies the JSON format of every value that can appear in a `Job`'s `result`
field once a job completes: `Job` itself (domain-agnostic, from `templemeads`)
and `greatwestern`'s result types - `AwardDetails` (wire name: `ProjectDetails`),
`ProjectUsageReport`, `Quota`, `Usage`, and all others. Includes the
`result_type` name reference table mapping Rust type names to their JSON
schemas.

---

### [wire-protocol.md](wire-protocol.md)
**The Templemeads and Paddington wire protocols**

Specifies the full protocol stack from the application layer down to the
network layer:

- **Templemeads layer**: `Envelope` (job delivery wrapper) and the `Command`
  enum (Put, Update, Delete, Register, Sync, HealthCheck, …)
- **Paddington layer**: `Message` framing, control vs keepalive vs regular
  message types, and the Paddington `Command` enum for connection lifecycle
- **Encryption layer**: the double-envelope wire frame format, HKDF-SHA512
  key derivation, and XChaCha20-Poly1305 AEAD
- **Handshake**: HTTP header salt exchange, session key negotiation, and
  `PeerDetails` identity exchange

---

### [security-model.md](security-model.md)
**Security model and key management**

Explains the trust model underlying OpenPortal — why there is no central
"god key", how per-peer symmetric key pairs are structured, and how they are
provisioned using the invite file mechanism. Also covers:

- The four-layer connection authentication sequence (IP allowlist →
  cryptographic handshake → zone verification → name verification)
- Config file encryption at rest (Environment and Simple schemes)
- Zone isolation
- The per-agent trust topology
- Memory safety guarantees (`SecretBox`, `Zeroize`)

---

### [python-api.md](python-api.md)
**OpenPortal Python API reference**

Documents the `openportal` Python module — the compiled Rust/pyo3 extension
that portal software uses to interact with OpenPortal via the bridge agent.
Covers initialisation, all top-level functions (`run`, `status`, `fetch_jobs`,
`send_result`, `health`, `diagnostics`, `sync_offerings`, etc.), and every
exported class (`Job`, `Status`, `Health`, `Diagnostics`, `Destination`,
`UserIdentifier`, `AwardDetails` / `ProjectDetails`, usage/storage types). Includes usage
patterns for both the portal → OpenPortal direction and the OpenPortal →
portal callback direction.

---

### [bridge-api.md](bridge-api.md)
**Bridge HTTP API**

Specifies the HTTP/JSON API exposed by the `op-bridge` agent, which allows
non-Rust portal software (e.g. Python/Django applications) to interact with
the OpenPortal network. Covers:

- Authentication (HMAC-SHA512 signatures, `Date` header, nonce replay
  prevention, rate limiting)
- All 14 endpoints (`/run`, `/status`, `/fetch_jobs`, `/send_result`,
  `/sync_offerings`, `/health`, `/restart`, `/diagnostics`, …)
- The two-direction communication model: portal → OpenPortal (via `/run`)
  and OpenPortal → portal (via the bridge board and signal URL)

---

### [agent-configuration.md](agent-configuration.md)
**Agent configuration reference**

The complete configuration reference for all ten agent types. Covers:

- Common TOML config fields shared by all agents (`name`, `url`, `ip`,
  `port`, peer lists, encryption)
- The common CLI subcommands (`init`, `client`, `server`, `encryption`,
  `extra`, `secret`, `run`)
- Per-agent sections with default ports, config file paths, and all
  agent-specific options:
  - **Portal**, **Provider**, **Bridge**, **Clusters**, **Cluster**
  - **FreeIPA** (server hostnames, credentials, group mappings)
  - **Filesystem** (volume config, quota engines, Lustre ID strategies)
  - **Slurm** (sacctmgr mode and REST API mode)
  - **Cloud Account** (assignment state directory, accounting directory,
    currency)
  - **Cloud Portal** (Award state directory, offerings table, approval
    CLI subcommands)
- Default port reference table and a typical deployment walkthrough

---

## Protocol Stack Overview

```
┌──────────────────────────────────────────────────────────┐
│  Portal software (Python, Django, …)                     │
│    ↕  bridge-api.md                                      │
├──────────────────────────────────────────────────────────┤
│  Instruction text protocol   instruction-protocol.md     │
│  Notification event grammar  notification-protocol.md    │
│  Result JSON types           json-types.md               │
├──────────────────────────────────────────────────────────┤
│  Templemeads: Envelope + Command   wire-protocol.md §1   │
│  Paddington:  Message              wire-protocol.md §2   │
│  Encryption:  double-envelope      wire-protocol.md §3   │
│  Handshake:   key exchange         wire-protocol.md §4   │
├──────────────────────────────────────────────────────────┤
│  Key model / trust topology        security-model.md     │
├──────────────────────────────────────────────────────────┤
│  WebSocket / TLS                                         │
└──────────────────────────────────────────────────────────┘
```

## Deployment and Configuration

See [agent-configuration.md](agent-configuration.md) for how to initialise,
wire together, and run agents in a real deployment.

---

### [typescript-bindings.md](typescript-bindings.md)
**TypeScript bindings**

Describes the auto-generated TypeScript type definitions produced from the
`templemeads` and `greatwestern` Rust types via
[ts-rs](https://github.com/Aleph-Alpha/ts-rs). Covers:

- How to regenerate the bindings with `cargo test`
- The full table of exported types, which crate and Rust source they derive
  from, and why `Job` needs a hand-written binding rather than a derived one
- Serialisation notes: timestamp formats, identifier strings, HashMap key
  conventions, and custom-format fields such as storage sizes
- The hand-written `identifiers.ts` utilities — parse/stringify helpers for
  `PortalIdentifier` (`templemeads/bindings/`) and `UserIdentifier`,
  `ProjectIdentifier`, `UserMapping`, `ProjectMapping`
  (`greatwestern/bindings/`)
- How to add a new exported type

---

### [notes.md](notes.md)
**Errata, provisional schemas, and operational notes**

Records known gaps in the formal specifications, provisional or still-evolving
schemas, and operational observations that do not fit neatly into the other
documents. Covers:

- Provisional `HealthInfo` and `DiagnosticsReport` schemas
- Duplicate job detection and resolution behaviour
- Job expiry behaviour
- Virtual agent mechanism (`sync_offerings`)
- Operational troubleshooting notes (connection failures, key rotation, health
  cascade timing, slow job threshold, diagnostics path format)
