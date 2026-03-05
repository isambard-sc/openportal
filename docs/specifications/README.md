<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Specifications

This directory contains formal specifications for the OpenPortal protocol and
its components. The documents are ordered from the highest level of abstraction
(what instructions agents send each other) down to the lowest (how bytes are
encrypted on the wire).

---

## Documents

### [instruction-protocol.md](instruction-protocol.md)
**The OpenPortal instruction text protocol**

Specifies the full grammar for the instruction strings that agents exchange:
all 53 instructions, their argument formats, and the identifier types
(`UserIdentifier`, `ProjectIdentifier`, `UserMapping`, `ProjectMapping`,
`Destination`, etc.). This is the primary reference for anyone implementing
a portal or agent that needs to construct or parse OpenPortal commands.

---

### [json-types.md](json-types.md)
**JSON serialisation of result types**

Specifies the JSON format of every value that can appear in a `Job`'s `result`
field once a job completes: `Job` itself, `ProjectDetails`, `ProjectUsageReport`,
`Quota`, `Usage`, and all other return types. Includes the `result_type` name
reference table mapping Rust type names to their JSON schemas.

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

The complete configuration reference for all eight agent types. Covers:

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
- Default port reference table and a typical deployment walkthrough

---

## Protocol Stack Overview

```
┌──────────────────────────────────────────────────────────┐
│  Portal software (Python, Django, …)                     │
│    ↕  bridge-api.md                                      │
├──────────────────────────────────────────────────────────┤
│  Instruction text protocol   instruction-protocol.md     │
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

### [notes.md](notes.md)
**Errata, provisional schemas, and operational notes**

Records known gaps in the formal specifications, provisional or still-evolving
schemas, and operational observations that do not fit neatly into the other
documents. Covers:

- Known errata (e.g. `GetUserDirs` / `GetLocalUserDirs` missing from the
  instruction parser)
- Provisional `HealthInfo` and `DiagnosticsReport` schemas
- Duplicate job detection and resolution behaviour
- Job expiry behaviour
- Virtual agent mechanism (`sync_offerings`)
- Operational troubleshooting notes (connection failures, key rotation, health
  cascade timing, slow job threshold, diagnostics path format)
