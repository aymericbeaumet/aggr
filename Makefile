.PHONY: all setup build release test lint fmt fmt-check check check-rust timings client-build client-check client-dev msrv clean install run help

all: check

setup:
	mise install rust node cmake ninja actionlint aqua:EmbarkStudios/cargo-deny chromedriver
	mise exec -- npm ci --prefix web

build:
	mise exec -- cargo build --locked

timings:
	mise exec -- cargo build --locked --timings

release:
	mise exec -- cargo build --locked --release

test:
	mise exec -- cargo test --locked

lint:
	mise exec -- cargo clippy --locked --all-targets -- --deny warnings

fmt:
	mise exec -- cargo fmt --all

fmt-check:
	mise exec -- cargo fmt --all -- --check

# Keep the two Cargo commands ordered while frontend checks run alongside them.
check: fmt-check
	$(MAKE) -j check-rust client-check

check-rust:
	$(MAKE) lint
	$(MAKE) test

# Node is pinned in mise.toml and only needed to develop the reader: its compiled bundles are
# committed under src/theme/static/app and embedded by the Rust binary.
client-build:
	mise exec -- npm --prefix web run build

client-check:
	mise exec -- npm --prefix web run check
	mise exec -- npm --prefix web test

client-dev:
	mise exec -- npm --prefix web run dev

# Build with the minimum supported Rust version declared in Cargo.toml.
msrv:
	mise exec rust@$$(perl -ne 'print $$1 if /^rust-version = "(.*)"/' Cargo.toml) -- cargo build --locked

clean:
	mise exec -- cargo clean

install:
	mise exec -- cargo install --path . --locked

# Sync, render and live-reload the demo config: make run ARGS="dev --port 3000"
run:
	mise exec -- cargo run --locked -- --config examples/aggr.toml $(ARGS)

help:
	@echo "Available targets:"
	@echo "  setup      - Install locked development tools and client packages"
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
