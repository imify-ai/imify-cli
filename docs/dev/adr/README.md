# Architecture decision records

One file per decision, numbered in order. A decision is changed by a new ADR that supersedes the old one; accepted ADRs are not edited except to set their status to `Superseded by NNNN`. Before the first release (0.1.0), accepted ADRs may be amended in place; git history keeps the previous text.

| ADR | Decision |
|---|---|
| [0001](0001-rust-workspace-sdk-and-cli.md) | Rust workspace with an SDK crate and a CLI crate |
| [0002](0002-noun-verb-command-grammar.md) | Noun-verb command grammar |
| [0003](0003-output-contract.md) | Output contract for scripts and agents |
| [0004](0004-authentication.md) | Browser login, environment token, platform tokens later |
| [0005](0005-host-discovery-and-profiles.md) | Host discovery and profiles |
| [0006](0006-upload-transport.md) | Streaming PUT upload with a local resume journal |
| [0007](0007-server-derived-dataset-identity.md) | Dataset identity is derived by the server |
| [0008](0008-inline-tui.md) | Inline TUI only in the first release |
| [0009](0009-sdk-types-and-api-contract.md) | Hand-written SDK types, contract-tested against OpenAPI (superseded by 0015) |
| [0010](0010-mock-only-testing.md) | Tests run against mocks only |
| [0011](0011-no-telemetry.md) | No telemetry |
| [0012](0012-distribution.md) | Distribution with dist and release-plz |
| [0013](0013-contribution-policy.md) | Contribution policy |
| [0014](0014-ci-gates-and-guarded-paths.md) | CI gates and guarded paths |
| [0015](0015-generated-sdk-api-layer.md) | SDK API layer generated from the vendored OpenAPI document |
| [0016](0016-self-update-and-update-check.md) | Self update and update check |

## Template

```markdown
# NNNN. Title

Status: Proposed | Accepted | Superseded by NNNN
Date: YYYY-MM-DD

## Context
The forces at play, stated as facts.

## Decision
What we do, in active voice.

## Consequences
What becomes easier, what becomes harder, what we give up.
```
