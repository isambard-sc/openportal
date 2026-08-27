# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

OpenPortal is a distributed infrastructure management protocol implementation written in Rust. It provides secure communication between user portals (e.g., Waldur) and digital research infrastructure (e.g., supercomputers). The system uses a peer-to-peer agent-based architecture where each agent handles specific infrastructure management tasks without requiring centralized "god keys."

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

- **slurm** (`op-slurm`): Agent that interfaces with the Slurm scheduler. Unlike the other agents this crate also builds a library (`src/lib.rs`), so that the operator tools in `slurm/tools/` - currently `get_reservation_report` - run the agent's own accounting code rather than a second copy of it. A tool is a `[[bin]]` in `slurm/Cargo.toml`; it reads `sacct` directly and takes no config file.

- **bridge** (`op-bridge`): Bridges non-Rust portal implementations to the OpenPortal network. Runs a local HTTP server to translate API calls into OpenPortal Jobs.

- **proxy** (`op-proxy`): A blind relay for two agents that can each only make outbound connections (neither can open a port the other can reach). Depends only on `paddington`, never `templemeads` - it has no `Domain`, no Jobs, and never decrypts the traffic it forwards; see `docs/plans/archive/blind-relay-proxy-design.md`. Agents opt in explicitly via a `proxy` field in their paddington config, and the proxy operator must separately `allow` each `(agent, agent)` pair before it will relay between them (default-deny).

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

When writing or modifying code:
- Use proper error handling with Result types and the anyhow crate
- Follow existing patterns for agent implementation
- Maintain the security model - agents should only have necessary permissions
- Add tests to the appropriate crate's lib.rs or separate test files
- After making changes, run `make style-check`, `make lint` and `make test` (or `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings` and `cargo test --all-targets`). Fix any warnings introduced by the changes before finishing. Note `--all-targets` on both: without it clippy skips test code and `cargo test` skips every test in the agent binary crates.
- Lints are declared once in `[workspace.lints]` in the root `Cargo.toml`; member crates inherit them with `[lints] workspace = true`. `unwrap_used`, `expect_used`, `indexing_slicing` and `dbg_macro` are denied in production code (`clippy.toml` exempts tests), and `unsafe_code` is forbidden. Use `get`/`first`/`split_first`/slice patterns rather than indexing.
- Release builds set `panic = "abort"` **and** `overflow-checks = true`, so any reachable panic or integer overflow is a remote process kill. Arithmetic on values that arrive from a peer must be explicitly saturating or checked - see `Usage` and `StorageSize` in `greatwestern`.
- Any file containing key material must be written with `paddington::config::write_secret_file`, never a bare `fs::write`. `scripts/check-secret-writes.sh` (run by `make lint` and CI) enforces this.
- Release binaries are **statically linked against musl**, and musl has no NSS implementation at all. Never resolve a Unix user or group through libc (`nix::unistd::User::from_name`, `Group::from_name`, `getpwnam_r`, `getgrnam_r`): in a musl binary those see only `/etc/passwd` and `/etc/group` plus a single fragile `nscd` attempt, so a directory-backed name looks absent whenever `nscd` is down and reports `EIO` whenever it is merely busy. Go through the host's `getent`, which consults every source in `nsswitch.conf` - `filesystem/src/nameservice.rs` is the reference implementation and explains it in full. `scripts/check-nss-lookups.sh` (run by `make lint` and CI) enforces this.
- A name lookup that fails to answer is not a name that does not exist. Keep the two apart: report a genuine absence as a terminal error and an indeterminate result as a retryable one, or a transient outage in an identity service becomes a permanent job failure.

## Examples

The docs/ directory contains example implementations that demonstrate OpenPortal concepts:

- **docs/echo**: Basic paddington services that echo messages (demonstrates message passing)
- **docs/job**: Basic templemeads agents that send Jobs (demonstrates agent Job handling)
- **docs/cmdline**: Standardized agent structure with CLI and config file handling

Study these examples when creating new agents or understanding the framework.

## License

Source files include SPDX license identifiers in their headers. Code files use MIT, while configuration and documentation files use CC0-1.0.
