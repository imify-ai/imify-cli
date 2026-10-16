# imify

Command-line client and Rust SDK for the [imify](https://imify.ai) platform: manage robot learning datasets from a terminal, a script or an AI agent.

> **Status:** in development, no release yet. The commands below are the scope of the first release, tracked in the [0.1.0 milestone](https://github.com/imify-ai/imify-cli/milestone/1).

## Install

After the first release:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/imify-ai/imify-cli/releases/latest/download/imify-cli-installer.sh | sh
```

The installer places `imify` in `$CARGO_HOME/bin` (default `~/.cargo/bin`) and adds that directory to `PATH` in your shell profile. To leave the shell profile unchanged, set `IMIFY_CLI_NO_MODIFY_PATH=1` and add the directory to `PATH` yourself:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/imify-ai/imify-cli/releases/latest/download/imify-cli-installer.sh | IMIFY_CLI_NO_MODIFY_PATH=1 sh
```

From source: `cargo install --locked --git https://github.com/imify-ai/imify-cli imify-cli`.

### Verify a download

Each release archive has a `.sha256` checksum file and a GitHub artifact attestation. Download an archive from the [releases page](https://github.com/imify-ai/imify-cli/releases) and check both:

```sh
sha256sum --check imify-cli-x86_64-unknown-linux-gnu.tar.xz.sha256
gh attestation verify imify-cli-x86_64-unknown-linux-gnu.tar.xz --repo imify-ai/imify-cli
```

On macOS, use `shasum -a 256 --check` in place of `sha256sum --check`. `gh attestation verify` confirms that the archive was built by the release workflow of `imify-ai/imify-cli`.

## Usage

```sh
imify login                                     # sign in through the browser
imify datasets validate ./pick-cube             # check a LeRobot v3 or v2.1 dataset offline
imify datasets upload ./pick-cube --task "pick the cube"
imify datasets ls
imify doctor                                    # check the configuration, the API and the token
```

Every command accepts `--json` for machine-readable output on stdout and returns a [documented exit code](docs/dev/adr/0003-output-contract.md). Set `IMIFY_OUTPUT=json` to make it the default, for example in an agent's environment.

## Authentication

```console
$ imify login
Signing in to https://platform.imify.ai through login.example.com. Opening the browser.
https://login.example.com/oauth2/authorize?...
Logged in to https://platform.imify.ai as Ada Lovelace (admin).
Credentials stored in the keychain.
$ imify whoami
Name     Ada Lovelace
Subject  user-1234
Role     admin
```

After the install, `imify login` signs in to the hosted service at `https://platform.imify.ai` through the browser. It stores the login for that host in the OS keychain, or in a file readable only by the user when no keychain is available. On a machine without a browser, `imify login --no-browser` prints the URL and reads the address the browser ends up on. `imify logout` revokes the login and removes it. ID tokens issued before the logout stay valid until they expire. Log in only to an API URL you trust: every command sends the token to that host.

`IMIFY_TOKEN` takes precedence over the stored login, for example in CI. The CLI sends the token as `Authorization: Bearer` and never writes it to logs or error messages.

A missing login fails with the error code `auth_required`, a rejected token with `auth_rejected`, an expired login with `auth_expired`; all exit with code 3. The [authentication guide](docs/guide/authentication.md) covers the stores, the environment variables and the errors.

## Another host

```sh
imify config set api-url https://api.example.com   # store the host in the config file
imify login --api-url https://api.example.com      # or pass it to one command
export IMIFY_API_URL=https://api.example.com       # or set it for the shell
```

`--api-url` takes precedence over `IMIFY_API_URL`, which takes precedence over the config file. The URL must use `https`, except for `localhost`, `127.0.0.1` and `::1`. Each host keeps its own login. By default, the config file is `~/.config/imify/config.toml` on macOS and Linux. The [configuration guide](docs/guide/configuration.md) covers the config file and `imify config`.

## Rust SDK

The CLI is built on the [`imify`](crates/imify) crate, which can be used directly.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Architecture and decisions: [docs/dev](docs/dev/ARCHITECTURE.md).

## License

[Apache-2.0](LICENSE)
