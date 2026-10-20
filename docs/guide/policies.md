# Policies

A policy is the model trained on one dataset version. A checkpoint is a policy's state at one training step. `imify policies list` lists policies, `imify policies show` shows one policy with its checkpoints, and `imify policies download` saves a checkpoint to a directory.

The commands require a login or `IMIFY_TOKEN` for the API host, as described in [Authentication](authentication.md).

## List policies

```console
$ imify policies list
ID            TYPE        STATE           PROGRESS  DATASET                               CREATED
run-3c9e1a7f  act         ready        20000/20000  0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13  2026-10-01T09:30:00Z
run-8b20d4e6  groot_n1.7  training      1200/30000  5e2d7a90-1c3b-4e6f-8a7d-9b0c1d2e3f45  2026-10-02T08:00:00Z
```

`imify policies list` prints every policy visible to the user the access token belongs to, in the order the API returns them. `TYPE` is the policy architecture, such as `act`, `groot_n1.7` or `pi0.5`. `PROGRESS` is the completed training steps out of the planned ones. Without policies, the command prints `No policies.` and exits with code 0.

`STATE` is one of these values:

| State | Meaning |
|---|---|
| `queued` | Training has not started. |
| `training` | Training is running. |
| `ready` | Training succeeded. |
| `failed` | Training failed. `imify policies show` prints the reason. |

A state that this version of `imify` does not know is printed as the API sends it.

### Filter

```sh
imify policies list --state ready
imify policies list --dataset 0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13
```

| Flag | Keeps policies |
|---|---|
| `--state <STATE>` | in this state: `queued`, `training`, `ready` or `failed` |
| `--dataset <ID>` | trained on the dataset with this id |

Filters combine: a policy is listed when it matches every filter given.

### JSON

```console
$ imify policies list --state ready --json
[{"created_at":"2026-10-01T09:30:00Z","dataset_id":"0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13","dataset_version_id":"ver-41d2","error":null,"finished_at":"2026-10-01T11:02:40Z","id":"run-3c9e1a7f","phase":"done","policy_type":"act","started_at":"2026-10-01T09:31:12Z","state":"ready","step":20000,"total_steps":20000}]
```

`--json` prints one array, `[]` when no policy matches. Each object has these fields:

| Field | Type | Content |
|---|---|---|
| `id` | string | Policy id. |
| `policy_type` | string | Policy architecture. A value unknown to this version is passed through. |
| `state` | string | `queued`, `training`, `ready` or `failed`. A value unknown to this version is passed through. |
| `phase` | string | Sub-step of the current state as the API sends it. |
| `step` | integer | Training steps completed. |
| `total_steps` | integer | Training steps planned. |
| `dataset_id` | string | Id of the dataset the policy is trained on. |
| `dataset_version_id` | string | Id of the dataset version the policy is trained on. |
| `error` | string or null | Reason for a failed training. |
| `created_at` | string | Creation time, an ISO 8601 date-time as the API sends it. |
| `started_at` | string or null | Start of training. |
| `finished_at` | string or null | End of training. |

Schema: [`policies-list.json`](../reference/schemas/policies-list.json).

## Show a policy

```console
$ imify policies show run-3c9e1a7f
ID          run-3c9e1a7f
Type        act
State       ready
Phase       done
Progress    20000/20000
Dataset     0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13
Version     v3 (ver-41d2)
Task        Pick up the red cube
Embodiment  so100
Episodes    2
Created     2026-10-01T09:30:00Z
Started     2026-10-01T09:31:12Z
Finished    2026-10-01T11:02:40Z

CHECKPOINT     SIZE  LAST MODIFIED
010000      1.2 GiB  2026-10-01T10:15:02Z
020000      1.2 GiB  2026-10-01T11:01:55Z
```

`imify policies show <policy>` prints the policy, its lineage and its checkpoints, oldest first. The lineage is what the policy was trained on: the dataset, the dataset version and its episodes. A failed policy has an `Error` line with the reason. A policy without checkpoints prints `No checkpoints.` in place of the table.

### JSON

`--json` prints one object with the fields of `imify policies list --json` and two more:

| Field | Type | Content |
|---|---|---|
| `lineage` | object | `dataset_id`, `dataset_version_id`, `dataset_version_name`, `task`, `embodiment`, `embodiment_fingerprint`, `episodes` (integer) and `episode_ids` (array of strings). |
| `checkpoints` | array | One object per checkpoint, oldest first: `id`, `size` in bytes and `last_modified`, an ISO 8601 date-time as the API sends it. |

Schema: [`policies-show.json`](../reference/schemas/policies-show.json).

## Download a checkpoint

```console
$ imify policies download run-3c9e1a7f latest
Latest checkpoint of run-3c9e1a7f: 020000.
Downloaded 4 files (1.2 GiB) in 41s to run-3c9e1a7f-020000.
policy      run-3c9e1a7f
checkpoint  020000
path        /home/ada/run-3c9e1a7f-020000
files       4 (1.2 GiB)
```

`imify policies download <policy> <checkpoint>` downloads the checkpoint directory, with `pretrained_model/` and `training_state/`, so LeRobot loads it with `--policy.path`. `<checkpoint>` is a checkpoint id from `imify policies show`, or `latest` for the checkpoint with the highest step. With `latest`, the command names the checkpoint it chose before the download starts. The download works in every policy state and for every listed checkpoint.

The target directory is `./<policy>-<checkpoint>`, with the resolved id for `latest`. `-o <dir>` selects another directory. Missing directories are created. The result names the target directory as an absolute path.

Files download 4 at a time. Each file is written to `<file>.part`, checked against the size the API lists and renamed into place. While the download runs, a live view on stderr shows the progress, as for [`imify datasets upload`](datasets-upload.md). With `--json`, `--quiet`, `-v` or without a terminal on stderr, progress is written as events instead.

### Resume

Ctrl-C stops the download with `interrupted` and exit code 130. Running the same command again continues it: files that are complete are not fetched again, and a partial `.part` file continues where it stopped. The download refuses to start with `conflict` when the target directory holds a file that is not part of the checkpoint, a file of the checkpoint with another size, or a symbolic link. It changes none of them.

### Known limitation

While a policy is training, its newest checkpoint may still be uploading. Storage lists only finished files, so such a download can succeed with files missing. Download a checkpoint older than the newest one, or wait until the policy is `ready`, when you need a complete checkpoint.

### JSON

```console
$ imify policies download run-3c9e1a7f 020000 --json
{"path":"pretrained_model/config.json","size":15,"type":"file_started"}
{"path":"pretrained_model/config.json","size":15,"type":"file_finished"}
{"bytes":1319413953,"checkpoint":"020000","files":4,"path":"/home/ada/run-3c9e1a7f-020000","policy":"run-3c9e1a7f","resumed_bytes":0,"type":"result"}
```

`--json` writes one event per line to stdout, the result last:

| Type | Fields | Written when |
|---|---|---|
| `checkpoint_resolved` | `checkpoint` | first, when the argument is `latest` |
| `resumed` | `files_done`, `bytes_done` | files are complete in the target directory and are not fetched again |
| `file_started` | `path`, `size` | a file transfer starts |
| `file_retrying` | `path`, `attempt`, `reason` | a file is requested again after a failure |
| `file_finished` | `path`, `size` | a file is complete |
| `rate_limited` | `pause_ms` | storage rate-limited a request |
| `manifest_refreshed` | | storage rejected an expired URL, and the URLs were fetched again from the API |
| `result` | `policy`, `checkpoint`, `path`, `files`, `bytes`, `resumed_bytes` | the download is complete |

`path` in the result is the absolute target directory. `bytes` is the size of the checkpoint, `resumed_bytes` the size of the files that were complete before the download.

Schema: [`policies-download.json`](../reference/schemas/policies-download.json).

## Errors

| Code | Exit | Cause |
|---|---|---|
| `invalid_argument` | 2 | The API URL is invalid, `--state` names an unknown state, or the default target directory `<policy>-<checkpoint>` is not a plain directory name; pass `-o`. |
| `auth_required` | 3 | There is no login for the host, and `IMIFY_TOKEN` is not set. |
| `auth_expired` | 3 | The stored login has expired or was revoked. |
| `auth_rejected` | 3 | The API rejected the token. |
| `permission_denied` | 3 | The token lacks the permission to read policies. |
| `not_found` | 4 | No policy has the id. `imify policies download`: the policy has no checkpoint with the id, or no checkpoints. |
| `conflict` | 6 | `imify policies download`: the target directory holds a file that is not part of the checkpoint, a file of the checkpoint with another size, or a symbolic link. |
| `network`, `rate_limited` | 7 | The API could not be reached, or throttled the request. |
| `server` | 7 | The API failed. `imify policies download`: storage failed or sent another size than listed, the list of files has an unsafe path or URL or changed during the download, or storage rejected the URLs again after they were fetched anew. |
| `io` | 9 | `imify policies download`: a file or directory cannot be created, written or renamed, such as on a full disk or without permission. |
| `interrupted` | 130 | Ctrl-C during `imify policies download`. |
| `internal` | 1 | The API's response does not describe policies. |

All error codes: [JSON output and exit codes](../reference/json-output.md#errors).
