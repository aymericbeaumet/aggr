.PHONY: all build release test lint fmt fmt-check check timings msrv clean install run help

all: check

build:
	cargo build --locked

timings:
	cargo build --locked --timings

release:
	cargo build --locked --release

test:
	cargo test --locked

lint:
	cargo clippy --locked --all-targets -- --deny warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

check: fmt-check
	$(MAKE) lint
	$(MAKE) test

# Build with the minimum supported Rust version declared in Cargo.toml.
msrv:
	cargo +$$(perl -ne 'print $$1 if /^rust-version = "(.*)"/' Cargo.toml) build --locked

clean:
	cargo clean

install:
	cargo install --path . --locked

# Sync, render and live-reload the demo config: make run ARGS="dev --port 3000"
run:
	cargo run --locked -- --config examples/aggr.toml $(ARGS)

help:
	@echo "Available targets:"
	@echo "  build      - Build debug binary"
	@echo "  release    - Build release binary"
	@echo "  test       - Run all tests"
	@echo "  lint       - Run clippy with warnings denied"
	@echo "  fmt        - Format code"
	@echo "  fmt-check  - Check formatting"
	@echo "  check      - Format check, lint and tests (default)"
	@echo "  timings    - Build with an HTML compiler timing report"
	@echo "  msrv       - Build with the minimum supported Rust version"
	@echo "  clean      - Remove build artifacts"
	@echo "  install    - Install the binary locally"
	@echo "  run        - Run against examples/aggr.toml (ARGS=...)"
