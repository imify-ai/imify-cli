#!/usr/bin/env bash
# Runs every gate CI runs on a pull request. A PR is ready when this passes.
#
#   scripts/check.sh          all gates
#   scripts/check.sh --fast   fmt, clippy and tests only
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

step() { printf '\n== %s\n' "$1"; }

step "fmt"
cargo fmt --all --check

step "clippy"
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

step "test"
cargo test --workspace --all-features --locked

[[ "${1:-}" == "--fast" ]] && exit 0

msrv="$(sed -n 's/^rust-version *= *"\([^"]*\)".*/\1/p' Cargo.toml)"
: "${msrv:?rust-version not found in Cargo.toml}"
step "msrv ($msrv)"
if ! rustup toolchain list | grep "^$msrv-" >/dev/null; then
  printf 'Rust %s is not installed. Install it with:\n  rustup toolchain install %s --profile minimal\n' "$msrv" "$msrv" >&2
  exit 1
fi
cargo "+$msrv" check --workspace --all-features --all-targets --locked

step "doc"
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked

step "crate boundary"
scripts/policy/check-crate-boundary.sh

step "deny"
cargo deny check

step "typos"
typos

step "vale"
git ls-files '*.md' ':!:**/CHANGELOG.md' | xargs vale --config .vale.ini

printf '\nall gates passed\n'
