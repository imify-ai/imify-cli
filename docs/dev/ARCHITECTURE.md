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

Besides the platform API, the SDK contacts GitHub releases in `imify::update`, behind the feature `self-update` that `imify-cli` enables ([ADR 0016](adr/0016-self-update-and-update-check.md)).

The SDK has no terminal concerns: it does not print, prompt or read environment variables other than the ones a `TokenSource` is documented to read. Progress is reported as typed events the caller consumes.

## Module layout

```
crates/imify/src/
  lib.rs            re-exports the public API
  client.rs         Client, ClientBuilder
  error.rs          Error, the single error type of the SDK
  retry.rs          RetryPolicy, retry decisions and backoff
  discovery.rs      OidcSettings: the host's identity provider issuer and client id
  health.rs         Health, Readiness: liveness and readiness of the API
  auth/             TokenSource trait and its implementations; browser login (PKCE,
                    localhost redirect listener, ID token checks), CredentialStore trait,
                    StoredLogin with refresh, token revocation
  datasets/         dataset endpoints and models
  ingest/           upload engine: Transport trait, planner, pacing, resume journal, progress events
  download/         download engine for training checkpoints: manifest checks, target
                    directory scan and writes relative to directory file descriptors,
                    storage GET, manifest refresh, progress events
  validate/         offline structural checks of LeRobot v3 and v2.1 datasets
  update.rs,        feature `self-update`: latest GitHub release, install receipt of the
  update/           dist shell installer, download and run of the release's installer

crates/imify-cli/src/
  main.rs           parse, dispatch, map errors to exit codes
  cli.rs            global flags and the top-level command enum
  commands/         one module per noun: datasets, auth, config, doctor, self (self_update)
  credentials/      where a login is stored: OS keychain, file; selection by IMIFY_CREDENTIAL_STORE
  browser.rs        opens the sign-in URL in the system browser
  config/           settings and their sources: --api-url, environment, profiles in config.toml, defaults
  dirs.rs           the configuration and upload journal directories (XDG layout)
  private_file.rs   files only the user can change: mode checks, atomic writes
  output/           Renderer (human, json), error envelope, exit codes
  tui/              inline viewport, theme, terminal restore
  update/           release lookup settings, install method of the running binary and its
                    update command, the update hint after a command
```

## Seams

Each seam is a trait with one production implementation and test doubles. They exist because a second implementation is already decided, not in anticipation of one.

| Seam | Crate | Production implementation | Planned addition |
|---|---|---|---|
| `TokenSource` | imify | `EnvToken` (`IMIFY_TOKEN`), `StoredLogin` | platform-issued API tokens |
| `CredentialStore` | imify (trait), imify-cli (stores) | OS keychain, file readable only by the user | none |
| `Transport` | imify | streaming `PUT` per file | direct-to-storage upload with server verification |
| `Renderer` | imify-cli | human (tables, inline TUI) | none; `json` is the second implementation |

## Upload flow

1. `validate` runs offline; the upload does not start when the report has an error.
2. The engine creates an ingest session and uploads `meta/info.json` first, so the server rejects an unsupported format before any bulk transfer.
3. Remaining files upload with bounded concurrency. Each finished file is written to the resume journal in the user's state directory: `$XDG_STATE_HOME/imify/uploads/`, or `~/.local/state/imify/uploads/`, on macOS and Linux ([ADR 0006](adr/0006-upload-transport.md)).
4. `complete` asks the server to catalogue the session. The server decides whether the episodes create a new dataset or extend an existing one.

## Download flow

1. The engine fetches the manifest of a training checkpoint: one presigned storage URL, relative path and size per file. It refuses unsafe or colliding paths and storage URLs other than `https`, or `http` on `localhost`, `127.0.0.1` or `::1`, before it writes anything.
2. It opens the target directory and scans it without following links. An entry the download does not own stops it before any storage request. Files that exist with the manifest size are skipped.
3. Remaining files download with bounded concurrency through an HTTP client that sends no `Authorization` header. Each file is written to `<path>.part` relative to directory file descriptors with `O_NOFOLLOW`, checked against the manifest size, synced and renamed into place.
   - An existing `.part` that is a regular file with one link and fewer bytes `n` than the manifest size is continued with `Range: bytes=n-`. A 206 with `Content-Range: bytes n-(size-1)/size` appends to it, and a 200 truncates it and writes from byte 0. Another `Content-Range` or a 416 removes it and requests the file once more without a range.
   - Any other `.part` is removed, and a new one is created with `O_CREAT | O_EXCL` only after storage accepts the request, so a refused response leaves no `.part`. An interrupted transfer keeps its `.part` for the next attempt or run.
4. A 403 from storage fetches the manifest again, once for all files rejected with the same URLs. The new manifest must list the same paths and sizes.

## Testing

Tests never contact a real server.

| Level | Tooling | Covers |
|---|---|---|
| Unit | `cargo test` | pure logic: planning, retry decisions, journal, error mapping |
| SDK integration | `wiremock` | HTTP contract and failure modes |
| CLI end-to-end | `assert_cmd`, `insta` | the `imify` binary against a mock server: output, exit codes, TUI frames |

Mock fixtures are built from the types generated from the vendored OpenAPI document, so a mock cannot describe a response the API does not have. Datasets used in tests come from `DatasetBuilder` in `imify-test-support`:

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
