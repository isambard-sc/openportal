build:
	@cargo build

release:
	@cargo build --release

python:
	@maturin develop -m python/Cargo.toml

clean:
	@cargo clean

TESTS = ""
test:
	@cargo test $(TESTS) --offline --lib -- --color=always --nocapture

docs: build
	@cargo doc --no-deps

style-check:
	@rustup component add rustfmt 2> /dev/null
	cargo fmt --all -- --check

lint:
	@rustup component add clippy 2> /dev/null
	cargo clippy --all-targets --all-features -- -D warnings

dev-portal:
	cargo run --bin portal-svc

dev-provider:
	cargo run --bin provider-svc

.PHONY: build python test docs style-check lint
