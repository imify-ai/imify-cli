# 0008. Inline TUI only in the first release

Status: Accepted
Date: 2026-10-16

## Context
Uploads are long-running, parallel and can fail per file; they need a live view. Listing commands are read by scripts and agents as often as by people.

## Decision
The TUI uses ratatui with an inline viewport: live progress rendered within the normal terminal flow, leaving a summary behind. Listing and detail commands print static tables. There is no full-screen mode in the first release. The TUI is off with `--json`, `--quiet`, or when stderr is not a TTY.

## Consequences
The TUI foundation (theme, terminal restore on exit, panic and interrupt, snapshot tests of frames) is built once and reused by a full-screen browser later.
