# 0016. Self update and update check

Status: Accepted
Date: 2026-10-17

## Context
The dist shell installer installs `imify` into `$CARGO_HOME/bin`, or `~/.cargo/bin`, and writes an install receipt to `$XDG_CONFIG_HOME/imify-cli/imify-cli-receipt.json`, or `~/.config/imify-cli/imify-cli-receipt.json`. The receipt records the install prefix, the binaries, whether the installer changed `PATH`, and the repository the release came from. The installer reads `IMIFY_CLI_INSTALL_DIR` as the install prefix and `IMIFY_CLI_NO_MODIFY_PATH=1` to leave the shell profile unchanged.

`imify` is also installed with `cargo install`, and releases will publish a Homebrew formula `imify-cli`. Those installs have no receipt and update through their own tool.

`imify-cli` does no network I/O of its own ([0001](0001-rust-workspace-sdk-and-cli.md)). The SDK reads no environment variables except those a `TokenSource` documents. Every request sends `User-Agent: imify-cli/<version>` ([0015](0015-generated-sdk-api-layer.md)). [0011](0011-no-telemetry.md) allows one contact besides the configured host: the update check against GitHub releases, which `IMIFY_NO_UPDATE_CHECK=1` disables.

## Decision

### Self update
- The SDK has a module `imify::update` behind the non-default feature `self-update`, which `imify-cli` enables. It reads the install receipt, looks up the latest release, downloads the release's shell installer and runs it. It uses the SDK's HTTP client library and reads no environment variables. The CLI resolves every setting, such as the receipt's directory, and passes it in.
- The release lookup is `GET https://api.github.com/repos/imify-ai/imify-cli/releases/latest` with `User-Agent: imify-cli/<version>` and `Accept: application/vnd.github+json`. It sends no credentials, no cookies, no query and no body.
- The installer comes from `https://github.com/imify-ai/imify-cli/releases/download/<tag>/imify-cli-installer.sh`, the release of the same repository that the lookup returned. Redirects are followed to `https` URLs only. A download that does not start with `#!` is not run. `/bin/sh` runs the installer with `IMIFY_CLI_INSTALL_DIR` set to the receipt's install prefix and, when the receipt records that the installer left `PATH` unchanged, `IMIFY_CLI_NO_MODIFY_PATH=1`. Installer variables that would change the download source, the install directory or the receipt are removed from its environment. The installer's output is captured and logged at debug level.
- `self` is a command noun, singular like `auth` and `config`, because it is not an API resource. `imify self update` updates an installer install when the latest release is newer than the running version. It does not prompt.
- Without a receipt that names the running executable, `imify self update` fails with `self_update_unsupported` (exit 2). The hint names the update command of the install method, detected from the executable path: `brew upgrade imify-cli` for a path below `Cellar/imify-cli/`, `cargo install --locked imify-cli` or `cargo install --locked --git <url> imify-cli` for an executable that `.crates2.json` in `CARGO_HOME` lists, and the releases page otherwise. A receipt wins over the other methods.
- An unreadable receipt, a release without an installer, or a failing installer is `self_update_failed` (exit 1). A failed lookup or download uses `network`, `server` or `rate_limited` (exit 7).
- `IMIFY_CLI_INSTALLER_GHE_BASE_URL`, which the installer also reads, points the lookup at `<base>/api/v3` and the download at `<base>`, for GitHub Enterprise Server, a mirror, or tests.

### Update hint
- After a successful command, the CLI writes one line to stderr when a newer release exists: ``imify 0.2.0 is available (installed: 0.1.0). Run `imify self update`.`` The command in the hint follows the install method, as in the `self_update_unsupported` hint.
- The check uses the same release lookup as `imify self update`. It runs only when every one of these holds:
  - The output is human: no `--json` and no `IMIFY_OUTPUT=json`.
  - `--quiet` is not set.
  - stderr is a terminal.
  - `IMIFY_NO_UPDATE_CHECK`, `CI`, `BUILD_NUMBER` and `RUN_ID` are unset or empty.
  - The command is neither `imify self update` nor `imify doctor`. `imify doctor` contacts only the API host and shows no hint. Bare `imify`, `--help`, `--version` and usage errors never check.
- The check sends at most one request per 24 hours. Before the request, the CLI writes the time to `$XDG_STATE_HOME/imify/update-check.json`, or `~/.local/state/imify/update-check.json`, with mode `0600`. The file stores nothing else.
- The check runs on its own thread with a 1 s deadline. After a successful command, the CLI waits for it at most until that deadline. A failed command shows no hint and does not wait. Any failure of the check is dropped from the output, logged at debug level, and leaves the exit code unchanged.

## Consequences
- An installer install updates with one command, into the prefix and with the `PATH` setting it was installed with.
- With the check enabled, GitHub receives the client's IP address, the time of the request and the installed version in `User-Agent`, at most once per day. Users who disable the check, scripts, `--json` runs and CI runs send nothing.
- An interactive command shorter than 1 s can take up to 1 s longer, at most once per day.
- The SDK gains a concern outside the platform API. SDK users who do not enable `self-update` do not compile it.
- Installs through `cargo install` or Homebrew update through those tools. A new channel adds an install method.
- The installer is trusted as much as the documented `curl … | sh` install: TLS to GitHub, no signature check.
