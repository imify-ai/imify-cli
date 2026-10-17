# 0015. SDK API layer generated from the vendored OpenAPI document

Status: Accepted
Date: 2026-10-17

## Context
The server defines the API and publishes it as an OpenAPI document. The API is versioned by path (`/api/v1`). Within a version, the server only adds fields and endpoints. Hand-written SDK types repeat what the document already states and can drift from it. Contract tests catch that drift, but only for the cases the tests cover.

Some SDK behavior is not described by the document: login and token refresh, resumable uploads with a local journal, retries and progress events.

## Decision
- The repository vendors the client-facing subset of the OpenAPI document that the server exports for this purpose. `scripts/update-openapi.sh` refreshes it, and a refresh is an explicit change in a PR.
- The SDK's request and response types and its endpoint calls are generated from the vendored document. Generated code is not edited by hand.
- Generated types are part of the SDK's public API. A breaking change in the document is a breaking change of the SDK.
- Login and token refresh, uploads, retries and progress events stay hand-written and use the generated types.
- Responses deserialize tolerantly: unknown fields are ignored.
- Every request sends `User-Agent: imify-cli/<version>`.
- Mock fixtures are built from the generated types, not from raw JSON.
- A check compares the vendored document with the one the server publishes and reports the difference.
- The generator is chosen in the implementing PR. It must read the OpenAPI version the server emits, run without network access during the build, and produce code that passes the CI gates.

Supersedes [0009](0009-sdk-types-and-api-contract.md).

## Consequences
The server stays the single source of truth for the API. A change in the document shows up as a compile error in the SDK or the CLI, not as a failing contract test or a user-facing error. Contract tests for types are no longer needed.

The SDK's version follows breaking changes of the API. Generated items carry the descriptions from the document, not hand-written doc comments with examples. The build depends on a code generator. The vendored document is public with this repository, so it contains only the client-facing subset.
