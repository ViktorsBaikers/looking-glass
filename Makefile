.PHONY: frontend build test run check fmt clippy check-scripts verify

# `central` embeds frontend/build via rust-embed, so the SPA must be built before
# any cargo command. These targets enforce that ordering.

frontend:
	cd frontend && npm ci && npm run build

build: frontend
	cargo build --locked --release

test: frontend
	cd frontend && npm test && npm run check
	cargo test --all --locked

run: frontend
	cargo run --bin central

check:
	cd frontend && npm run check

fmt:
	cargo fmt --all -- --check

clippy: frontend
	cargo clippy --locked --all-targets -- -D warnings

# README content, release workflow shape, and THIRD_PARTY_NOTICES.md freshness.
check-scripts:
	sh scripts/check-readme.sh
	sh scripts/check-release-workflow.sh
	python3 scripts/third-party-notices.py --check

# Everything CI runs except the Linux-only installer test and the Docker build.
verify: fmt clippy test check-scripts
	cd frontend && npm run test:e2e
	cargo build --locked --release
