# 0014. CI gates and guarded paths

Status: Accepted
Date: 2026-10-16

## Context
A rule that exists only in documentation erodes. The configuration that defines the gates must not change without the maintainer noticing.

## Decision
- CI gates every PR: rustfmt, clippy with a deny list, tests on three operating systems, doctests, MSRV, rustdoc, cargo-deny, typos, Vale prose style, crate boundary, cargo-semver-checks, and a PR policy check (conventional title, no attribution lines).
- `scripts/check.sh` runs the same gates locally.
- Paths listed in `.github/guarded-paths` (CI, lint and release configuration, policy scripts, authentication code) are merged by the maintainer only. The PR policy check lists touched guarded paths in its summary.
- Branch protection on `main` requires the CI checks.

## Consequences
Policy changes are visible as changes to guarded files and always reach the maintainer.
