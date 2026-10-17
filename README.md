# imify

Command-line client and Rust SDK for the [imify](https://imify.ai) platform: manage robot learning datasets from a terminal, a script or an AI agent.

```sh
imify auth login                                # sign in through the browser
imify datasets validate ./pick-cube             # check a LeRobot v3 or v2.1 dataset offline
imify datasets upload ./pick-cube --task "pick the cube"
imify datasets ls
imify doctor                                    # check the configuration, the API and the token
```

Every command accepts `--json` for machine-readable output on stdout and returns a [documented exit code](docs/dev/adr/0003-output-contract.md). Set `IMIFY_OUTPUT=json` to make it the default, for example in an agent's environment.

The [CLI reference](docs/reference/cli.md) lists the `--help` text of every command. In a clone of the repository, `man ./docs/man/imify.1` opens the man page.

## Install

On macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/imify-ai/imify-cli/releases/latest/download/imify-cli-installer.sh | sh
```

The installer places `imify` in `$CARGO_HOME/bin` (default `~/.cargo/bin`) and adds that directory to `PATH` in your shell profile. To leave the shell profile unchanged, set `IMIFY_CLI_NO_MODIFY_PATH=1` and add the directory to `PATH` yourself:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/imify-ai/imify-cli/releases/latest/download/imify-cli-installer.sh | IMIFY_CLI_NO_MODIFY_PATH=1 sh
```

Release archives exist for macOS (Apple silicon and Intel) and Linux (x86_64 and aarch64). Windows is not supported.

From source, with Rust installed: `cargo install --locked --git https://github.com/imify-ai/imify-cli imify-cli`.

### Update

```sh
imify self update
```

`imify self update` installs the latest release when the installer above installed `imify`. A `cargo install` updates with `cargo install --locked imify-cli`. See [Update imify](docs/guide/self-update.md).

### Verify a download

Each release archive has a `.sha256` checksum file and a GitHub artifact attestation. Download an archive from the [releases page](https://github.com/imify-ai/imify-cli/releases) and check both:

```sh
sha256sum --check imify-cli-x86_64-unknown-linux-gnu.tar.xz.sha256
gh attestation verify imify-cli-x86_64-unknown-linux-gnu.tar.xz --repo imify-ai/imify-cli
```

On macOS, use `shasum -a 256 --check` in place of `sha256sum --check`. `gh attestation verify` confirms that the archive was built by the release workflow of `imify-ai/imify-cli`.

## What works in 0.1.0

- `imify auth login`, `imify auth logout` and `imify auth whoami`: browser sign-in, with `--no-browser` for machines without one; the login is stored in the OS keychain or a user-only file. `IMIFY_TOKEN` for CI.
- `imify datasets validate`: offline check of a LeRobot v3 or v2.1 dataset.
- `imify datasets upload`: uploads a dataset into an ingest session; an interrupted upload continues where it stopped.
- `imify datasets ls`: lists datasets, with filters and sorting.
- `imify doctor`: checks the configuration, the API, the clock and the token.
- `imify config`: stores the API URL.

## Authentication

```console
$ imify auth login
Signing in to https://platform.imify.ai through login.example.com. Opening the browser.
https://login.example.com/oauth2/authorize?...
Logged in to https://platform.imify.ai as Ada Lovelace (admin).
Credentials stored in the keychain.
$ imify auth whoami
Name     Ada Lovelace
Subject  user-1234
Role     admin
```

`imify auth login` signs in to the hosted service at `https://platform.imify.ai` through the browser. It stores the login for that host in the OS keychain, or in a file readable only by the user when no keychain is available. On a machine without a browser, `imify auth login --no-browser` prints the URL and reads the address the browser ends up on. `imify auth logout` revokes the login and removes it. ID tokens issued before the logout stay valid until they expire. Log in only to an API URL you trust: every command sends the token to that host.

`IMIFY_TOKEN` takes precedence over the stored login, for example in CI. The CLI sends the token as `Authorization: Bearer` and never writes it to logs or error messages.

A missing login fails with the error code `auth_required`, a rejected token with `auth_rejected`, an expired login with `auth_expired`; all exit with code 3. The [authentication guide](docs/guide/authentication.md) covers the stores, the environment variables and the errors.

## Configuration

```sh
imify config set api-url https://api.example.com        # store the host in ~/.config/imify/config.toml
imify auth login --api-url https://api.example.com      # or pass it to one command
export IMIFY_API_URL=https://api.example.com            # or set it for the shell
```

Without configuration, `imify` uses `https://platform.imify.ai`. `--api-url` takes precedence over `IMIFY_API_URL`, which takes precedence over the config file. The URL must use `https`, except for `localhost`, `127.0.0.1` and `::1`. Each host keeps its own login.

The config file is `~/.config/imify/config.toml` on macOS and Linux. `IMIFY_CONFIG_DIR` or `XDG_CONFIG_HOME` selects another directory. The [configuration guide](docs/guide/configuration.md) covers the config file and `imify config`.

## Rust SDK

The CLI is built on the [`imify`](crates/imify) crate, which can be used directly. Its public API is not stable before 1.0.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Architecture and decisions: [docs/dev](docs/dev/ARCHITECTURE.md).

## License

[Apache-2.0](LICENSE)
