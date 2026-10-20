# Policies

A policy is the model trained on one dataset version. A checkpoint is a policy's state at one training step. `imify policies list` lists policies, `imify policies show` shows one policy with its checkpoints.

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

## Errors

| Code | Exit | Cause |
|---|---|---|
| `invalid_argument` | 2 | The API URL is invalid, or `--state` names an unknown state. |
| `auth_required` | 3 | There is no login for the host, and `IMIFY_TOKEN` is not set. |
| `auth_expired` | 3 | The stored login has expired or was revoked. |
| `auth_rejected` | 3 | The API rejected the token. |
| `permission_denied` | 3 | The token lacks the permission to read policies. |
| `not_found` | 4 | `imify policies show`: no policy has the id. |
| `network`, `server`, `rate_limited` | 7 | The API could not be reached, failed, or throttled the request. |
| `internal` | 1 | The API's response does not describe policies. |

All error codes: [JSON output and exit codes](../reference/json-output.md#errors).
