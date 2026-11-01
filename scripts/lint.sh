#!/bin/sh
# Formatting and clippy checks for the whole workspace. CI runs this script
# (.github/workflows/lint.yml).
#
# Usage: scripts/lint.sh
#   RUST_TOOLCHAIN=v   toolchain to check with (default 1.99.1)
#
# The toolchain is always passed as `cargo +v`: on a machine where `cargo`
# is a mise shim, plain `cargo fmt` recurses through the shim and fails.
set -eu

cd "$(dirname "$0")/.."
toolchain=${RUST_TOOLCHAIN:-1.99.1}

cargo +"$toolchain" fmt --all -- --check
cargo +"$toolchain" clippy --workspace --all-targets -- -D warnings
cargo +"$toolchain" clippy --workspace --all-targets --all-features -- -D warnings
