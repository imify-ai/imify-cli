# Architecture

`imify` is a command-line client for the imify platform API. It is two published crates and one test-only crate in one Cargo workspace:

| Crate | Kind | Responsibility |
|---|---|---|
| `crates/imify` | library, published as `imify` | Everything that touches the network or the platform's data formats: HTTP client, authentication, typed API models, upload engine, local dataset validation. |
| `crates/imify-cli` | binary `imify`, published as `imify-cli` | Everything that touches the terminal: argument parsing, configuration and profiles, output rendering, the TUI. |
| `crates/imify-test-support` | library, not published | Synthetic LeRobot v2.1 and v3.0 datasets for tests. Other crates use it only as a dev-dependency. |

```
            ┌──────────────────────── imify-cli ─────────────────────────┐
 argv ────▶ │ clap grammar ─▶ commands/<noun> ─▶ output (human | json)   │ ─▶ stdout / stderr
            │        │                 │                tui (inline)     │
            │     config/            calls                                │
            └──────────────────────────┼──────────────────────────────────┘
                                       ▼
            ┌───────────────────────── imify ────────────────────────────┐
            │ Client ─▶ auth::TokenSource   datasets   ingest::Transport  │ ─▶ imify API
            │           error::Error        validate (offline)           │
            └────────────────────────────────────────────────────────────┘
```

## Crate boundary

`imify-cli` never builds an HTTP request and never parses an API response. It calls `imify` and renders what it gets back. CI enforces this: `scripts/policy/check-crate-boundary.sh` fails when `imify-cli` depends directly on an HTTP client.

The SDK has no terminal concerns: it does not print, prompt or read environment variables other than the ones a `TokenSource` is documented to read. Progress is reported as typed events the caller consumes.

## Module layout

```
crates/imify/src/
  lib.rs            re-exports the public API
  client.rs         Client, ClientBuilder
  error.rs          Error, the single error type of the SDK
  retry.rs          RetryPolicy, retry decisions and backoff
  discovery.rs      OidcSettings: the host's identity provider issuer and client id
  auth/             TokenSource trait and its implementations; browser login (PKCE,
                    localhost redirect listener, ID token checks), CredentialStore trait,
                    StoredLogin with refresh, token revocation
  datasets/         dataset endpoints and models
  ingest/           upload engine: Transport trait, planner, pacing, resume journal, progress events
  validate/         offline structural checks of LeRobot v3 and v2.1 datasets

crates/imify-cli/src/
  main.rs           parse, dispatch, map errors to exit codes
  cli.rs            global flags and the top-level command enum
  commands/         one module per noun: datasets, auth, config, doctor
  config/           profiles, file locations, environment overrides
  output/           Renderer (human, json), error envelope, exit codes
  tui/              inline viewport, theme, terminal restore
```

## Seams

Each seam is a trait with one production implementation and test doubles. They exist because a second implementation is already decided, not in anticipation of one.

| Seam | Crate | Production implementation | Planned addition |
|---|---|---|---|
| `TokenSource` | imify | `EnvToken` (`IMIFY_TOKEN`), `StoredLogin` | platform-issued API tokens |
| `CredentialStore` | imify (trait), imify-cli (stores) | OS keychain | file readable only by the user |
| `Transport` | imify | streaming `PUT` per file | direct-to-storage upload with server verification |
| `Renderer` | imify-cli | human (tables, inline TUI) | none; `json` is the second implementation |

## Upload flow

1. `validate` runs offline; the upload does not start when the report has an error.
2. The engine creates an ingest session and uploads `meta/info.json` first, so the server rejects an unsupported format before any bulk transfer.
3. Remaining files upload with bounded concurrency. Each finished file is written to the resume journal in the platform state directory: `$XDG_STATE_HOME/imify/uploads/` on Linux, `~/Library/Application Support/imify/uploads/` on macOS ([ADR 0006](adr/0006-upload-transport.md)).
4. `complete` asks the server to catalogue the session. The server decides whether the episodes create a new dataset or extend an existing one.

## Testing

Tests never contact a real server.

| Level | Tooling | Covers |
|---|---|---|
| Unit | `cargo test` | pure logic: planning, retry decisions, journal, error mapping |
| SDK integration | `wiremock` | HTTP contract and failure modes |
| CLI end-to-end | `assert_cmd`, `insta` | the `imify` binary against a mock server: output, exit codes, TUI frames |

Mock fixtures are validated against the vendored OpenAPI document, so a mock cannot describe a response the API does not have. Datasets used in tests come from `DatasetBuilder` in `imify-test-support`:

```rust
use imify_test_support::{DatasetBuilder, Defect, Format};

let dataset = DatasetBuilder::new(Format::V3_0)
    .episodes(3)
    .defect(Defect::MissingVideo)
    .build()?;
// dataset.root() is a temporary directory, removed when `dataset` is dropped.
```

The builder writes `meta/`, parquet data (uncompressed unless `data_compression` sets a codec) and placeholder video files. Each `Defect` changes the dataset in one respect. The same configuration always produces byte-identical files. A crate uses it through `[dev-dependencies]`, with `imify-test-support = { path = "../imify-test-support" }`.

## Decisions

Architecture decisions are recorded in [`adr/`](adr/README.md).
