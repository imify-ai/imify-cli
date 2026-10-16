# Configuration

```console
$ imify config set api-url https://api.example.com
Set api-url to https://api.example.com in /home/ada/.config/imify/config.toml.
$ imify config get api-url
https://api.example.com (config)
```

`imify` targets the hosted service at `https://platform.imify.ai` without configuration. To use another host, pass `--api-url`, set `IMIFY_API_URL`, or store `api-url` in the config file.

## API URL

Every command that contacts the API takes the first of:

1. `--api-url <URL>`, a global flag valid before or after any command.
2. `IMIFY_API_URL`, when set and not empty.
3. `api_url` in the config file.
4. `https://platform.imify.ai`.

The same rules apply to every source. The URL must use `https`, except on `localhost`, `127.0.0.1` and `::1`. A user name, password, query or fragment is refused. An invalid URL fails with `invalid_argument` and exit code 2, and the message names the source: `--api-url`, `IMIFY_API_URL` or the config file's path.

The config file is read only when neither `--api-url` nor `IMIFY_API_URL` is set.

Logins are stored per host. Switching to another host keeps the login of the previous one; `imify auth login` signs in to the host the API URL selects.

```console
$ imify auth login --api-url https://staging.example.com
$ imify datasets ls --api-url https://staging.example.com
```

## Commands

| Command | Effect |
|---|---|
| `imify config get <key>` | Prints the value commands use and its source. |
| `imify config set <key> <value>` | Checks the value and stores it in the config file. |
| `imify config unset <key>` | Removes the key from the config file. Removes the file when no key is left. |
| `imify config list` | Prints every key with its value and source. |

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
$ imify config get api-url --json
{"key":"api-url","source":"config","value":"https://api.example.com"}
$ imify config list --json
[{"key":"api-url","source":"config","value":"https://api.example.com"}]
$ imify config set api-url https://api.example.com --json
{"key":"api-url","path":"/home/ada/.config/imify/config.toml","value":"https://api.example.com"}
$ imify config unset api-url --json
{"key":"api-url","path":"/home/ada/.config/imify/config.toml","removed":true}
```

`removed` is `false` when the file did not set the key. Schemas: [`config-get.json`](../reference/schemas/config-get.json), [`config-list.json`](../reference/schemas/config-list.json), [`config-set.json`](../reference/schemas/config-set.json), [`config-unset.json`](../reference/schemas/config-unset.json).

## Config file

```toml
api_url = "https://api.example.com"
```

The file is `config.toml` in the configuration directory, the same on macOS and Linux:

1. `IMIFY_CONFIG_DIR`, when set and not empty.
2. `$XDG_CONFIG_HOME/imify`, when `XDG_CONFIG_HOME` is an absolute path.
3. `~/.config/imify`.

The login file `credentials.json` uses the same directory.

| Key | Value |
|---|---|
| `api_url` | The API host, such as `https://api.example.com`. |

The file holds these keys only. A key not in the table, or a file that is not valid TOML, fails every command that reads the file with `invalid_argument`; the message names the file and the key.

`imify config set` creates the directory with mode `0700` and writes the file with mode `0600`, replacing it atomically. `imify config set` and `imify config unset` rewrite the file, so comments in it are not kept. The CLI refuses:

- a file that other users can write to, that belongs to another user, that is a symbolic link, or that is larger than 1 MiB;
- a directory that belongs to another user or that other users can write to.

A refused file or directory fails with `internal` and exit code 1; the message says how to fix the mode.

## Errors

| Code | Exit | Cause |
|---|---|---|
| `invalid_argument` | 2 | An unknown key, an invalid value for `config set`, an invalid API URL from any source, a config file that is not valid TOML or has an unknown key, or `HOME` is not an absolute path while `IMIFY_CONFIG_DIR` and `XDG_CONFIG_HOME` are unset. |
| `internal` | 1 | The config file or its directory is refused or cannot be read or written. |
