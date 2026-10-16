# 0012. Distribution with dist and release-plz

Status: Accepted
Date: 2026-10-16

## Context
Installation must be one command, releases must be reproducible and verifiable, and versioning follows conventional commits.

## Decision
- release-plz derives versions and changelogs from conventional commits through a release PR. Merging it tags `vX.Y.Z`.
- The tag triggers dist, which builds macOS (arm64, x86_64), Linux (arm64, x86_64, static musl x86_64) and Windows (x86_64) archives, sha256 checksums, GitHub artifact attestations, and shell and PowerShell installers.
- Binaries are not code-signed in the first release.

## Consequences
`curl ... | sh` installs work without signing. Homebrew and package managers that check signatures need signing added later.
