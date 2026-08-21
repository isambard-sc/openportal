<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal

OpenPortal is a distributed infrastructure management protocol that provides
secure, authenticated communication between user portals (e.g. Waldur) and
digital research infrastructure (e.g. supercomputers). Rather than requiring
a single service with "god keys" that grant full access to the infrastructure,
OpenPortal uses a peer-to-peer agent architecture where each agent handles only
the specific operations it is permitted to perform.

Each agent is a small, statically compiled Rust executable. Agents communicate
over encrypted WebSocket connections and exchange structured Jobs. A typical
deployment has agents running at both the portal side and the infrastructure
side, coordinating to carry out tasks such as account creation, project
management, filesystem provisioning, and usage reporting.

For a full description of the design, the agent types, and worked examples, see
the [docs](docs) directory. For formal protocol and API specifications, see
[docs/specifications](docs/specifications).

## Agent types

| Binary | Role |
|---|---|
| `op-portal` | Entry point for portal software |
| `op-provider` | Represents an infrastructure provider |
| `op-clusters` | Platform agent for clusters |
| `op-cluster` | Instance agent for a single cluster |
| `op-freeipa` | Account management via FreeIPA |
| `op-filesystem` | Filesystem and quota management |
| `op-slurm` | Slurm account management |
| `op-bridge` | HTTP bridge for Python portal software |

## Compiling OpenPortal

OpenPortal is written in Rust, so you will need to have Rust installed.

To compile OpenPortal, run:

```bash
make
```

or

```bash
make release
```

or use the `cargo` command directly:

```bash
cargo build
```

or

```bash
cargo build --release
```

## Installing OpenPortal

The result of compilation will be a number of executable binaries in the
`target/debug` or `target/release` directories. These are static executables
that can be safely copied to their target destinations and run there.

To understand where to install the executables, you will first need to
understand what OpenPortal is, and how it is used. Please see the
[docs](docs) directory for detailed documentation on the
design and implementation of OpenPortal, together with worked examples.
Formal protocol and API specifications are in
[docs/specifications](docs/specifications).
