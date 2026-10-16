# 0003. Output contract for scripts and agents

Status: Accepted
Date: 2026-10-16

## Context
People, shell scripts and AI agents run the CLI. Output that changes shape depending on whether stdout is a TTY breaks agents that run inside a pseudo-terminal.

## Decision
- `--json` (or `IMIFY_OUTPUT=json`) switches every command to machine-readable output. Without it, output is for people. A TTY only changes styling, never the format.
- stdout carries the result. Progress, logs and warnings go to stderr.
- Long-running commands with `--json` emit newline-delimited JSON events on stdout, ending with one `{"type":"result", ...}` event.
- Errors are `{"error":{"code":"...","message":"...","hint":"..."}}` on stderr. `code` is a stable, documented enum.
- Exit codes: 0 success, 1 unexpected, 2 usage, 3 authentication or permission (`auth_required`, `auth_rejected`, `auth_expired`, `login_failed`, `permission_denied`), 4 not found, 5 validation failed, 6 conflict, 7 network, server or rate limit, 8 cancelled (the user declined a confirmation prompt), 130 interrupted (interrupt signal).
- Without a TTY the CLI never prompts. A command that needs input fails with a hint; `--yes` confirms.
- `NO_COLOR` and `--no-color` disable color. `-v` and `-vv` add tracing on stderr.

## Consequences
The JSON output is a public, versioned contract. Each command's output has a JSON Schema in `docs/reference/schemas/`, and snapshot tests pin the output. Removing or renaming a field is a breaking change.
