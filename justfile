#!/usr/bin/env -S just --justfile
# Pen's task runner. Install once: `brew install just`. Then from the repo
# root: `just run`, `just test`, `just gate`, etc. Run `just` alone to list
# every recipe. Requires `cargo`/`rustc` on PATH (rust-toolchain.toml pins
# the exact version `rustup` resolves to).

default:
    @just --list

# Build and run the release binary (what you want day to day).
run *ARGS:
    cargo run --release -p pen -- {{ARGS}}

# Build and run the debug binary (faster builds, slower runtime).
dev *ARGS:
    cargo run -p pen -- {{ARGS}}

# Open a specific headless design scene (see `pen --list-scenes`).
scene NAME *ARGS:
    cargo run --release -p pen -- --scene {{NAME}} {{ARGS}}

# Run every test in the workspace.
test:
    cargo test --all-targets

# Run the release-only performance budget tests (10 MB open, keystroke).
bench:
    cargo test --release -p buffer -- --ignored --nocapture

# Format the workspace in place.
fmt:
    cargo fmt --all

# Format check only (CI-equivalent, no changes written).
fmt-check:
    cargo fmt --all -- --check

# Lint with warnings denied, matching CI.
lint:
    cargo clippy --all-targets -- -D warnings

# The full Phase-1/2 gate: format check, lint, tests, perf budgets.
gate: fmt-check lint test bench
    @echo "Gate passed."

# Remove build artifacts.
clean:
    cargo clean
