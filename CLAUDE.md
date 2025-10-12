<!--
SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

OpenPortal is a distributed infrastructure management protocol implementation written in Rust. It provides secure communication between user portals (e.g., Waldur) and digital research infrastructure (e.g., supercomputers). The system uses a peer-to-peer agent-based architecture where each agent handles specific infrastructure management tasks without requiring centralized "god keys."

## Build Commands

```bash
# Development build
make
# or
cargo build

# Release build (optimized, stripped binaries)
make release
# or
cargo build --release

# Run tests
make test
# or
cargo test --offline --lib -- --color=always --nocapture

# Run specific test(s) - set TESTS variable
make test TESTS="test_name"

# Build Python bindings
make python
# or
maturin develop -m python/Cargo.toml

# Generate documentation
make docs
# or
cargo doc --no-deps

# Code quality checks
make style-check    # Check formatting with rustfmt
make lint           # Run clippy with strict warnings
```

## Development Commands

```bash
# Run portal service locally
make dev-portal
# or
cargo run --bin portal-svc

# Run provider service locally
make dev-provider
# or
cargo run --bin provider-svc

# Run specific binary
cargo run --bin <binary-name>
# Available binaries: portal-svc, provider-svc, op-bridge, op-cluster,
# op-clusters, op-filesystem, op-freeipa, and example binaries in docs/
```

## Workspace Structure

This is a Cargo workspace with multiple crates. The workspace is organized into:

### Core Library Crates

- **paddington**: Low-level secure websocket peer-to-peer protocol for service communication. Handles cryptographic authentication, message passing, and connection management between services.

- **templemeads**: High-level agent framework built on paddington. Implements the Agent concept, Job management, Job Boards, and distributed task coordination. All agent executables depend on this.

### Agent Executable Crates

Each agent type is its own binary crate that implements specific infrastructure management logic:

- **portal** (`op-portal`): Entry point for user portals. Receives requests from portal software and routes Jobs to appropriate agents.

- **provider** (`op-provider`): Represents an infrastructure provider (e.g., a supercomputing center). Receives Jobs from portals and delegates to platform agents.

- **clusters** (`op-clusters`): Platform agent for managing multiple cluster instances.

- **cluster** (`op-cluster`): Instance agent for individual cluster management.

- **freeipa** (`op-freeipa`): Account agent that interfaces with FreeIPA for user account management.

- **filesystem** (`op-filesystem`): Agent for filesystem operations (creating directories, managing files).

- **slurm** (`op-slurm`): Agent that interfaces with the Slurm scheduler.

- **bridge** (`op-bridge`): Bridges non-Rust portal implementations to the OpenPortal network. Runs a local HTTP server to translate API calls into OpenPortal Jobs.

- **python**: Python library (via pyo3) for calling into OpenPortal via the bridge agent.

## Architecture Concepts

### Agent Hierarchy

Jobs flow through the system in a hierarchical manner:

1. **Portal** receives request from portal software → creates Job
2. **Provider** receives Job → determines which platform handles it
3. **Platform** receives Job → delegates to specific instance
4. **Instance** receives Job → may delegate to account/filesystem agents
5. **Account/Filesystem** agents perform actual privileged operations

Each agent only has the permissions needed for its specific role, avoiding centralized privileged access.

### Jobs and Job Boards

- **Job**: A task/request with a unique ID, source, destination, payload, and status
- **Job Board**: A distributed queue where agents post Jobs and subscribe to Jobs meant for them
- Jobs can be in states: pending, in_progress, completed, failed
- The system is designed to handle agent failures gracefully - Jobs can be recovered and reassigned

### Message Passing

- All inter-agent communication goes through paddington's secure websocket protocol
- Messages are authenticated and encrypted
- Agents can be distributed across different machines/networks
- Connection management is handled automatically with health checks and reconnection

### Configuration

Agents use TOML configuration files (typically in ~/.config/openportal/ or specified via command line). Configuration includes:

- Agent identity (name, keys)
- Network settings (bind address, peers)
- Service-specific settings (e.g., FreeIPA connection details)

## Code Standards

The codebase enforces strict Rust safety standards via lints in Cargo.toml files:

- **unsafe_code = "forbid"**: No unsafe code allowed
- **dbg_macro = "deny"**: No debug macros in production code
- **unwrap_used = "deny"**: Must handle errors explicitly, no .unwrap()
- **expect_used = "deny"**: Must handle errors explicitly, no .expect()

When writing or modifying code:
- Use proper error handling with Result types and the anyhow crate
- Follow existing patterns for agent implementation
- Maintain the security model - agents should only have necessary permissions
- Add tests to the appropriate crate's lib.rs or separate test files

## Examples

The docs/ directory contains example implementations that demonstrate OpenPortal concepts:

- **docs/echo**: Basic paddington services that echo messages (demonstrates message passing)
- **docs/job**: Basic templemeads agents that send Jobs (demonstrates agent Job handling)
- **docs/cmdline**: Standardized agent structure with CLI and config file handling

Study these examples when creating new agents or understanding the framework.

## License

Code uses SPDX identifiers: MIT license for code (SPDX-License-Identifier: MIT), CC0-1.0 for config/docs.
