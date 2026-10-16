# Check the setup

```console
$ imify doctor
imify-cli 0.1.0 (macos aarch64)

ok    api_url       https://api.example.com
ok    health        Reachable in 84 ms.
ok    readiness     Database ok, storage ok.
warn  clock         The local clock is 95 s behind the server.
                    hint: Enable time synchronization in the system settings.
ok    token         IMIFY_TOKEN is set.
ok    token_expiry  Expires at 2026-10-16T12:00:00Z.
ok    account       Ada Lovelace (admin).
```

`imify doctor` checks the CLI configuration, the API at `IMIFY_API_URL` and the access token in `IMIFY_TOKEN`, then writes a report to stdout. It contacts only the host in `IMIFY_API_URL` and sends three requests: `GET /api/health` and `GET /api/health/ready` without a token, then `GET /api/v1/me` with the token. Requests are not retried and time out after 10 s without progress.

Each line is one check: status, name and message. A `fail` or `warn` line with a hint is followed by a `hint:` line.

| Status | Meaning |
|---|---|
| `ok` | The check passed. |
| `warn` | The check passed with a problem that does not stop other commands. |
| `fail` | The check failed. Commands that depend on it fail too. |
| `skip` | The check did not run because a check it depends on did not pass. |

## Checks

| Name | Fails when (error code) | Warns when | Skipped when |
|---|---|---|---|
| `api_url` | `IMIFY_API_URL` is unset, not a URL, has a user name, password, query or fragment, or uses `http` for a host other than `localhost`, `127.0.0.1` or `::1` (`invalid_argument`). | | |
| `health` | `GET /api/health` fails (`network`, `server`, `not_found`, `rate_limited`, `internal`), or its `status` is not `ok` (`server`). | | `api_url` failed. |
| `readiness` | The API reports a `status` other than `ok` (`server`). The message names the state of the database and the storage. Also fails when the request fails. | | `health` did not pass. |
| `clock` | | The local clock differs from the response's `Date` header by more than 60 s. | `health` did not pass, or the response has no `Date` header. |
| `token` | Neither `IMIFY_TOKEN` nor a stored login for the host exists (`auth_required`), `IMIFY_TOKEN` is not valid Unicode (`invalid_argument`), or the stored login cannot be refreshed (`auth_expired`). | | |
| `token_expiry` | The token's `exp` claim is in the past (`auth_rejected`). | The token expires within 5 min. | `token` failed, or the token is not a JWT with a numeric `exp`. |
| `account` | `GET /api/v1/me` fails (`auth_rejected`, `permission_denied`, `network`, ...). | | `token` or `health` did not pass. |

The `exp` claim is read without verifying the token's signature. The API decides whether it accepts the token; the `account` check reports that decision.

## Exit codes

| Exit | Meaning |
|---|---|
| 0 | No check failed. Warnings are allowed. |
| 2, 3, 4, 7 | At least one check failed. The exit code belongs to the error code of the first failing check in report order. |

With a failing check, the report goes to stdout and the error to stderr. The error message names every failing check, and the hint is the first failing check's hint:

```console
$ imify doctor
...
error: 2 checks failed: token, account.
hint: Set `IMIFY_TOKEN` to an access token.
```

## JSON output

```console
$ imify doctor --json
{"api_url":"https://api.example.com","checks":[{"code":null,"hint":null,"message":"https://api.example.com","name":"api_url","status":"ok"}],"cli_version":"0.1.0","clock_skew_ms":-95000,"latency_ms":84,"platform":{"arch":"aarch64","os":"macos"},"readiness":{"db":"ok","status":"ok","storage":"ok"},"token_expires_at":"2026-10-16T12:00:00Z","token_source":"env","user":{"name":"Ada Lovelace","role":"admin","subject":"user-1234"}}
```

The example shows one of the seven checks.

| Field | Content |
|---|---|
| `cli_version` | Version of the CLI. |
| `platform` | `os` and `arch` the CLI was built for. |
| `api_url` | The value of `IMIFY_API_URL`. |
| `latency_ms` | Duration of `GET /api/health` in milliseconds. |
| `clock_skew_ms` | The server's `Date` minus the local time in milliseconds. Negative when the local clock is ahead. The `Date` header has a resolution of 1 s. |
| `readiness` | `status`, `db` and `storage` from `GET /api/health/ready`. |
| `token_source` | `env` when the token comes from `IMIFY_TOKEN`, `stored` when it comes from the stored login of the host. |
| `token_expires_at` | The token's `exp` claim, RFC 3339 in UTC. |
| `user` | `subject`, `name` and `role` from `GET /api/v1/me`. |
| `checks` | `name`, `status`, `message`, `code` and `hint` of each check, in report order. `code` is set for `fail` only. |

Every field except `cli_version`, `platform` and `checks` is `null` when its check did not produce it. The report contains no part of the token. Schema: [`doctor.json`](../reference/schemas/doctor.json).
