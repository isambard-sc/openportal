# SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: CC0-1.0

build:
	@cargo build

release:
	@cargo build --release

python:
	@maturin develop -m python/Cargo.toml

clean:
	@cargo clean

TESTS = ""
# No --lib: that silently skipped every test in the agent binary crates, which
# is where most of the privileged logic lives.
#
# The `python` crate is not a workspace default-member and needs a second
# invocation. `--no-default-features` turns off `pyo3/extension-module`, which
# tells pyo3 not to link libpython - correct when building the module that
# Python itself loads, but it leaves a test binary with no interpreter to link
# against. `--lib` for the same reason the stub_gen binary is gated: it needs
# maturin's build environment.
test:
	@cargo test $(TESTS) --offline --all-targets -- --color=always --nocapture
	@cargo test -p openportal --lib --offline --no-default-features -- --color=always --nocapture

docs: build
	@cargo doc --no-deps

style-check:
	@rustup component add rustfmt 2> /dev/null
	cargo fmt --all -- --check

lint:
	@rustup component add clippy 2> /dev/null
	cargo clippy --all-targets --all-features -- -D warnings
	@./scripts/check-secret-writes.sh
	@./scripts/check-nss-lookups.sh

audit:
	@cargo audit --version > /dev/null 2>&1 || cargo install cargo-audit --locked
	cargo audit

dev-portal:
	cargo run --bin portal-svc

dev-provider:
	cargo run --bin provider-svc

.PHONY: build python test docs style-check lint audit
