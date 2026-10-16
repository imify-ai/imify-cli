# 0006. Streaming PUT upload with a local resume journal

Status: Accepted
Date: 2026-10-16

## Context
The API accepts dataset files as streamed `PUT` requests per file into an ingest session, followed by a `complete` call that validates and catalogues the session. It also offers presigned direct-to-storage URLs, which no client uses yet and which skip the server's early format check. Neither path is resumable on the server side. The Hugging Face Hub uses direct-to-storage transfer followed by a commit that verifies what arrived; the imify API does not verify transfers at `complete` yet.

The API has no endpoint that lists the files stored in a session. A client that resumes an upload cannot ask the server which files arrived.

## Decision
- The first release uses streaming `PUT` only, behind a `Transport` trait in the SDK.
- `meta/info.json` uploads first, so an unsupported format fails before the bulk transfer.
- Uploads use bounded concurrency (default 4), retries with exponential backoff and jitter, and client-side request pacing.
- A local journal records finished files per session. Re-running the same upload, or `--resume`, sends only unfinished files to the same session.

### Resume journal
- The SDK reads no environment variables. The caller passes the journal directory to `Uploader::journal`.
- The CLI resolves the directory. Journals are state, not configuration, so `IMIFY_CONFIG_DIR` does not move them:
  - Linux: `$XDG_STATE_HOME/imify/uploads/`, or `~/.local/state/imify/uploads/` when `XDG_STATE_HOME` is unset or empty.
  - macOS: `~/Library/Application Support/imify/uploads/`.
- The directory has mode `0700`. Each journal has mode `0600`. Its name is the hex SHA-256 of the API base URL, a newline and the canonical dataset directory, with the extension `.jsonl`.
- The journal is append-only JSON Lines. The first line holds the format version, the session id, the session name and the task description. Each later line records one file the API stored: path, size, modification time in nanoseconds, and the SHA-256 of the bytes sent. An entry exists only for a stored file, so no `done` flag is needed. A partial last line, left by a crash, is ignored.
- The header is written after the session exists, to a temporary file renamed into place. Each entry is flushed to disk before the file is reported as finished.
- Resume trusts the journal, because the API cannot list session files. A journaled file is skipped when its size and modification time match. When only the modification time differs, it is skipped when its SHA-256 matches. Every other file is sent again; `PUT` overwrites.
- Resume refuses a journal whose session name or task description differs from the new upload, or that records a file no longer in the dataset directory. A stored file cannot be removed from a session, so the caller restarts.
- Restarting deletes the journal and creates a new session. The old session stays open, because the API cannot delete a session.
- The journal is deleted when `complete` succeeds and kept after any error.

## Consequences
All bytes pass through the API, which bounds throughput. Direct-to-storage transfer with server-side verification is a second `Transport` implementation once the API verifies uploads at `complete`.

Resume does not verify journaled files against the server. An endpoint that lists the files of a session with path, size and hash would allow that check. Two concurrent uploads of the same directory to the same host share one session and journal; `PUT` overwrites and `complete` is idempotent, so the session ends up complete.
