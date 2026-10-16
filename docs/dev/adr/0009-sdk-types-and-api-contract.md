# 0009. Hand-written SDK types, contract-tested against OpenAPI

Status: Accepted
Date: 2026-10-16

## Context
Installed CLI versions keep calling the API after it changes. Generated client types would make every server-side rename a breaking change of the SDK's public API.

## Decision
- SDK types are written by hand and documented.
- Responses deserialize tolerantly: unknown fields are ignored.
- The repository vendors the API's OpenAPI document. Every mock fixture and every request the SDK builds is validated against it in tests.
- Every request sends `User-Agent: imify-cli/<version>`.

## Consequences
Drift between SDK and API fails a test instead of breaking users. Refreshing the vendored document is an explicit change in a PR.
