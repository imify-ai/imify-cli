# 0017. Direct-to-storage upload into recordings

Status: Accepted
Date: 2026-11-18

## Context
The API of milestone "API v1 redesign" replaces ingest sessions with recordings and no longer takes file bytes. A client creates a recording, announces each file with its path, size and SHA-256 (`files:batchCreate`), and sends each file with one `PUT` to the presigned storage URL the API returns. `:complete` reads the uploaded dataset and catalogues its episodes. The API reports the state of each announced file, read from storage on every call, so a client can ask which files arrived. A create carries a `requestId`: a repeated create with the same value returns the first recording. The old ingest routes are removed.

Storage verifies the SHA-256 of each `PUT` against the announced value and answers in the S3 XML shape. A presigned URL expires after one hour and carries a temporary credential.

## Decision
- The upload is direct to storage. The `Transport` sends one streamed `PUT` per file to its presigned URL, with `Content-Length` and the headers the API names, never an `Authorization` header. Errors and logs never contain the URL.
- The engine computes the SHA-256 of every file before the recording is created, and announces the files at most 1000 per request. `meta/info.json` is no longer sent first: the API reads the dataset only at `:complete`.
- The create, the announcements and `:complete` are retried on network errors, server errors and rate limits; the create sends one `requestId` for all its attempts.
- A storage 403 renews the URL by announcing the file again, once per file. A `BadDigest` sends the file once more from its first byte. A second failure ends the upload. A storage 429 or 5xx is retried with backoff.
- The resume journal keeps its location, permissions and line format, with format version 2. The header holds the recording id, the `requestId` and the settings of the recording; each entry holds a file storage holds, with its SHA-256.
- Resume asks the API, not the journal, which files storage holds: it reads the recording, announces every file again and sends the files that are not uploaded. The journal saves hashing unchanged files and detects a resume with other settings. When the upload of the journaled recording is done, resume reports that recording.
- A journal of format version 1 cannot be continued; the upload fails with the hint to run with `--restart`.
- The JSON output keeps its keys and event types, such as `session_id` and `session_created`, so scripts keep working.

Supersedes [0006](0006-upload-transport.md).

## Consequences
File bytes no longer pass through the API, so the API does not bound throughput. Resume no longer trusts the journal: a file the journal records but storage lost is sent again. Hashing reads every file once before the upload starts, which takes time for large datasets. A file that changes after it was announced cannot be uploaded to the same recording; the user restarts. Two concurrent uploads of the same directory to the same host share one recording and journal; storage holds one copy of each file and `:complete` is idempotent.
