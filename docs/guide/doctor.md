# Check the setup

```console
$ imify doctor
imify-cli 0.1.0 (macos aarch64)

ok    api_url       https://api.example.com (IMIFY_API_URL)
ok    health        Reachable in 84 ms.
ok    readiness     Database ok, storage ok.
warn  clock         The local clock is 95 s behind the server.
                    hint: Enable time synchronization in the system settings.
ok    token         IMIFY_TOKEN is set.
ok    token_expiry  Expires at 2026-10-16T12:00:00Z.
ok    account       Ada Lovelace (admin).
ok    oidc          Issuer https://login.example.com, client client-1.
```

`imify doctor` checks the CLI configuration, the API, the access token and the identity provider settings, then writes a report to stdout. It contacts the API host, which `--api-url`, `IMIFY_API_URL`, the config file or the default selects (see [Configuration](configuration.md)), and sends four requests: `GET /api/health` and `GET /api/health/ready` without a token, `GET /api/v1/me` with the token, then `GET /config.json` without a token. With both `IMIFY_OIDC_ISSUER` and `IMIFY_OIDC_CLIENT_ID` set, it does not request `config.json`. A stored login whose ID token expires within 60 s is refreshed before `GET /api/v1/me`, with one request to the identity provider's token endpoint. Requests are not retried and time out after 10 s without progress.

With a stored login, the `token` line names where the login is stored and whether it has a refresh token:

```console
ok    token         A login for https://api.example.com is stored in the keychain, with a refresh token.
ok    token_expiry  Expires at 2026-10-16T12:00:00Z.
```

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
| `api_url` | The API URL is not a URL, has a user name, password, query or fragment, or uses `http` for a host other than `localhost`, `127.0.0.1` or `::1` (`invalid_argument`). The message names the source. | | |
| `health` | `GET /api/health` fails (`network`, `server`, `not_found`, `rate_limited`, `internal`), or its `status` is not `ok` (`server`). | | `api_url` failed. |
| `readiness` | The API reports a `status` other than `ok` (`server`). The message names the state of the database and the storage. Also fails when the request fails. | | `health` did not pass. |
| `clock` | | The local clock differs from the response's `Date` header by more than 60 s. | `health` did not pass, or the response has no `Date` header. |
| `token` | Neither `IMIFY_TOKEN` nor a stored login for the host exists (`auth_required`; the message names the host and the store that was searched), `IMIFY_TOKEN` is not valid Unicode (`invalid_argument`), `IMIFY_CREDENTIAL_STORE` is unsupported (`invalid_argument`), or the credential store cannot be read (`internal`). | | |
| `token_expiry` | `IMIFY_TOKEN`: the token's `exp` claim is in the past (`auth_rejected`). Stored login: the ID token has expired and the login has no refresh token (`auth_expired`). | The token expires within 5 min. For a stored login, only when it has no refresh token. | `token` failed, or `IMIFY_TOKEN` is not a JWT with a numeric `exp`. |
| `account` | `GET /api/v1/me` fails (`auth_rejected`, `permission_denied`, `network`, ...), or the stored login cannot be refreshed (`auth_expired`). | | `token` or `health` did not pass. |
| `oidc` | Never. | The host publishes no identity provider settings in `config.json` and the overrides are unset, the request fails, or the issuer is neither `https` nor `http` on `localhost`, `127.0.0.1` or `::1`. | `api_url` failed, or `health` did not pass and the two overrides are not both set. |

The `exp` claim is read without verifying the token's signature. The API decides whether it accepts the token; the `account` check reports that decision.

For a stored login, `token` and `token_expiry` read the login from the credential store and do not refresh it. A stored login with a refresh token passes `token_expiry` even when its ID token has expired, because the refresh token renews it; the `account` check refreshes it as every other command does. The hint for a stored login that is rejected or expired is "Run `imify auth login`.".

The `oidc` check reports the settings `imify auth login` uses: `IMIFY_OIDC_ISSUER` and `IMIFY_OIDC_CLIENT_ID`, completed from the host's `config.json` when one of them is unset. Its problems affect `imify auth login` only, so it never fails.

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
{"api_url":"https://api.example.com","api_url_source":"env","checks":[{"code":null,"hint":null,"message":"https://api.example.com (IMIFY_API_URL)","name":"api_url","status":"ok"}],"cli_version":"0.1.0","clock_skew_ms":-95000,"config_dir":"/home/robot/.config/imify","credential_store":"auto","latency_ms":84,"login":{"has_refresh_token":true,"storage":"keychain"},"oidc":{"client_id":"client-1","issuer":"https://login.example.com","issuer_https":true},"platform":{"arch":"aarch64","os":"macos"},"readiness":{"db":"ok","status":"ok","storage":"ok"},"token_expires_at":"2026-10-16T12:00:00Z","token_source":"stored","user":{"name":"Ada Lovelace","role":"admin","subject":"user-1234"}}
```

The example shows one of the eight checks.

| Field | Content |
|---|---|
| `cli_version` | Version of the CLI. |
| `platform` | `os` and `arch` the CLI was built for. |
| `api_url` | The API URL the checks used. |
| `api_url_source` | Where the API URL comes from: `flag`, `env`, `config` or `default`. |
| `latency_ms` | Duration of `GET /api/health` in milliseconds. |
| `clock_skew_ms` | The server's `Date` minus the local time in milliseconds. Negative when the local clock is ahead. The `Date` header has a resolution of 1 s. |
| `readiness` | `status`, `db` and `storage` from `GET /api/health/ready`. |
| `token_source` | `env` when the token comes from `IMIFY_TOKEN`, `stored` when it comes from the stored login of the host. |
| `token_expires_at` | The token's `exp` claim, RFC 3339 in UTC. For a stored login, the expiry of the stored ID token, before any refresh. |
| `user` | `subject`, `name` and `role` from `GET /api/v1/me`. |
| `credential_store` | The store `IMIFY_CREDENTIAL_STORE` selects: `auto`, `keychain` or `file`. `null` when the value is unsupported. |
| `config_dir` | The directory of `config.toml` and `credentials.json`: `IMIFY_CONFIG_DIR`, else `$XDG_CONFIG_HOME/imify`, else `~/.config/imify`. `null` when it cannot be located. |
| `login` | Set when `token_source` is `stored`. `storage` is `keychain` or `file`, as in `imify auth login --json`. `has_refresh_token` is `true` when the login can be refreshed. |
| `oidc` | `issuer`, `client_id`, and `issuer_https` (`true` when the issuer's scheme is `https`) of the settings the `oidc` check found. |
| `checks` | `name`, `status`, `message`, `code` and `hint` of each check, in report order. `code` is set for `fail` only. |

Every field except `cli_version`, `platform` and `checks` is `null` when its check did not produce it. The report contains no part of the token. Schema: [`doctor.json`](../reference/schemas/doctor.json).
