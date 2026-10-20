# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/imify-ai/imify-cli/compare/v0.2.0...v0.3.0) - 2026-10-20

### Added

- *(cli)* rename datasets ls to datasets list ([#107](https://github.com/imify-ai/imify-cli/pull/107))
- *(policies)* download a checkpoint into a directory ([#105](https://github.com/imify-ai/imify-cli/pull/105))
- *(policies)* list policies and show one with its checkpoints ([#104](https://github.com/imify-ai/imify-cli/pull/104))
- *(auth)* discover the login client from protected resource metadata ([#94](https://github.com/imify-ai/imify-cli/pull/94))

## [0.2.0](https://github.com/imify-ai/imify-cli/compare/v0.1.0...v0.2.0) - 2026-10-17

### Added

- *(cli)* show a hint when a newer release exists ([#87](https://github.com/imify-ai/imify-cli/pull/87))
- *(cli)* imify self update ([#83](https://github.com/imify-ai/imify-cli/pull/83))
- *(doctor)* report the active profile ([#85](https://github.com/imify-ai/imify-cli/pull/85))
- *(cli)* profiles with --profile and IMIFY_PROFILE ([#84](https://github.com/imify-ai/imify-cli/pull/84))
- *(doctor)* check the stored login ([#79](https://github.com/imify-ai/imify-cli/pull/79))
- *(auth)* point to the stored login when IMIFY_TOKEN is rejected ([#75](https://github.com/imify-ai/imify-cli/pull/75))

### Fixed

- *(tui)* fall back to plain lines when the live view cannot start ([#74](https://github.com/imify-ai/imify-cli/pull/74))

### Other

- *(release)* publish a Homebrew formula to imify-ai/homebrew-tap ([#90](https://github.com/imify-ai/imify-cli/pull/90))
- explain the macOS quarantine for browser-downloaded archives ([#81](https://github.com/imify-ai/imify-cli/pull/81))
- *(cli)* generate the CLI reference and man pages ([#78](https://github.com/imify-ai/imify-cli/pull/78))
- release v0.1.0 ([#24](https://github.com/imify-ai/imify-cli/pull/24))

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
