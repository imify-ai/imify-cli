# Datasets

[Validate a dataset](datasets-validate.md) before an upload and [upload it](datasets-upload.md) into an ingest session.

## List datasets

```console
$ imify datasets ls
ID                                    TASK                  FORMAT      EMBODIMENT  EPISODES  ACCEPTED
0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13  Pick up the red cube  lerobot_v3  so100             50        48
5e2d7a90-1c3b-4e6f-8a7d-9b0c1d2e3f45  Stack two blocks      lerobot_v3  koch              12        12
```

`imify datasets ls` prints every dataset visible to the user the access token belongs to. A dataset holds the episodes recorded for one task on one embodiment. `EPISODES` is the number of episodes in the dataset, `ACCEPTED` the number that passed quality control. Values are never truncated. Without datasets, the command prints `No datasets.` and exits with code 0.

The command requires `IMIFY_API_URL` and a login or `IMIFY_TOKEN`, as described in [Authentication](authentication.md).

### Filter

```sh
imify datasets ls --task cube
imify datasets ls --format lerobot_v3 --embodiment so100
```

| Flag | Keeps datasets |
|---|---|
| `--task <TEXT>` | whose task contains `TEXT`, ignoring case |
| `--format <FORMAT>` | with exactly this format |
| `--embodiment <EMBODIMENT>` | with exactly this embodiment label |

Filters combine: a dataset is listed when it matches every filter given.

### Sort

```sh
imify datasets ls --sort episodes --reverse
```

Without `--sort`, datasets are ordered by task, then embodiment, then id. `--sort` takes `id`, `task`, `format`, `embodiment`, `episodes` or `accepted`; datasets with the same value are ordered by id. Text compares by Unicode code point, so uppercase letters sort before lowercase. Numbers compare numerically. `--reverse` reverses the whole order.

### JSON

```console
$ imify datasets ls --embodiment so100 --json
[{"accepted":48,"action_dim":6,"cameras":["wrist","top"],"created_at":"2026-10-01T09:30:00Z","embodiment":"so100","embodiment_fingerprint":"f3a9c1","episodes":50,"format":"lerobot_v3","id":"0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13","task":"Pick up the red cube"}]
```

`--json` prints one array, `[]` when no dataset matches. Filters and order apply as in the table. Each object has these fields:

| Field | Type | Content |
|---|---|---|
| `id` | string | Dataset id, a UUID. |
| `task` | string | Task description the episodes were recorded for. |
| `format` | string | Dataset format, such as `lerobot_v3`. |
| `embodiment` | string | Display label of the robot embodiment. |
| `embodiment_fingerprint` | string | Fingerprint that identifies the embodiment. |
| `cameras` | array of strings | Camera names recorded in the episodes. |
| `action_dim` | integer | Number of action dimensions. |
| `created_at` | string | Creation time, an ISO 8601 date-time as the API sends it. |
| `episodes` | integer | Number of episodes. |
| `accepted` | integer | Number of episodes that passed quality control. |

Schema: [`datasets-ls.json`](../reference/schemas/datasets-ls.json).

### Errors

| Code | Exit | Cause |
|---|---|---|
| `invalid_argument` | 2 | `IMIFY_API_URL` is unset or invalid, or `--sort` names an unknown field. |
| `auth_required` | 3 | There is no login for the host, and `IMIFY_TOKEN` is not set. |
| `auth_expired` | 3 | The stored login has expired or was revoked. |
| `auth_rejected` | 3 | The API rejected the token. |
| `permission_denied` | 3 | The token lacks the permission to list datasets. |
| `network`, `server`, `rate_limited` | 7 | The API could not be reached, failed, or throttled the request. |
| `internal` | 1 | The API's response is not a list of datasets. |

All error codes: [JSON output and exit codes](../reference/json-output.md#errors).
