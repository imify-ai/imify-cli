# Authentication

```console
$ export IMIFY_API_URL=https://api.example.com
$ imify login
Signing in at login.example.com. Opening the browser.
https://login.example.com/oauth2/authorize?...
Logged in to https://api.example.com as Ada Lovelace (admin).
Credentials stored in the keychain.
```

`imify login` signs in to the host in `IMIFY_API_URL` and stores the login for that host. Every other command then uses it. One login is stored per host.

## Log in

`imify login` reads the host's identity provider from `<IMIFY_API_URL>/config.json`, writes the identity provider's host and the sign-in URL to stderr, and opens the URL in the browser. After the sign-in, the browser returns to `http://localhost:28753/callback`, or to port 28754 or 28755 when the first port is taken. The command waits up to 5 minutes. It then confirms the account with the API and stores the login.

When the browser does not open, open the URL from stderr by hand.

### Without a browser

```console
$ imify login --no-browser
Signing in at login.example.com. Open this URL in a browser:
https://login.example.com/oauth2/authorize?...
Paste the URL from the browser's address bar: http://localhost:28753/callback?code=...&state=...
Logged in to https://api.example.com as Ada Lovelace (admin).
Credentials stored in the keychain.
```

Open the URL on any machine. After the sign-in, the browser shows an error page for `localhost:28753`, because nothing listens there. Copy the full address from the address bar and paste it at the prompt. The prompt accepts up to 16 KiB; backspace does not edit the pasted line. `--no-browser` needs a terminal on stdin; without one, the command fails with `invalid_argument`.

### Output

```console
$ imify login --json
{"host":"https://api.example.com","subject":"user-1234","name":"Ada Lovelace","role":"admin","storage":"keychain"}
```

| Field | Type | Meaning |
|---|---|---|
| `host` | string | The API URL the login belongs to. |
| `subject` | string | Stable user id. |
| `name` | string | Display name or email address. |
| `role` | string | Platform role. |
| `storage` | string | `keychain` or `file`. |

Schema: [`login.json`](../reference/schemas/login.json). The sign-in instructions and warnings go to stderr.

## Log out

```console
$ imify logout
Logged out of https://api.example.com.
Tokens issued before the logout stay valid until they expire.
```

`imify logout` revokes the refresh token at the identity provider and removes the login from the keychain and the file. The ID token the API accepts cannot be revoked: a copy of it stays valid until it expires. When revocation fails or the identity provider does not support it, the command writes a warning, removes the login, and reports `revoked: false`. Without a login, it succeeds and reports `removed: false`.

```console
$ imify logout --json
{"host":"https://api.example.com","removed":true,"revoked":true}
```

Schema: [`logout.json`](../reference/schemas/logout.json).

## Token precedence

1. `IMIFY_TOKEN`, when set and not empty.
2. The stored login for the host in `IMIFY_API_URL`.

Without either, a command fails with `auth_required`. `imify login` warns when `IMIFY_TOKEN` is set, because the variable takes precedence over the new login.

A stored login holds an ID token and a refresh token. The CLI renews the ID token when less than 60 s remain before it expires. When the identity provider refuses the renewal, the command fails with `auth_expired`; run `imify login` again.

## Where the login is stored

| `IMIFY_CREDENTIAL_STORE` | Store |
|---|---|
| `auto` (default) | The OS keychain: the macOS Keychain, or the Secret Service on Linux. The file when no keychain is available, with a warning that names the path. |
| `keychain` | The OS keychain only. Without one, `imify login` fails with `internal`. |
| `file` | The file only. Use it on machines without a Secret Service, such as containers and robots. |

The keychain entry has the service `imify-cli` and the host as account. The file is `credentials.json` in `IMIFY_CONFIG_DIR`, or in the platform's configuration directory: `~/Library/Application Support/ai.imify.imify` on macOS, `~/.config/imify` on Linux. It has mode `0600`, and a directory the CLI creates has mode `0700`. The CLI refuses a file that other users can read.

## Environment

| Variable | Effect |
|---|---|
| `IMIFY_API_URL` | The API host. Must use `https`, except for `localhost`, `127.0.0.1` and `::1`. |
| `IMIFY_TOKEN` | A token that takes precedence over the stored login. |
| `IMIFY_CREDENTIAL_STORE` | `auto`, `keychain` or `file`. |
| `IMIFY_CONFIG_DIR` | The directory of `credentials.json`. |
| `IMIFY_OIDC_ISSUER` | The identity provider's issuer URL, replacing `authority` from `config.json`. Must use `https`, except on loopback hosts. |
| `IMIFY_OIDC_CLIENT_ID` | The client id, replacing `clientId` from `config.json`. |

With both `IMIFY_OIDC_ISSUER` and `IMIFY_OIDC_CLIENT_ID` set, `imify login` does not read `config.json`.

## Errors

| Code | Exit | Cause |
|---|---|---|
| `invalid_argument` | 2 | `IMIFY_API_URL` or the issuer is not `https` on a remote host; the host publishes no `config.json` and the overrides are unset; `IMIFY_CREDENTIAL_STORE` is unsupported; `--no-browser` without a terminal; the pasted text is not the redirect URL. |
| `auth_required` | 3 | There is no login for the host, and `IMIFY_TOKEN` is not set. |
| `auth_rejected` | 3 | The API rejected the token. |
| `auth_expired` | 3 | The stored login has expired or was revoked. |
| `login_failed` | 3 | The sign-in was denied, no redirect arrived within 5 minutes, all three redirect ports are taken, or the identity provider answered with an error. After a timeout or with all ports taken, the hint is `imify login --no-browser`. |
| `internal` | 1 | The keychain or the credentials file cannot be used. |

All error codes: [JSON output and exit codes](../reference/json-output.md#errors).
