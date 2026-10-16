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
