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
# op-clusters, op-filesystem, op-freeipa, op-slurm, op-cloudaccount,
# op-cloudportal, and example binaries in docs/
```

## Workspace Structure

This is a Cargo workspace with multiple crates. The workspace is organized into:

### Core Library Crates

- **paddington**: Low-level secure websocket peer-to-peer protocol for service communication. Handles cryptographic authentication, message passing, and connection management between services.

- **templemeads**: High-level agent framework built on paddington. Implements the Agent concept, Job management, Job Boards, and distributed task coordination. All agent executables depend on this. Deliberately domain-agnostic: `Job`, `Board`, `Command`, `Notification`, etc. are generic over one `L: templemeads::domain::Domain` type, chosen at compile time per binary via a type alias (e.g. `type Job = templemeads::job::Job<greatwestern::Hpc>;`). templemeads itself carries no command vocabulary - see `docs/plans/archive/grammar-split-design.md` for why and how this split happened.

- **greatwestern**: The HPC/Waldur command vocabulary (the `Instruction` enum, `ProjectIdentifier`/`UserIdentifier`, usage/storage reports, notification events) that rides on top of templemeads. This is the reference `Domain` - every built-in agent below is compiled against `greatwestern::Hpc`. A developer targeting a different kind of infrastructure entirely would write their own crate implementing `templemeads::domain::Domain` instead, and reuse paddington/templemeads unchanged. Two agents built against different `Domain`s are not expected to interoperate - that's intentional, not a bug.

### Agent Executable Crates

Each agent type is its own binary crate that implements specific infrastructure management logic:

- **portal** (`op-portal`): Entry point for user portals. Receives requests from portal software and routes Jobs to appropriate agents.

- **provider** (`op-provider`): Represents an infrastructure provider (e.g., a supercomputing center). Receives Jobs from portals and delegates to platform agents.

- **clusters** (`op-clusters`): Platform agent for managing multiple cluster instances.

- **cluster** (`op-cluster`): Instance agent for individual cluster management.

- **freeipa** (`op-freeipa`): Account agent that interfaces with FreeIPA for user account management.

- **filesystem** (`op-filesystem`): Agent for filesystem operations (creating directories, managing files).

- **slurm** (`op-slurm`): Agent that interfaces with the Slurm scheduler.

- **cloudaccount** (`op-cloudaccount`): Represents a single cloud account (e.g. an AWS account) assigned to a project. This is a deliberately rough prototype agent, co-developed alongside cloud operators who are still building out their side of the integration - it collapses what would normally be separate Instance + Account/Scheduler agents into one process (see `docs/plans/archive/op-cloudaccount-design.md`), holds its own project/user assignment state as plain JSON files (there's no cloud-side API for this yet), and reconstructs usage reports by parsing whatever cost-report JSON files the operators drop into a directory. Expect this to need reshaping once the cloud side of the integration matures.

- **cloudportal** (`op-cloudportal`): A self-contained `Portal` agent representing the "cloud" side of a portal-to-portal relationship (e.g. a central portal creating Awards on it). Also a deliberately rough prototype (see `docs/plans/archive/op-cloudportal-design.md`): there's no real portal management software (no Waldur) behind it, so it stores Award state itself as plain JSON files, addresses/is addressed directly by the upstream portal (no virtual-resource/offering indirection - that mechanism turned out to be same-process-only, see the design doc §4), and requires a human operator to `approve`/`reject` a pending Award via CLI subcommands before a background poller provisions it on whichever `cloudaccount` its `AwardDetails.template` maps to. Also added one-shot CLI support (`run --one-shot`) to `templemeads::portal::run()`, previously only available to Account/Filesystem/Scheduler agents.

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

Exception: **op-cloudaccount** is an Instance agent that does *not* delegate to separate Account/Scheduler agents - it's a rough prototype that merges those roles into one process (see the crate note above and `docs/plans/archive/op-cloudaccount-design.md`).

Exception: **op-cloudportal** is a Portal agent that receives Jobs directly from its upstream portal rather than via a bridge, and provisions approved Awards on **op-cloudaccount** directly rather than via a Provider/Platform layer (see the crate note above and `docs/plans/archive/op-cloudportal-design.md`).

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
- After making changes, run `cargo fmt` to format the code and `cargo clippy` to check for warnings. Fix any warnings introduced by the changes before finishing.

## Examples

The docs/ directory contains example implementations that demonstrate OpenPortal concepts:

- **docs/echo**: Basic paddington services that echo messages (demonstrates message passing)
- **docs/job**: Basic templemeads agents that send Jobs (demonstrates agent Job handling)
- **docs/cmdline**: Standardized agent structure with CLI and config file handling

Study these examples when creating new agents or understanding the framework.

## License

Source files include SPDX license identifiers in their headers. Code files use MIT, while configuration and documentation files use CC0-1.0.
