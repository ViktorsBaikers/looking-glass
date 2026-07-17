.PHONY: frontend build test run check fmt clippy verify

# `central` embeds frontend/build via rust-embed, so the SPA must be built before
# any cargo command. These targets enforce that ordering.

frontend:
	cd frontend && npm ci && npm run build

build: frontend
	cargo build --release

test: frontend
	cargo test --all

run: frontend
	cargo run --bin central

check:
	cd frontend && npm run check

fmt:
	cargo fmt --all -- --check

clippy: frontend
	cargo clippy --all-targets -- -D warnings

verify: frontend fmt clippy
	cargo test --all
	cargo build --release
