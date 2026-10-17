# Upload a dataset

```console
$ imify datasets upload ./pick-cube --task "Pick up the red cube"
warning  unexpected_entry  notes.txt  Not uploaded: only meta/, data/ and videos/ are uploaded.
Uploaded 12 files (1.4 GiB) in 1m 12s to ingest session 3f2c9a.
session   3f2c9a
name      pick-cube
task      Pick up the red cube
format    lerobot_v3
robot     so100
fps       30
episodes  3
status    ready
files     12 (1.4 GiB)
```

`imify datasets upload <path> --task <text>` uploads a local LeRobot dataset into an ingest session and completes the session. Running it again after an interrupted upload continues that session. The API then catalogues the episodes into a dataset. The command reports the ingest session, not the dataset.

| Argument | Meaning |
|---|---|
| `<path>` | The dataset root: the directory that contains `meta/info.json`. |
| `--task <text>` | Task description of the recorded episodes. Required and not empty. |
| `--dry-run` | Validate and plan the upload, then stop. Needs no token and no network access. |
| `--resume` | Continue the interrupted upload of `<path>`. Fails with `not_found` when there is none. |
| `--restart` | Discard the interrupted upload of `<path>` and start a new ingest session. Cannot be combined with `--resume`. |

The command requires a login or `IMIFY_TOKEN` for the API host, as described in [Authentication](authentication.md), except with `--dry-run`. It checks them before it reads the dataset.

## Steps

1. Validate the dataset with the checks of [`imify datasets validate`](datasets-validate.md). Findings are written as they are in that command's report. With an error finding, the command stops with `dataset_invalid` and sends no request.
2. Plan the files: every regular file below `meta/`, `data/` and `videos/`. Other entries, hidden files and symbolic links are skipped; symbolic links are not followed, and each skipped link is named in the output. A file is opened without following a symbolic link, and the upload stops with `invalid_argument` when the file was replaced or its size changed since planning.
3. Create an ingest session, or continue the session of an interrupted upload of the same directory, as described in [Resume](#resume). The session's name is the name of the dataset directory, `pick-cube` for `./pick-cube` and for `.` inside it.
4. Send `meta/info.json`, then the other files, 4 at a time. A file that fails with a network error, a server error or a rate limit is sent again from its first byte. Each stored file is recorded in the upload journal.
5. Complete the session.

## Progress

When stderr is a terminal, a live view on stderr shows the upload:

```text
Uploading  pick-cube  session 3f2c9a
[████████████████░░░░░░░░░░░░░░]  760.0 MiB / 1.4 GiB  53%  32.0 MiB/s  20s left
5/12 files  1 retry
  data/chunk-000/file-000.parquet                           90.0 MiB / 300.0 MiB
  data/chunk-000/file-001.parquet                           30.0 MiB / 300.0 MiB
  videos/observation.images.f…chunk-000/episode_000002.mp4  40.0 MiB / 210.0 MiB
```

The first row shows the phase: `Creating session`, `Uploading`, `Paused (rate limited, 3s)` or `Completing`. For a resumed upload, the progress bar and the file count start at the files the session already stores. The rate is measured over the last 5 seconds. The rows below list the files in flight. When the view ends, one line stays in the scrollback:

| Outcome | Line |
|---|---|
| Success | `Uploaded 12 files (1.4 GiB) in 1m 12s to ingest session 3f2c9a.` |
| Ctrl-C | `Interrupted after 5 of 12 files. Ingest session 3f2c9a stays open.` |
| Error | `Failed after 5 of 12 files.` |

With `--json`, `--quiet`, `-v`, `TERM=dumb` or stderr not on a terminal, the live view is off. Plain lines on stderr report the same steps instead, one line per event, and `--quiet` turns them off. When the terminal does not report the cursor position within 250 ms, the command writes one `warning:` line on stderr and reports the steps as plain lines.

## JSON output

```console
$ imify datasets upload ./pick-cube --task "Pick up the red cube" --json
{"errors":0,"findings":[],"type":"validated","warnings":0}
{"bytes":8338,"files":6,"skipped":[],"skipped_entries":[],"type":"planned"}
{"session_id":"3f2c9a","type":"session_created"}
{"path":"meta/info.json","size":1451,"type":"file_started"}
{"path":"meta/info.json","size":1451,"type":"file_finished"}
{"path":"data/chunk-000/file-000.parquet","size":1417,"type":"file_started"}
{"path":"data/chunk-000/file-000.parquet","size":1417,"type":"file_finished"}
{"type":"completing"}
{"bytes":8338,"dry_run":false,"episodes":2,"files":6,"format":"lerobot_v3","fps":30.0,"name":"pick-cube","resumed":false,"robot_type":"so100","session_id":"3f2c9a","status":"ready","task":"Pick up the red cube","type":"result"}
```

`--json` writes one JSON object per line to stdout. Events of files in flight at the same time interleave. With `--quiet`, only the `result` line is written.

| Type | Fields | Written |
|---|---|---|
| `validated` | `errors`, `warnings`, `findings` | Once. `findings` has the objects of `datasets validate --json`. |
| `planned` | `files`, `bytes`, `skipped`, `skipped_entries` | Once. `skipped` lists the paths of the entries that are not uploaded. `skipped_entries` lists the same entries as objects with `path` and `reason`: `outside_dataset`, `hidden`, `symbolic_link`, `not_regular_file` or `invalid_name`. |
| `session_created` | `session_id` | Once the session exists. |
| `session_resumed` | `session_id`, `files_done`, `bytes_done` | Instead of `session_created`, when the upload continues an interrupted upload. `files_done` files of `bytes_done` bytes are not sent again. |
| `file_started` | `path`, `size` | When a file transfer starts. |
| `file_retrying` | `path`, `attempt`, `reason` | When a file is sent again from its first byte. |
| `file_finished` | `path`, `size` | When the API stored a file. |
| `rate_limited` | `pause_ms` | When the API rate-limited a request. No request starts during the pause. |
| `completing` | | Once every file is stored. |
| `result` | see below | Last line, after success only. |

The `result` line:

| Field | Type | Content |
|---|---|---|
| `dry_run` | boolean | `true` with `--dry-run`. |
| `name` | string | Name of the ingest session. |
| `task` | string | The `--task` value. |
| `files` | integer | Number of files of the dataset, including files a resumed upload did not send again. |
| `bytes` | integer | Their total size in bytes. |
| `session_id` | string | Id of the ingest session. Not present with `--dry-run`. |
| `format` | string or null | Dataset format the API detected, such as `lerobot_v3`. Not present with `--dry-run`. |
| `robot_type` | string or null | Robot type from `meta/info.json`. Not present with `--dry-run`. |
| `fps` | number or null | Frames per second from `meta/info.json`. Not present with `--dry-run`. |
| `episodes` | integer or null | Number of episodes. Not present with `--dry-run`. |
| `status` | string | `ready`, `uploading`, or another value the API reports. Not present with `--dry-run`. |
| `resumed` | boolean | `true` when the upload continued an interrupted upload. Not present with `--dry-run`. |

Paths use `/` separators and are relative to the dataset root. Schema: [`datasets-upload.json`](../reference/schemas/datasets-upload.json).

## Dry run

```console
$ imify datasets upload ./pick-cube --task "Pick up the red cube" --dry-run --json
{"errors":0,"findings":[],"type":"validated","warnings":0}
{"bytes":8338,"files":6,"skipped":[],"skipped_entries":[],"type":"planned"}
{"bytes":8338,"dry_run":true,"files":6,"name":"pick-cube","task":"Pick up the red cube","type":"result"}
```

`--dry-run` stops after the plan. It sends no request and reads no token.

## Ctrl-C

Ctrl-C stops the upload: the transfers in flight are dropped and the command exits with `interrupted` and code 130. The ingest session stays open and is not completed. Running the command again continues it.

## Resume

```console
$ imify datasets upload ./pick-cube --task "Pick up the red cube" --json
{"errors":0,"findings":[],"type":"validated","warnings":0}
{"bytes":8338,"files":6,"skipped":[],"skipped_entries":[],"type":"planned"}
{"bytes_done":7314,"files_done":5,"session_id":"3f2c9a","type":"session_resumed"}
{"path":"videos/observation.images.front/chunk-000/file-000.mp4","size":1024,"type":"file_started"}
{"path":"videos/observation.images.front/chunk-000/file-000.mp4","size":1024,"type":"file_finished"}
{"type":"completing"}
{"bytes":8338,"dry_run":false,"episodes":2,"files":6,"format":"lerobot_v3","fps":30.0,"name":"pick-cube","resumed":true,"robot_type":"so100","session_id":"3f2c9a","status":"ready","task":"Pick up the red cube","type":"result"}
```

An upload records each file the API stored in a journal. When an upload stops before the session is complete, after Ctrl-C or an error, running the same command again continues the same ingest session and sends only the files the journal does not record, or that changed since. The journal is deleted once the session is complete.

| Flags | Without a journal | With a journal |
|---|---|---|
| none | New session. | Continues the journaled session. |
| `--resume` | Fails with `not_found`. | Continues the journaled session. |
| `--restart` | New session. | Deletes the journal and creates a new session. The journaled session stays open. |

There is one journal per API host and dataset directory, in the state directory of the user: `$XDG_STATE_HOME/imify/uploads/` when `XDG_STATE_HOME` is an absolute path, otherwise `~/.local/state/imify/uploads/`. The location is the same on macOS and Linux. `IMIFY_CONFIG_DIR` does not move it.

A journal cannot be continued when the `--task` value differs from the interrupted upload, a journaled file is no longer in the dataset directory, or the journal is corrupt. A journal is corrupt when it is a symbolic link, is larger than 64 MiB, does not parse, or names another API URL or dataset directory than its file name stands for. The command then fails with `conflict` and the hint to run again with `--restart`.

The journal directory is created with mode `0700`. An existing journal directory that belongs to another user, or that the group or others can write, fails the upload with `internal` before any request.

## Errors

| Code | Exit | Cause |
|---|---|---|
| `invalid_argument` | 2 | `<path>` is not a readable directory or has no name, `--task` is missing or empty, `--resume` and `--restart` are combined, the API URL is invalid, `HOME` is not an absolute path while `XDG_STATE_HOME` is unset, or a file changed or became unreadable during the upload. |
| `auth_required` | 3 | There is no login for the host, and `IMIFY_TOKEN` is not set. |
| `auth_expired` | 3 | The stored login has expired or was revoked. |
| `auth_rejected`, `permission_denied` | 3 | The API rejected the token, or the token lacks the permission. |
| `not_found` | 4 | `--resume` found no interrupted upload of `<path>`. |
| `dataset_invalid` | 5 | The validation report has an error, or the API rejected the dataset. The message is the API's reason. |
| `conflict` | 6 | The interrupted upload of `<path>` has another `--task` value or a file no longer in the directory, or its journal is corrupt. |
| `network`, `server`, `rate_limited` | 7 | The API could not be reached, failed, or throttled the request after the retries. |
| `internal` | 1 | The API's response does not match the expected form, or the upload journal cannot be read or written. |
| `interrupted` | 130 | Ctrl-C. |

All error codes: [JSON output and exit codes](../reference/json-output.md#errors).
