# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-10-16

### Added

- *(cli)* add imify doctor ([#47](https://github.com/imify-ai/imify-cli/pull/47))
- *(sdk)* resume interrupted uploads from a local journal ([#42](https://github.com/imify-ai/imify-cli/pull/42))
- *(auth)* add SDK browser login, stored login and refresh ([#43](https://github.com/imify-ai/imify-cli/pull/43))
- *(datasets)* add imify datasets validate ([#35](https://github.com/imify-ai/imify-cli/pull/35))
- *(datasets)* add imify datasets ls ([#34](https://github.com/imify-ai/imify-cli/pull/34))
- *(sdk)* add upload engine with streaming PUT transport and pacing ([#33](https://github.com/imify-ai/imify-cli/pull/33))
- *(auth)* add IMIFY_TOKEN token source and whoami ([#30](https://github.com/imify-ai/imify-cli/pull/30))
- *(sdk)* add client, error model and retry policy ([#26](https://github.com/imify-ai/imify-cli/pull/26))

### Fixed

- *(datasets)* harden upload and validate against crafted datasets ([#63](https://github.com/imify-ai/imify-cli/pull/63))
- *(auth)* harden SDK sign-in ([#55](https://github.com/imify-ai/imify-cli/pull/55))

### Other

- *(release)* prepare 0.1.0 ([#70](https://github.com/imify-ai/imify-cli/pull/70))
- bump base64 from 0.22.1 to 0.23.1 ([#53](https://github.com/imify-ai/imify-cli/pull/53))
- bump getrandom from 0.3.4 to 0.4.3 ([#52](https://github.com/imify-ai/imify-cli/pull/52))
- bootstrap workspace, gates and docs
