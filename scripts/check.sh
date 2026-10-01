#!/usr/bin/env bash
# Shared branch/PR/release gates. No phone or external service is required.
set -euo pipefail
cd "$(dirname "$0")/.."
JAVA_CHECK_DIR=$(mktemp -d)
trap 'rm -rf "$JAVA_CHECK_DIR"' EXIT
javac --release 8 -Xlint:-options -d "$JAVA_CHECK_DIR" native/java/UsageSetting.java
cargo fmt --manifest-path native/Cargo.toml --check
cargo clippy --manifest-path native/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path native/Cargo.toml --locked
shellcheck -s sh module/*.sh
shellcheck build.sh scripts/*.sh
node --check module/webroot/app.js
node --test tests/*.test.cjs
