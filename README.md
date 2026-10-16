# imify

Command-line client and Rust SDK for the [imify](https://imify.ai) platform: manage robot learning datasets from a terminal, a script or an AI agent.

> **Status:** in development, no release yet. The commands below are the scope of the first release, tracked in the [0.1.0 milestone](https://github.com/imify-ai/imify-cli/milestone/1).

## Install

After the first release:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/imify-ai/imify-cli/releases/latest/download/imify-cli-installer.sh | sh
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/imify-ai/imify-cli/releases/latest/download/imify-cli-installer.ps1 | iex"
```

From source: `cargo install --locked --git https://github.com/imify-ai/imify-cli imify-cli`.

## Usage

```sh
imify login                                     # sign in through the browser
imify datasets validate ./pick-cube             # check a LeRobot v3 or v2.1 dataset offline
imify datasets upload ./pick-cube --task "pick the cube"
imify datasets ls
```

Every command accepts `--json` for machine-readable output on stdout and returns a [documented exit code](docs/dev/adr/0003-output-contract.md). Set `IMIFY_OUTPUT=json` to make it the default, for example in an agent's environment.

## Authentication

```console
$ export IMIFY_TOKEN=<access token>
$ export IMIFY_API_URL=https://api.example.com
$ imify whoami
Name     Ada Lovelace
Subject  user-1234
Role     admin
```

`IMIFY_TOKEN` is the access token. The CLI sends it as `Authorization: Bearer` and never writes it to logs or error messages. `IMIFY_API_URL` is the API host; every command that calls the API requires it. `imify whoami` prints the user the token belongs to. With `--json`, it prints `subject`, `name` and `role` as one object. A missing token fails with the error code `auth_required`, a rejected token with `auth_rejected`; both exit with code 3.

## Rust SDK

The CLI is built on the [`imify`](crates/imify) crate, which can be used directly.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Architecture and decisions: [docs/dev](docs/dev/ARCHITECTURE.md).

## License

[Apache-2.0](LICENSE)
