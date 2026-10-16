# 0011. No telemetry

Status: Accepted
Date: 2026-10-16

## Context
Enterprises review what a binary sends and where. The `User-Agent` header already tells the server which CLI versions are in use.

## Decision
The CLI collects no usage data. It contacts only the configured host and, for the update check, GitHub releases. `IMIFY_NO_UPDATE_CHECK=1` disables the update check. Diagnostics are local: `imify doctor` and `-vv`.

## Consequences
Product questions about command usage are answered from server-side logs, not from the client.
