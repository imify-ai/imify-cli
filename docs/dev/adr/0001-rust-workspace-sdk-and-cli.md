# 0001. Rust workspace with an SDK crate and a CLI crate

Status: Accepted
Date: 2026-10-16

## Context
The CLI must install with a single command and no runtime, start fast enough to be called in loops by agents, and move multi-GB datasets. The platform also needs an SDK; the CLI is meant to be a thin client over it, the way the AWS CLI relates to its SDK.

## Decision
Rust. One Cargo workspace with two published crates: `imify` (library, the SDK) and `imify-cli` (binary `imify`). The CLI crate does no network I/O of its own; CI rejects a direct HTTP client dependency in it.

The workspace also has one unpublished crate, `imify-test-support`. It generates synthetic datasets for tests, and the other crates use it only as a dev-dependency.

## Consequences
- Static binaries for macOS and Linux; no interpreter to install.
- The SDK is usable on its own and is published to crates.io.
- Local dataset validation is a Rust implementation of the platform's ingest rules, separate from the server, which stays authoritative. A check is an error only when the platform enforces the same rule; every other check is a warning. The local check therefore never rejects a dataset the server accepts, and a drift between the two shows up as a missed error, not as a false rejection.
- SDKs in other languages, if needed, are generated from the API or bind to this crate.
