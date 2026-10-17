# Update imify

```console
$ imify self update
Updated imify from 0.1.0 to 0.2.0 in /home/ada/.cargo/bin.
$ imify self update
imify 0.2.0 is the latest release.
```

`imify self update` updates an install of the imify installer to the latest release. It reads the install receipt that the installer wrote to `$XDG_CONFIG_HOME/imify-cli/imify-cli-receipt.json`, or `~/.config/imify-cli/imify-cli-receipt.json`. It looks up the latest release on GitHub. When that release is newer than the running version, it downloads the release's `imify-cli-installer.sh` and runs it with `/bin/sh` for the install directory in the receipt. When the receipt records that the installer left `PATH` unchanged, the update leaves it unchanged too. The command takes no arguments and does not prompt.

The command sends two requests, both without credentials:

1. `GET https://api.github.com/repos/imify-ai/imify-cli/releases/latest`.
2. `GET https://github.com/imify-ai/imify-cli/releases/download/<tag>/imify-cli-installer.sh`, only when the release is newer. Redirects are followed to `https` URLs only.

The installer then downloads the release archive for the platform. `-v` logs the installer output to stderr.

## JSON output

```console
$ imify self update --json
{"install_prefix":"/home/ada/.cargo","previous_version":"0.1.0","updated":true,"version":"0.2.0"}
```

`updated` is `false` when the running version is the latest release or newer; `version` is then the running version. The binary is in the `bin` directory of `install_prefix`. Schema: [`self-update.json`](../reference/schemas/self-update.json).

## Other install methods

An install without a receipt cannot update itself. The command fails with `self_update_unsupported` and exit code 2, and the hint names the command that updates it:

| Install | Detected by | Hint |
|---|---|---|
| Homebrew | the binary, with links resolved, is below `Cellar/imify-cli/` | `brew upgrade imify-cli` |
| `cargo install` from crates.io | the binary is `$CARGO_HOME/bin/imify` (default `~/.cargo/bin/imify`), and `$CARGO_HOME/.crates2.json` lists `imify-cli` from a registry | `cargo install --locked imify-cli` |
| `cargo install --git` | as above, from a Git repository | `cargo install --locked --git <url> imify-cli` |
| anything else | | the [releases page](https://github.com/imify-ai/imify-cli/releases/latest) |

The installer and `cargo install` both use `~/.cargo/bin`. When the installer wrote a receipt for the binary, `imify self update` uses it.

## Errors

| Code | Exit | When |
|---|---|---|
| `self_update_unsupported` | 2 | No install receipt names the running binary. |
| `self_update_failed` | 1 | The receipt is unreadable or has an unsupported layout, the release has no installer, or the installer failed. Run `imify self update -v` to see the installer output. |
| `rate_limited` | 7 | GitHub answered 403 or 429. |
| `network`, `server` | 7 | GitHub could not be reached or failed. |

## GitHub Enterprise Server or a mirror

```sh
IMIFY_CLI_INSTALLER_GHE_BASE_URL=https://ghe.example.com imify self update
```

With `IMIFY_CLI_INSTALLER_GHE_BASE_URL` set, the release lookup goes to `<base>/api/v3` and the downloads go to `<base>`, for both `imify self update` and the installer. The URL must use `https`, except for `localhost`, `127.0.0.1` and `::1`.
