.PHONY: all build release test lint fmt fmt-check check msrv clean install run help

all: check

build:
	cargo build

release:
	cargo build --release

test:
	cargo test

lint:
	cargo clippy --all-targets -- --deny warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

# What CI runs.
check: fmt-check lint test

# Build with the minimum supported Rust version declared in Cargo.toml.
msrv:
	cargo +$$(perl -ne 'print $$1 if /^rust-version = "(.*)"/' Cargo.toml) build --locked

clean:
	cargo clean

install:
	cargo install --path . --locked

# Render and serve the demo config: make run ARGS="serve --port 3000"
run:
	cargo run -- --config examples/aggr.toml $(ARGS)

help:
	@echo "Available targets:"
	@echo "  build      - Build debug binary"
	@echo "  release    - Build release binary"
	@echo "  test       - Run all tests"
	@echo "  lint       - Run clippy with warnings denied"
	@echo "  fmt        - Format code"
	@echo "  fmt-check  - Check formatting"
	@echo "  check      - fmt-check, lint, test (default)"
	@echo "  msrv       - Build with the minimum supported Rust version"
	@echo "  clean      - Remove build artifacts"
	@echo "  install    - Install the binary locally"
	@echo "  run        - Run against examples/aggr.toml (ARGS=...)"
