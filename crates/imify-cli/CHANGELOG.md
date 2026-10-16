# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/imify-ai/imify-cli/releases/tag/v0.1.0) - 2026-10-16

### Added

- *(cli)* move login, logout and whoami under auth ([#69](https://github.com/imify-ai/imify-cli/pull/69))
- *(cli)* use ~/.config/imify and ~/.local/state/imify on macOS and Linux ([#68](https://github.com/imify-ai/imify-cli/pull/68))
- *(cli)* default API URL, --api-url and imify config ([#61](https://github.com/imify-ai/imify-cli/pull/61))
- *(auth)* add imify login and imify logout ([#51](https://github.com/imify-ai/imify-cli/pull/51))
- *(datasets)* resume interrupted uploads with --resume and --restart ([#50](https://github.com/imify-ai/imify-cli/pull/50))
- *(cli)* add imify doctor ([#47](https://github.com/imify-ai/imify-cli/pull/47))
- *(datasets)* add imify datasets upload ([#44](https://github.com/imify-ai/imify-cli/pull/44))
- *(cli)* add inline viewport foundation for live progress views ([#32](https://github.com/imify-ai/imify-cli/pull/32))
- *(datasets)* add imify datasets validate ([#35](https://github.com/imify-ai/imify-cli/pull/35))
- *(datasets)* add imify datasets ls ([#34](https://github.com/imify-ai/imify-cli/pull/34))
- *(auth)* add IMIFY_TOKEN token source and whoami ([#30](https://github.com/imify-ai/imify-cli/pull/30))
- *(cli)* add output contract with --json, error codes and exit codes ([#27](https://github.com/imify-ai/imify-cli/pull/27))

### Fixed

- *(datasets)* harden upload and validate against crafted datasets ([#63](https://github.com/imify-ai/imify-cli/pull/63))
- *(cli)* escape control and format characters in all human output ([#59](https://github.com/imify-ai/imify-cli/pull/59))

### Other

- *(release)* prepare 0.1.0 ([#70](https://github.com/imify-ai/imify-cli/pull/70))
- *(release)* harden workflows before 0.1.0 ([#62](https://github.com/imify-ai/imify-cli/pull/62))
- bump base64 from 0.22.1 to 0.23.1 ([#53](https://github.com/imify-ai/imify-cli/pull/53))
- *(release)* drop Windows from CI and release targets ([#29](https://github.com/imify-ai/imify-cli/pull/29))
- bootstrap workspace, gates and docs
