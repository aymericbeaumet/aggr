.PHONY: all build release test lint fmt fmt-check check check-rust timings client-build client-check client-dev msrv clean install run help

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

# Keep the two Cargo commands ordered while frontend checks run alongside them.
check: fmt-check
	$(MAKE) -j check-rust client-check

check-rust:
	$(MAKE) lint
	$(MAKE) test

client-build:
	npm --prefix web run build

client-check:
	npm --prefix web run check
	npm --prefix web test

client-dev:
	npm --prefix web run dev

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
	@echo "  check      - Rust and frontend checks (default)"
	@echo "  check-rust - Run Rust lint and tests in sequence"
	@echo "  timings    - Build with an HTML compiler timing report"
	@echo "  client-build - Compile the embedded reader assets"
	@echo "  client-check - Type-check and test the reader"
	@echo "  client-dev - Run Vite for frontend development"
	@echo "  msrv       - Build with the minimum supported Rust version"
	@echo "  clean      - Remove build artifacts"
	@echo "  install    - Install the binary locally"
	@echo "  run        - Run against examples/aggr.toml (ARGS=...)"
