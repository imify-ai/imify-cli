# Contributing

## Setup

- Rust 1.88 or newer (`rustup`).
- Gate tools: `cargo install --locked cargo-deny typos-cli`, and [Vale](https://vale.sh/docs/install).

```sh
scripts/check.sh --fast   # fmt, clippy, tests
scripts/check.sh          # every gate CI runs
```

## Workflow

1. Every change starts from an issue. Comment on the issue before starting larger work.
2. Branch: `feature/issue-<id>-<slug>` or `fix/issue-<id>-<slug>`.
3. Open a pull request against `main`. Its title is a [conventional commit](https://www.conventionalcommits.org) header, for example `feat(datasets): resume interrupted uploads`; it becomes the commit message when the PR is squash-merged.
4. A PR is ready when `scripts/check.sh` passes.

Commits and PR descriptions contain no `Co-authored-by` lines and no tool attribution. CI rejects them.

## Code

- The `imify-cli` crate does no network I/O; it calls the `imify` SDK crate.
- Public SDK items have doc comments with an example that compiles.
- Tests describe behavior, one behavior per test, and never contact a real server.
- Prose is concise and factual; documentation opens with an example. Vale checks Markdown files against the house style in `.vale/styles/Imify/`.

## Decisions

Changes to architecture are proposed as a new ADR in [`docs/dev/adr/`](docs/dev/adr/README.md) in the same PR.

## Licensing

Contributions are licensed under [Apache-2.0](LICENSE), as stated in its section 5.
