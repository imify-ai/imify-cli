# Authentication

```console
$ imify auth login --api-url https://api.example.com
Signing in to https://api.example.com through login.example.com. Opening the browser.
https://login.example.com/oauth2/authorize?...
Logged in to https://api.example.com as Ada Lovelace (admin).
Credentials stored in the keychain.
```

`imify auth login` signs in to the API host and stores the login for that host. Every other command then uses it. One login is stored per host. The host is `https://platform.imify.ai` unless `--api-url`, `IMIFY_API_URL` or the config file selects another; see [Configuration](configuration.md).

> **Log in only to an API URL you trust.** The API host names the identity provider you sign in at, and every command sends your token to that host. Before you sign in, check both hosts in the first line of `imify auth login`.

## Log in

`imify auth login` reads the host's identity provider from `<API URL>/config.json`. It writes the API host, the identity provider's host and the sign-in URL to stderr, then opens the URL in the browser. After the sign-in, the browser returns to `http://localhost:28753/callback`, or to port 28754 or 28755 when the first port is taken. The command waits up to 5 minutes. It then confirms the account with the API and stores the login.

When the browser does not open, open the URL from stderr by hand.

### Without a browser

```console
$ imify auth login --no-browser
Signing in to https://api.example.com through login.example.com. Open this URL in a browser:
https://login.example.com/oauth2/authorize?...
Paste the URL from the browser's address bar: http://localhost:28753/callback?code=...&state=...
Logged in to https://api.example.com as Ada Lovelace (admin).
Credentials stored in the keychain.
```

Open the URL on any machine. After the sign-in, the browser shows an error page for `localhost:28753`, because nothing listens there. Copy the full address from the address bar and paste it at the prompt.

The prompt accepts up to 16 KiB; backspace does not edit the pasted line. Ctrl-C cancels the login with `interrupted` and exit code 130; Ctrl-D on an empty line ends the input. Input after the first line is discarded. `--no-browser` needs a terminal on stdin; without one, the command fails with `invalid_argument`.

### Output

```console
$ imify auth login --json
{"host":"https://api.example.com","subject":"user-1234","name":"Ada Lovelace","role":"admin","storage":"keychain","storage_fallback":false}
```

| Field | Type | Meaning |
|---|---|---|
| `host` | string | The API URL the login belongs to. |
| `subject` | string | Stable user id. |
| `name` | string | Display name or email address. |
| `role` | string | Platform role. |
| `storage` | string | `keychain` or `file`. |
| `storage_fallback` | boolean | `true` when `auto` stored the login in the file because the machine has no keychain service. |

Schema: [`auth-login.json`](../reference/schemas/auth-login.json). The sign-in instructions and warnings go to stderr. The warning about a login stored in the file because there is no keychain service is written with `--quiet` too.

Control characters in text from the API and the identity provider, such as names and error messages, are written escaped, for example `\u{1b}`.

## Log out

```console
$ imify auth logout
Logged out of https://api.example.com.
Tokens issued before the logout stay valid until they expire.
```

`imify auth logout` revokes the refresh token at the identity provider and removes the login from the keychain and the file. The ID token the API accepts cannot be revoked: a copy of it stays valid until it expires.

When revocation fails or the identity provider does not support it, the command writes a warning, removes the login, and reports `revoked: false`. When the stored login cannot be read, for example because the file has the wrong mode, the command writes a warning, skips the revocation and removes the login. With `auto`, it removes the login from both stores and reports an error of either store afterwards. Without a login, it succeeds and reports `removed: false`.

```console
$ imify auth logout --json
{"host":"https://api.example.com","removed":true,"revoked":true}
```

Schema: [`auth-logout.json`](../reference/schemas/auth-logout.json).

## Token precedence

1. `IMIFY_TOKEN`, when set and not empty.
2. The stored login for the API host.

Without either, a command fails with `auth_required`. `imify auth login` warns when `IMIFY_TOKEN` is set, because the variable takes precedence over the new login. When the API rejects `IMIFY_TOKEN` and a login is stored for the host, the `auth_rejected` hint is ``Unset `IMIFY_TOKEN` to use the stored login, or set it to a valid, unexpired token.``.

A stored login holds an ID token and a refresh token. The CLI renews the ID token when less than 60 s remain before it expires. When the identity provider refuses the renewal, the command fails with `auth_expired`; run `imify auth login` again.

## Where the login is stored

| `IMIFY_CREDENTIAL_STORE` | Store |
|---|---|
| `auto` (default) | The OS keychain: the macOS Keychain, or the Secret Service on Linux. The file only when the machine has no keychain service, with a warning that names the path. |
| `keychain` | The OS keychain only. Without one, `imify auth login` fails with `internal`. |
| `file` | The file only. Use it on machines without a Secret Service, such as containers and robots. |

The keychain entry has the service `imify-cli` and the host as account. After `auto` saves a login in the keychain, it removes the host's entry from the file.

A keychain that exists but refuses access, such as a locked keyring on Linux, is an error with code `internal` and the hint ``Unlock your keyring, or set `IMIFY_CREDENTIAL_STORE=file`.``. The CLI then neither writes the login to the file nor reads an older login from it.

The file is `credentials.json` in `IMIFY_CONFIG_DIR`, otherwise in `$XDG_CONFIG_HOME/imify` when `XDG_CONFIG_HOME` is an absolute path, otherwise in `~/.config/imify`, on macOS and Linux alike. It has mode `0600`, and a directory the CLI creates has mode `0700`. The CLI refuses:

- a file that other users can access, that belongs to another user, that is a symbolic link, or that is larger than 1 MiB;
- a directory that belongs to another user or that other users can write to.

Writers take a lock on `credentials.json.lock` in the same directory.

## Security considerations

- The file stores the ID token and the refresh token unencrypted. The root user and backups of the home directory can read them. Prefer the keychain where one exists.
- On Linux, every process in the same desktop session can read entries from the Secret Service while the keyring is open.
- On macOS, the Keychain can ask for permission again after an upgrade of `imify`, because the binary changed. Allow access to keep using the stored login.
- `IMIFY_TOKEN` is not bound to a host: every command sends it to the API host. Other processes of the same user can read it from the environment.
- On a shared machine, run `imify auth logout` when you are done. Logout revokes the refresh token; the current ID token stays valid until it expires.
- `-v` and `-vv` log the imify crates only; HTTP and TLS libraries log at `warn` and above. Tokens never appear in the output.

## Environment

| Variable | Effect |
|---|---|
| `IMIFY_API_URL` | The API host. `--api-url` takes precedence over it. Must use `https`, except for `localhost`, `127.0.0.1` and `::1`. A user name, password, query or fragment is refused. |
| `IMIFY_TOKEN` | A token that takes precedence over the stored login. |
| `IMIFY_CREDENTIAL_STORE` | `auto`, `keychain` or `file`. |
| `IMIFY_CONFIG_DIR` | The directory of `credentials.json` and `config.toml`. |
| `XDG_CONFIG_HOME` | When `IMIFY_CONFIG_DIR` is unset and `XDG_CONFIG_HOME` is an absolute path, the files are in `$XDG_CONFIG_HOME/imify`. |
| `IMIFY_OIDC_ISSUER` | The identity provider's issuer URL, replacing `authority` from `config.json`. Must use `https`, except on loopback hosts. |
| `IMIFY_OIDC_CLIENT_ID` | The client id, replacing `clientId` from `config.json`. |

With both `IMIFY_OIDC_ISSUER` and `IMIFY_OIDC_CLIENT_ID` set, `imify auth login` does not read `config.json`.

## Errors

| Code | Exit | Cause |
|---|---|---|
| `invalid_argument` | 2 | The API URL or the issuer is not `https` on a remote host, or the API URL has a user name, password, query or fragment; the host publishes no `config.json` and the overrides are unset; `IMIFY_CREDENTIAL_STORE` is unsupported; the credentials file is needed and `HOME` is not an absolute path while `IMIFY_CONFIG_DIR` and `XDG_CONFIG_HOME` are unset; `--no-browser` without a terminal; the pasted text is not the redirect URL. |
| `auth_required` | 3 | There is no login for the host, and `IMIFY_TOKEN` is not set. |
| `auth_rejected` | 3 | The API rejected the token. |
| `auth_expired` | 3 | The stored login has expired or was revoked. |
| `login_failed` | 3 | The sign-in was denied, no redirect arrived within 5 minutes, all three redirect ports are taken, or the identity provider answered with an error. After a timeout or with all ports taken, the hint is `imify auth login --no-browser`. |
| `internal` | 1 | The keychain refuses access or fails, or the credentials file or its directory cannot be used. |
| `interrupted` | 130 | Ctrl-C at the paste prompt. |

All error codes: [JSON output and exit codes](../reference/json-output.md#errors).
