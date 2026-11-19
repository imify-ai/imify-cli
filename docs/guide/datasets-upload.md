# Upload a dataset

```console
$ imify datasets upload ./pick-cube --task "Pick up the red cube"
warning  unexpected_entry  notes.txt  Not uploaded: only meta/, data/ and videos/ are uploaded.
Uploaded 12 files (1.4 GiB) in 1m 12s to recording 3f2c9a.
recording   3f2c9a
name        pick-cube
task        Pick up the red cube
operator    user-1234
dataset     0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13
format      v3
robot type  so100
fps         30
episodes    3
status      processing
files       12 (1.4 GiB)
```

`imify datasets upload <path> --task <text>` uploads a local LeRobot dataset into a recording and completes the recording. The files go straight to storage; the API never receives their bytes. Running the command again after an interrupted upload continues that recording. The API then catalogues the episodes into the dataset of the task, embodiment and format. The command reports the recording and the dataset its episodes went into.

| Argument | Meaning |
|---|---|
| `<path>` | The dataset root: the directory that contains `meta/info.json`. |
| `--task <text>` | Task description of the recorded episodes. Required and not empty, at most 512 characters. |
| `--robot <serial>` | Serial number of the robot: letters, digits, `.`, `_` and `-`, at most 128 characters. |
| `--operator <id>` | Id of the operator, never a name, with the characters of `--robot`. Without it, the API sets the signed-in user. |
| `--site <id>` | Id of the site, with the characters of `--robot`. |
| `--start-time <time>` | Start of the recording, an RFC 3339 date-time such as `2026-11-18T09:30:00Z`. |
| `--label <key>=<value>` | A label of the recording; repeat it for more, at most 64. A key starts with a lowercase letter and has only lowercase letters, digits, `_` and `-`, at most 63 characters. A value has at most 256 characters. |
| `--dry-run` | Validate and plan the upload, then stop. Needs no token and no network access. |
| `--resume` | Continue the interrupted upload of `<path>`. Fails with `not_found` when there is none. |
| `--restart` | Discard the interrupted upload of `<path>` and start a new recording. Cannot be combined with `--resume`. |

The recording options are checked before the dataset is read. A value outside its limits fails with `invalid_argument`, before any request.

The command requires a login or `IMIFY_TOKEN` for the API host, as described in [Authentication](authentication.md), except with `--dry-run`. It checks them before it reads the dataset.

## Steps

1. Validate the dataset with the checks of [`imify datasets validate`](datasets-validate.md). Findings are written as they are in that command's report. With an error finding, the command stops with `dataset_invalid` and sends no request.
2. Plan the files: every regular file below `meta/`, `data/` and `videos/`. Other entries, hidden files and symbolic links are skipped; symbolic links are not followed, and each skipped link is named in the output. A file is opened without following a symbolic link, and the upload stops with `invalid_argument` when the file was replaced or its size changed since planning.
3. Compute the SHA-256 of every file. The API takes the size and the SHA-256 of each file before its upload.
4. Create a recording, or continue the recording of an interrupted upload of the same directory, as described in [Resume](#resume). The recording's name is the name of the dataset directory, `pick-cube` for `./pick-cube` and for `.` inside it, cut to 128 characters.
5. Announce the files to the API, at most 1000 per request. The API answers with a presigned storage URL for each file storage does not have.
6. Send each of those files to its URL, 4 at a time. These requests carry no token. A file that fails with a network error, a storage error or a rate limit is sent again from its first byte. A URL that storage rejects is replaced once with a fresh one. Each uploaded file is recorded in the upload journal.
7. Complete the recording. The upload is done once the API answers; the API processes the episodes afterwards.

## Progress

When stderr is a terminal, a live view on stderr shows the upload:

```text
Uploading  pick-cube  recording 3f2c9a
[████████████████░░░░░░░░░░░░░░]  760.0 MiB / 1.4 GiB  53%  32.0 MiB/s  20s left
5/12 files  1 retry
  data/chunk-000/file-000.parquet                           90.0 MiB / 300.0 MiB
  data/chunk-000/file-001.parquet                           30.0 MiB / 300.0 MiB
  videos/observation.images.f…chunk-000/episode_000002.mp4  40.0 MiB / 210.0 MiB
```

The first row shows the phase: `Preparing` while the files are hashed and the recording is created, `Uploading`, `Paused (rate limited, 3s)` or `Completing`. For a resumed upload, the progress bar and the file count start at the files storage already holds. The rate is measured over the last 5 seconds. The rows below list the files in flight. When the view ends, one line stays in the scrollback:

| Outcome | Line |
|---|---|
| Success | `Uploaded 12 files (1.4 GiB) in 1m 12s to recording 3f2c9a.` |
| Ctrl-C | `Interrupted after 5 of 12 files. Recording 3f2c9a stays open.` |
| Error | `Failed after 5 of 12 files.` |

With `--json`, `--quiet`, `-v`, `TERM=dumb` or stderr not on a terminal, the live view is off. Plain lines on stderr report the same steps instead, one line per event, and `--quiet` turns them off. When the terminal does not report the cursor position within 250 ms, the command writes one `warning:` line on stderr and reports the steps as plain lines.

## JSON output

```console
$ imify datasets upload ./pick-cube --task "Pick up the red cube" --json
{"errors":0,"findings":[],"type":"validated","warnings":0}
{"bytes":8338,"files":6,"skipped":[],"skipped_entries":[],"type":"planned"}
{"session_id":"3f2c9a","type":"session_created"}
{"path":"meta/info.json","size":1451,"type":"file_started"}
{"path":"data/chunk-000/file-000.parquet","size":1417,"type":"file_started"}
{"path":"meta/info.json","size":1451,"type":"file_finished"}
{"path":"data/chunk-000/file-000.parquet","size":1417,"type":"file_finished"}
{"type":"completing"}
{"bytes":8338,"dataset_id":"0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13","dry_run":false,"episodes":2,"files":6,"format":"v3","fps":30,"labels":{},"name":"pick-cube","operator":"user-1234","resumed":false,"robot":null,"robot_type":"so100","session_id":"3f2c9a","site":null,"start_time":null,"status":"processing","task":"Pick up the red cube","type":"result"}
```

`--json` writes one JSON object per line to stdout. Events of files in flight at the same time interleave. With `--quiet`, only the `result` line is written.

| Type | Fields | Written |
|---|---|---|
| `validated` | `errors`, `warnings`, `findings` | Once. `findings` has the objects of `datasets validate --json`. |
| `planned` | `files`, `bytes`, `skipped`, `skipped_entries` | Once. `skipped` lists the paths of the entries that are not uploaded. `skipped_entries` lists the same entries as objects with `path` and `reason`: `outside_dataset`, `hidden`, `symbolic_link`, `not_regular_file` or `invalid_name`. |
| `session_created` | `session_id` | Once the recording exists. `session_id` is the id of the recording. |
| `session_resumed` | `session_id`, `files_done`, `bytes_done` | Instead of `session_created`, when the upload continues an interrupted upload. `files_done` files of `bytes_done` bytes are in storage and are not sent again. |
| `file_started` | `path`, `size` | When a file transfer starts. |
| `file_retrying` | `path`, `attempt`, `reason` | When a file is sent again from its first byte. |
| `file_finished` | `path`, `size` | When storage holds a file. |
| `rate_limited` | `pause_ms` | When the API rate-limited a request. The request is sent again after the pause. |
| `completing` | | Once storage holds every file. |

The event types `session_created` and `session_resumed` and the key `session_id` keep their names from the ingest sessions of the API before recordings, so scripts that read them keep working.
| `result` | see below | Last line, after success only. |

The `result` line:

| Field | Type | Content |
|---|---|---|
| `dry_run` | boolean | `true` with `--dry-run`. |
| `name` | string | Name of the recording. |
| `task` | string | The `--task` value. |
| `robot` | string or null | The `--robot` value. |
| `operator` | string or null | The `--operator` value with `--dry-run`. Otherwise the operator the API recorded: the signed-in user without `--operator`. |
| `site` | string or null | The `--site` value. |
| `start_time` | string or null | The `--start-time` value. |
| `labels` | object | The `--label` values, keyed by label key. |
| `files` | integer | Number of files of the dataset, including files a resumed upload did not send again. |
| `bytes` | integer | Their total size in bytes. |
| `session_id` | string | Id of the recording. Not present with `--dry-run`. |
| `dataset_id` | string or null | Id of the dataset the episodes went into. Not present with `--dry-run`. |
| `format` | string or null | Dataset format the API detected, such as `v3` or `v2.1`. Not present with `--dry-run`. |
| `robot_type` | string or null | Robot type from `meta/info.json`. Not present with `--dry-run`. |
| `fps` | number or null | Frames per second from `meta/info.json`. Not present with `--dry-run`. |
| `episodes` | integer | Number of episodes. Not present with `--dry-run`. |
| `status` | string | `processing` while the API processes the episodes, then `ready`, or another value the API reports. Not present with `--dry-run`. |
| `resumed` | boolean | `true` when the upload continued an interrupted upload. Not present with `--dry-run`. |

Paths use `/` separators and are relative to the dataset root. Schema: [`datasets-upload.json`](../reference/schemas/datasets-upload.json).

## Dry run

```console
$ imify datasets upload ./pick-cube --task "Pick up the red cube" --dry-run --json
{"errors":0,"findings":[],"type":"validated","warnings":0}
{"bytes":8338,"files":6,"skipped":[],"skipped_entries":[],"type":"planned"}
{"bytes":8338,"dry_run":true,"files":6,"labels":{},"name":"pick-cube","operator":null,"robot":null,"site":null,"start_time":null,"task":"Pick up the red cube","type":"result"}
```

`--dry-run` stops after the plan and shows every recording field the upload would send. It sends no request and reads no token.

## Ctrl-C

Ctrl-C stops the upload: the transfers in flight are dropped and the command exits with `interrupted` and code 130. The recording stays open and is not completed. Running the command again continues it.

## Resume

```console
$ imify datasets upload ./pick-cube --task "Pick up the red cube" --json
{"errors":0,"findings":[],"type":"validated","warnings":0}
{"bytes":8338,"files":6,"skipped":[],"skipped_entries":[],"type":"planned"}
{"bytes_done":7314,"files_done":5,"session_id":"3f2c9a","type":"session_resumed"}
{"path":"videos/observation.images.front/chunk-000/file-000.mp4","size":1024,"type":"file_started"}
{"path":"videos/observation.images.front/chunk-000/file-000.mp4","size":1024,"type":"file_finished"}
{"type":"completing"}
{"bytes":8338,"dataset_id":"0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13","dry_run":false,"episodes":2,"files":6,"format":"v3","fps":30,"labels":{},"name":"pick-cube","operator":"user-1234","resumed":true,"robot":null,"robot_type":"so100","session_id":"3f2c9a","site":null,"start_time":null,"status":"processing","task":"Pick up the red cube","type":"result"}
```

An upload records the recording and each uploaded file in a journal. When an upload stops before the recording is complete, after Ctrl-C or an error, running the same command again continues the same recording. The API reports which files storage holds, and only the others are sent. The journal keeps the SHA-256 of each uploaded file, so an unchanged file is not read again. When the API reports the upload of the journaled recording as done, the command reports that recording and sends nothing. The journal is deleted once the recording is complete.

| Flags | Without a journal | With a journal |
|---|---|---|
| none | New recording. | Continues the journaled recording. |
| `--resume` | Fails with `not_found`. | Continues the journaled recording. |
| `--restart` | New recording. | Deletes the journal and creates a new recording. The journaled recording stays open. |

There is one journal per API host and dataset directory, in the state directory of the user: `$XDG_STATE_HOME/imify/uploads/` when `XDG_STATE_HOME` is an absolute path, otherwise `~/.local/state/imify/uploads/`. The location is the same on macOS and Linux. `IMIFY_CONFIG_DIR` does not move it.

A file that changed after the recording learned its size and SHA-256 cannot replace the announced content. The upload then fails with `conflict` and the hint to run again with `--restart`.

A journal cannot be continued when the `--task` value or a recording option differs from the interrupted upload, a journaled file is no longer in the dataset directory, an older version of `imify` wrote the journal, or the journal is corrupt. A journal is corrupt when it is a symbolic link, is larger than 64 MiB, does not parse, or names another API URL or dataset directory than its file name stands for. The command then fails with `conflict` and the hint to run again with `--restart`.

The journal directory is created with mode `0700`. An existing journal directory that belongs to another user, or that the group or others can write, fails the upload with `internal` before any request.

## Errors

| Code | Exit | Cause |
|---|---|---|
| `invalid_argument` | 2 | `<path>` is not a readable directory or has no name, `--task` is missing or empty, a recording option is outside its limits, `--resume` and `--restart` are combined, the API URL is invalid, `HOME` is not an absolute path while `XDG_STATE_HOME` is unset, or a file changed size or became unreadable during the upload. |
| `auth_required` | 3 | There is no login for the host, and `IMIFY_TOKEN` is not set. |
| `auth_expired` | 3 | The stored login has expired or was revoked. |
| `auth_rejected`, `permission_denied` | 3 | The API rejected the token also after a refresh of the stored login, the token lacks the permission, or the network the request comes from has no access to the API. |
| `not_found` | 4 | `--resume` found no interrupted upload of `<path>`. |
| `dataset_invalid` | 5 | The validation report has an error, or the API rejected the dataset. The message is the API's reason. |
| `conflict` | 6 | The interrupted upload of `<path>` has another `--task` value, other recording options or a file no longer in the directory, its journal is outdated or corrupt, a file changed since the recording learned its SHA-256, or the API names files that are not uploaded. |
| `network`, `server`, `rate_limited` | 7 | The API or storage could not be reached, failed, or throttled the request after the retries, or storage refused a file. |
| `internal` | 1 | The API's response does not match the expected form, or the upload journal cannot be read or written. |
| `interrupted` | 130 | Ctrl-C. |

All error codes: [JSON output and exit codes](../reference/json-output.md#errors).
