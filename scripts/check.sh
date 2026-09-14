#!/usr/bin/env bash
# Shared branch/PR/release gates. No phone or external service is required.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --manifest-path native/Cargo.toml --check
cargo clippy --manifest-path native/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path native/Cargo.toml --locked
shellcheck -s sh module/*.sh
shellcheck build.sh scripts/*.sh
node --check module/webroot/app.js
node --test tests/*.test.cjs
