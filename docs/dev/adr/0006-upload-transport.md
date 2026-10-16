# 0006. Streaming PUT upload with a local resume journal

Status: Accepted
Date: 2026-10-16

## Context
The API accepts dataset files as streamed `PUT` requests per file into an ingest session, followed by a `complete` call that validates and catalogues the session. It also offers presigned direct-to-storage URLs, which no client uses yet and which skip the server's early format check. Neither path is resumable on the server side. The Hugging Face Hub uses direct-to-storage transfer followed by a commit that verifies what arrived; the imify API does not verify transfers at `complete` yet.

## Decision
- The first release uses streaming `PUT` only, behind a `Transport` trait in the SDK.
- `meta/info.json` uploads first, so an unsupported format fails before the bulk transfer.
- Uploads use bounded concurrency (default 4), retries with exponential backoff and jitter, and client-side request pacing.
- A local journal records finished files per session. Re-running the same upload, or `--resume`, sends only unfinished files to the same session.

## Consequences
All bytes pass through the API, which bounds throughput. Direct-to-storage transfer with server-side verification is a second `Transport` implementation once the API verifies uploads at `complete`.
