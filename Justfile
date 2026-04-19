# rustotron developer recipes.
#
# Install just: `brew install just` (macOS) or `cargo install just`.
# Run `just` with no args to see the recipe list.

# Show available recipes.
default:
    @just --list

# Build the debug binary.
build:
    cargo build --all-targets

# Build the release binary.
build-release:
    cargo build --release

# Run the full test suite.
test:
    cargo test --all-targets --all-features

# Run the binary with any passed-through args (e.g. `just run -- --mock`).
run *ARGS:
    cargo run -- {{ARGS}}

# Lint gate: fmt check + clippy with warnings denied. Same as CI.
lint:
    cargo fmt --check
    cargo clippy --all-targets --all-features -- -D warnings

# Auto-fix formatting and clippy issues.
fix:
    cargo fmt
    cargo clippy --all-targets --all-features --fix --allow-dirty --allow-staged

# Rebuild/retest on file change. Requires `cargo install cargo-watch`.
watch:
    cargo watch -x "test --all-targets"

# Full CI gate (matches .github/workflows/ci.yml).
ci: lint test
    cargo test --release
