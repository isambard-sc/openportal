<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Contributing to OpenPortal

Thank you for your interest in contributing to OpenPortal! This document explains
how to report bugs, suggest improvements, and submit code changes.

## Table of contents

- [Reporting bugs](#reporting-bugs)
- [Suggesting features](#suggesting-features)
- [Security vulnerabilities](#security-vulnerabilities)
- [Setting up your development environment](#setting-up-your-development-environment)
- [Making changes](#making-changes)
- [Code standards](#code-standards)
- [Submitting a pull request](#submitting-a-pull-request)
- [Documentation](#documentation)

---

## Reporting bugs

Please open a [GitHub issue](https://github.com/isambard-sc/openportal/issues) and include:

- A clear description of the problem and what you expected to happen
- The version of OpenPortal you are using (`cargo pkgid paddington` or the release tag)
- Steps to reproduce, including any relevant configuration snippets (with secrets redacted)
- Error messages, log output, or stack traces

## Suggesting features

Open a GitHub issue with the label `enhancement`. Describe the use case you have in mind
and how the feature would fit into the existing agent model. We are particularly interested
in new agent types that connect OpenPortal to other infrastructure systems.

## Security vulnerabilities

Please **do not** open a public issue for security vulnerabilities. See
[SECURITY.md](SECURITY.md) for how to report them privately.

---

## Setting up your development environment

### Prerequisites

- Rust toolchain (stable, via [rustup](https://rustup.rs))
- `cargo` (included with rustup)
- For Python bindings: Python 3.8+ and `maturin` (`pip install maturin`)

### Building

```bash
# Clone the repository
git clone https://github.com/isambard-sc/openportal
cd openportal

# Development build
make

# Run the test suite
make test

# Check formatting
make style-check

# Run lints
make lint
```

The full list of make targets is in the [Makefile](Makefile). You can also run
`cargo build`, `cargo test`, etc. directly.

### Running the examples

The `docs/` directory contains self-contained examples that build and run without
any external infrastructure:

```bash
# Echo example (basic paddington message passing)
cd docs/echo && cargo run --bin echo-server &
cd docs/echo && cargo run --bin echo-client

# Job example (basic templemeads agent job handling)
cd docs/job && cargo run --bin job-server &
cd docs/job && cargo run --bin job-client
```

See [docs/README.md](docs/README.md) for a full walkthrough.

---

## Making changes

1. Fork the repository and create a branch from `main`:

   ```bash
   git checkout -b my-feature
   ```

2. Make your changes. Keep commits focused — one logical change per commit.

3. Add or update tests for any changed behaviour. Tests live in `lib.rs` or
   separate `tests/` files within each crate.

4. Run the full check suite before pushing:

   ```bash
   make style-check
   make lint
   make test
   ```

5. Push to your fork and open a pull request against `main`.

---

## Code standards

The codebase enforces strict Rust safety rules via workspace-level lints:

| Lint | Level | Meaning |
|---|---|---|
| `unsafe_code` | `forbid` | No `unsafe` blocks anywhere |
| `unwrap_used` | `deny` | Use `?` or explicit error handling instead of `.unwrap()` |
| `expect_used` | `deny` | Same — no `.expect()` |
| `dbg_macro` | `deny` | No `dbg!()` in committed code |

All error handling uses [`anyhow`](https://docs.rs/anyhow). Propagate errors with `?`
and add context with `.context("what was happening")` where it aids debugging.

Formatting is enforced by `rustfmt` with the project's `rustfmt.toml`. Run
`make style-check` to check and `cargo fmt` to fix.

Clippy is run with `make lint` and warnings are treated as errors.

### Adding a new agent

Each agent is its own binary crate. The `docs/cmdline/` example is the recommended
starting point — it shows the standard CLI structure, config file handling, and
agent initialisation pattern. Copy it as a template and adapt the instruction
handlers.

---

## Submitting a pull request

- Target branch: `main`
- Title: short imperative summary (`Add slurm quota agent`, `Fix reconnect loop in paddington`)
- Description: explain *why* the change is needed, not just what it does. Link to
  any related issues with `Fixes #123` or `Relates to #456`
- Keep PRs focused. Large changes are easier to review if split into a series of
  smaller PRs
- All CI checks must pass before a PR can be merged

---

## Documentation

Documentation lives in `docs/` (examples and walkthroughs) and `docs/specifications/`
(protocol reference). Prose documentation uses the CC0-1.0 licence; source files
use MIT. Include the appropriate SPDX header when adding new files:

```
// SPDX-FileCopyrightText: © 2024 Your Name <your@email.com>
// SPDX-License-Identifier: MIT
```

or for Markdown/TOML:

```
<!--
SPDX-FileCopyrightText: © 2026 Your Name <your@email.com>
SPDX-License-Identifier: CC0-1.0
-->
```
