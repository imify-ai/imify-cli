# Configuration

```console
$ imify config set api-url https://api.example.com
Set api-url to https://api.example.com in /home/ada/.config/imify/config.toml.
$ imify config get api-url
https://api.example.com (config)
```

`imify` targets the hosted service at `https://platform.imify.ai` without configuration. To use another host, pass `--api-url`, set `IMIFY_API_URL`, store `api-url` in the config file, or select a [profile](#profiles).

## API URL

Every command that contacts the API takes the first of:

1. `--api-url <URL>`, a global flag valid before or after any command.
2. `IMIFY_API_URL`, when set and not empty.
3. `api_url` in the selected profile in the config file.
4. `api_url` at the top level of the config file.
5. `https://platform.imify.ai`.

The same rules apply to every source. The URL must use `https`, except on `localhost`, `127.0.0.1` and `::1`. A user name, password, query or fragment is refused. An invalid URL fails with `invalid_argument` and exit code 2, and the message names the source: `--api-url`, `IMIFY_API_URL`, or the profile and the config file's path.

The config file is read when neither `--api-url` nor `IMIFY_API_URL` is set, or when a profile other than `default` is selected.

Logins are stored per host. Switching to another host keeps the login of the previous one; `imify auth login` signs in to the host the API URL selects.

```console
$ imify auth login --api-url https://staging.example.com
$ imify datasets ls --api-url https://staging.example.com
```

## Profiles

```console
$ imify config set api-url https://staging.example.com --profile staging
Set api-url to https://staging.example.com for profile staging in /home/ada/.config/imify/config.toml.
$ imify auth login --profile staging
$ IMIFY_PROFILE=staging imify datasets ls
$ imify config profiles
ACTIVE  NAME     API-URL
*       default  https://platform.imify.ai
        staging  https://staging.example.com
```

A profile is a named set of settings in the config file. The top-level keys form the profile `default`; a `[profiles.<name>]` table holds the keys of another profile. A command reads its settings from the first of:

1. `--profile <NAME>`, a global flag valid before or after any command.
2. `IMIFY_PROFILE`, when set and not empty.
3. `default`.

A profile without `api_url` uses the top-level `api_url`, then `https://platform.imify.ai`. `--api-url` and `IMIFY_API_URL` take precedence over every profile.

When a profile other than `default` is selected, every command that reads a setting fails with `invalid_argument` when the config file has no such profile, even when `--api-url` or `IMIFY_API_URL` sets the host. `imify config set` is the exception: it creates the profile. `imify datasets validate` reads no setting and ignores the profile.

Logins belong to the host, not to the profile. Two profiles with the same host share one login.

A profile name has 1 to 64 characters: ASCII letters, digits, `-` and `_`, starting with a letter or digit. Names are case-sensitive. An invalid name from `--profile` or `IMIFY_PROFILE` fails with `invalid_argument`.

`imify config profiles` lists `default` first, then the other profiles sorted by name. `API-URL` is the URL the profile resolves to without `--api-url` and `IMIFY_API_URL`. `*` marks the selected profile. When the selected profile does not exist, no profile is marked, a warning goes to stderr and the command exits with 0.

```console
$ imify config profiles --json --profile staging
[{"active":false,"api_url":"https://platform.imify.ai","name":"default"},{"active":true,"api_url":"https://staging.example.com","name":"staging"}]
```

Schema: [`config-profiles.json`](../reference/schemas/config-profiles.json).

## Commands

| Command | Effect |
|---|---|
| `imify config get <key>` | Prints the value commands use and its source. |
| `imify config set <key> <value>` | Checks the value and stores it in the selected profile. Creates the profile when it does not exist. |
| `imify config unset <key>` | Removes the key from the selected profile. Removes a profile's table when no key is left in it, and the file when no key and no profile is left. |
| `imify config list` | Prints every key with its value and source. |
| `imify config profiles` | Prints every profile with its API URL and marks the selected one. |

Keys on the command line use dashes, such as `api-url`. An unknown key fails with `invalid_argument`.

`imify config set` checks the value with the rules of its key and writes nothing when it is invalid. It stores an API URL without a trailing slash. When `--api-url` or `IMIFY_API_URL` is set, it writes a warning to stderr, because that value takes precedence over the stored one.

The source is one of:

| Source | Meaning |
|---|---|
| `flag` | `--api-url` |
| `env` | `IMIFY_API_URL` |
| `config` | the config file |
| `default` | the built-in default |

### Output

```console
$ imify config list
KEY      VALUE                    SOURCE
api-url  https://api.example.com  config
$ imify config list --profile staging
KEY      VALUE                        SOURCE
api-url  https://staging.example.com  config (staging)
$ imify config get api-url --profile staging
https://staging.example.com (config, profile staging)
$ imify config get api-url --json
{"key":"api-url","profile":"default","source":"config","value":"https://api.example.com"}
$ imify config list --json
[{"key":"api-url","profile":"default","source":"config","value":"https://api.example.com"}]
$ imify config set api-url https://api.example.com --json
{"key":"api-url","path":"/home/ada/.config/imify/config.toml","profile":"default","value":"https://api.example.com"}
$ imify config unset api-url --json
{"key":"api-url","path":"/home/ada/.config/imify/config.toml","profile":"default","removed":true}
```

In `config get` and `config list`, `profile` names the profile whose table supplied the value, `default` for a top-level key, when `source` is `config`; otherwise it is `null`. In `config set` and `config unset`, `profile` is the profile written to. `removed` is `false` when the profile did not set the key. Schemas: [`config-get.json`](../reference/schemas/config-get.json), [`config-list.json`](../reference/schemas/config-list.json), [`config-set.json`](../reference/schemas/config-set.json), [`config-unset.json`](../reference/schemas/config-unset.json).

## Config file

```toml
api_url = "https://platform.imify.ai"     # profile default

[profiles.staging]
api_url = "https://staging.example.com"

[profiles.local]
api_url = "http://localhost:8000"
```

The file is `config.toml` in the configuration directory, the same on macOS and Linux:

1. `IMIFY_CONFIG_DIR`, when set and not empty.
2. `$XDG_CONFIG_HOME/imify`, when `XDG_CONFIG_HOME` is an absolute path.
3. `~/.config/imify`.

The login file `credentials.json` uses the same directory.

| Key | Value |
|---|---|
| `api_url` | The API host, such as `https://api.example.com`. |

The top level and each `[profiles.<name>]` table hold these keys only. A profile exists when its table exists, even an empty one; `default` always exists. Each of the following fails every command that reads the file with `invalid_argument`, and the message names the file:

- a file that is not valid TOML;
- a key not in the table, at the top level or in a profile; the message also names the key and the profile;
- a profile name that breaks the naming rule;
- a `[profiles.default]` table, because the top-level keys form `default`.

`imify config set` and `imify config unset` write the top-level keys first, then the profiles sorted by name. `imify` 0.1.0 rejects a file that has a `profiles` table.

`imify config set` creates the directory with mode `0700` and writes the file with mode `0600`, replacing it atomically. `imify config set` and `imify config unset` rewrite the file, so comments in it are not kept. The CLI refuses:

- a file that other users can write to, that belongs to another user, that is a symbolic link, or that is larger than 1 MiB;
- a directory that belongs to another user or that other users can write to.

A refused file or directory fails with `internal` and exit code 1; the message says how to fix the mode.

## Update check

```sh
export IMIFY_NO_UPDATE_CHECK=1
```

`IMIFY_NO_UPDATE_CHECK` with any value except the empty string turns off the [update hint](self-update.md#update-hint) and its request to GitHub. `CI`, `BUILD_NUMBER` and `RUN_ID` turn it off the same way.

## Errors

| Code | Exit | Cause |
|---|---|---|
| `invalid_argument` | 2 | An unknown key, an invalid value for `config set`, an invalid API URL from any source, a config file that is not valid TOML or is invalid as described above, an invalid profile name from `--profile` or `IMIFY_PROFILE`, a selected profile the config file does not have, or `HOME` is not an absolute path while `IMIFY_CONFIG_DIR` and `XDG_CONFIG_HOME` are unset. |
| `internal` | 1 | The config file or its directory is refused or cannot be read or written. |

An unknown profile names the profile, its source and the file:

```console
$ IMIFY_PROFILE=prod imify config get api-url
error: IMIFY_PROFILE selects the profile 'prod', which is not in /home/ada/.config/imify/config.toml.
hint: Run `imify config profiles` to list the profiles, `imify config set api-url <URL> --profile prod` to create it, or unset IMIFY_PROFILE.
```
