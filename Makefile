.PHONY: all setup dependencies build release test lint fmt fmt-check check check-rust timings dev client-check msrv clean install run help
.PHONY: package publish

PACKAGE_ARGS ?=

all: check

setup:
	mise install
	$(MAKE) dependencies

dependencies:
	mise exec -- node web/build/dependencies.mjs

build: dependencies
	mise exec -- cargo build --locked

timings:
	mise exec -- cargo build --locked --timings

release: dependencies
	mise exec -- cargo build --locked --release

package: build
	mise exec -- node web/build/package.mjs
	mise exec -- cargo package -p aggr-ui-model -p aggr-lib -p aggr $(PACKAGE_ARGS)

publish:
	mise exec -- node scripts/publish.mjs $(PACKAGE_ARGS)

test: dependencies
	mise exec -- cargo test --workspace --locked

lint: dependencies
	mise exec -- cargo clippy --workspace --locked --all-targets -- --deny warnings

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

# Cargo generates the shared types and compiles the embedded UI automatically.
client-check: build
	mise exec -- npm --prefix web run check
	mise exec -- npm --prefix web test
	mise exec -- npm --prefix web run test:build
	mise exec -- npm --prefix web run test:dev

# Vite serves the reader while the supervisor rebuilds and replaces Rust workers.
dev: export AGGR_CONFIG ?= examples/aggr.toml
dev: dependencies
	mise exec -- cargo run --locked -- dev $(ARGS)

# Build with the minimum supported Rust version declared in Cargo.toml.
msrv:
	mise exec rust@$$(perl -ne 'print $$1 if /^rust-version = "(.*)"/' Cargo.toml) -- cargo build --locked

clean:
	mise exec -- cargo clean

install:
	mise exec -- cargo install --path . --locked

# Sync, render and live-reload the demo config: make run ARGS="dev --port 3000"
run: dependencies
	mise exec -- cargo run --locked -- --config examples/aggr.toml $(ARGS)

help:
	@echo "Available targets:"
	@echo "  setup      - Install pinned build tools"
	@echo "  build      - Build debug binary"
	@echo "  release    - Build release binary"
	@echo "  package    - Prepare and verify the model, library and CLI crates"
	@echo "  publish    - Publish missing model, library and CLI versions to crates.io"
	@echo "  test       - Run all tests"
	@echo "  lint       - Run clippy with warnings denied"
	@echo "  fmt        - Format code"
	@echo "  fmt-check  - Check formatting"
	@echo "  check      - Rust and frontend checks (default)"
	@echo "  check-rust - Run Rust lint and tests in sequence"
	@echo "  timings    - Build with an HTML compiler timing report"
	@echo "  dev        - Run the demo with client HMR and automatic Rust restarts"
	@echo "  client-check - Type-check and test the reader"
	@echo "  msrv       - Build with the minimum supported Rust version"
	@echo "  clean      - Remove build artifacts"
	@echo "  install    - Install the binary locally"
	@echo "  run        - Run against examples/aggr.toml (ARGS=...)"
