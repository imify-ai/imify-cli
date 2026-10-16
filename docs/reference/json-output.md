# JSON output and exit codes

```console
$ imify --json --no-such-flag
$ echo $?
2
```

stderr:

```json
{"error":{"code":"invalid_argument","message":"unexpected argument '--no-such-flag' found","hint":"Run `imify --help`."}}
```

`--json` or `IMIFY_OUTPUT=json` switches every command to JSON. Without either, output is text for people. Whether stdout is a terminal changes styling only, never the format.

## Streams

| Stream | Human mode | JSON mode |
|---|---|---|
| stdout | the result | the result: one JSON document, or NDJSON events |
| stderr | progress, logs, warnings, errors | logs, the error envelope |

Human output escapes control characters and Unicode format characters, such as bidirectional overrides and zero-width characters, in text from the API, the identity provider and local files. A file named `a<ESC>[2J` prints as `a\u{1b}[2J`. A line feed inside a name, path or message prints as `\u{a}`, so every value stays on its line. JSON output carries the text unchanged, escaped by JSON rules.

## Global flags

| Flag | Environment | Effect |
|---|---|---|
| `--json` | `IMIFY_OUTPUT=json` | JSON output. `--json` takes precedence over `IMIFY_OUTPUT=human`. |
| `-q`, `--quiet` | | Writes the result only. Cannot be combined with `-v`. |
| `-v`, `-vv` | | Logs at debug or trace level to stderr. |
| `--yes` | | Confirms without prompting. |
| `--no-color` | `NO_COLOR` (non-empty) | Disables color. |
| `--api-url <URL>` | `IMIFY_API_URL` | The API host. `--api-url` takes precedence over `IMIFY_API_URL`, which takes precedence over the config file. See [Configuration](../guide/configuration.md). |

`IMIFY_OUTPUT` accepts `json` and `human`. Any other value fails with `invalid_argument` and exit code 2. Global flags are valid before or after the command.

## Results

A command that returns one value writes one JSON document to stdout. A command that returns a collection writes a JSON array. Field names are `snake_case`, timestamps are RFC 3339 UTC, sizes are bytes as integers and durations are milliseconds as integers.

Each command's output has a schema in [`schemas/`](schemas/).

## Events

```json
{"type":"progress","files_done":1}
{"type":"progress","files_done":2}
{"type":"result","dataset_id":42}
```

A long-running command with `--json` writes newline-delimited JSON events to stdout. Every line has a `type`. The last line has type `result` and carries the result. With `--quiet`, only the `result` line is written. In human mode, events go to stderr and the result goes to stdout. When stderr is a terminal, a live progress view replaces the event lines, except with `--quiet`, `-v`, `-vv` or `TERM=dumb`. Ctrl-C stops the command with `interrupted` and exit code 130.

Schema: [`schemas/event.json`](schemas/event.json).

## Errors

```json
{"error":{"code":"not_found","message":"Dataset 42 does not exist.","hint":null}}
```

A failed command writes the error envelope to stderr and exits with the code's exit code. `code` is a stable value from the table below. `message` states what happened. `hint` states what to do next, and is `null` when there is no hint. In human mode, stderr shows `error: <message>` and `hint: <hint>`.

Schema: [`schemas/error.json`](schemas/error.json).

| Code | Exit | Meaning |
|---|---|---|
| `internal` | 1 | Unexpected failure in the CLI. |
| `invalid_argument` | 2 | Invalid command line, unsupported `IMIFY_OUTPUT`, invalid API URL or config file, or input required without a terminal. |
| `auth_required` | 3 | The command needs a login or a token. |
| `auth_rejected` | 3 | The server rejected the access token. |
| `auth_expired` | 3 | The stored login has expired or was revoked. Run `imify auth login`. |
| `login_failed` | 3 | `imify auth login` did not complete: access denied, no redirect within 5 minutes, no free redirect port, or an identity provider error. |
| `permission_denied` | 3 | The access token lacks the permission for the request. |
| `not_found` | 4 | The resource does not exist, or `--resume` found no interrupted upload. |
| `dataset_invalid` | 5 | The local validation found at least one error in the dataset, or the API rejected the dataset. |
| `conflict` | 6 | The request conflicts with the current state of the resource, or the interrupted upload cannot be continued. |
| `network` | 7 | The server could not be reached. |
| `server` | 7 | The server failed to handle the request. |
| `rate_limited` | 7 | The server rejected the request because of its rate limit. |
| `cancelled` | 8 | The user declined a confirmation prompt. |
| `interrupted` | 130 | The user stopped the command with an interrupt signal. |

Exit code 0 means success.

## Prompts

Without a terminal on stdin, the CLI never prompts. A command that needs confirmation fails with `invalid_argument` and a hint naming `--yes`.
