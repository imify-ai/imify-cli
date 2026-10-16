# Validate a dataset

```console
$ imify datasets validate ./pick-cube
error    episode_length_mismatch  data/chunk-000/episode_000000.parquet  Episode 0 has 10 rows; meta/episodes.jsonl declares 11.
warning  unexpected_entry         notes.txt                              Not uploaded: only meta/, data/ and videos/ are uploaded.
1 error, 1 warning
error: The dataset has 1 error.
hint: Fix the errors listed in the report and run `imify datasets validate ./pick-cube` again.
```

`imify datasets validate <path>` checks a local LeRobot dataset in format v2.1 or v3.x before an upload. It reads the files only: it needs no network access, no token and no `IMIFY_API_URL`. `<path>` is the dataset root, the directory that contains `meta/info.json`.

The report goes to stdout for valid and invalid datasets. Each line is one finding: severity, code, path relative to the dataset root, and message. The last line counts errors and warnings.

## Exit codes

| Exit | Meaning |
|---|---|
| 0 | No error findings. Warnings are allowed. |
| 2 | `<path>` is not a readable directory (`invalid_argument`). |
| 5 | At least one error finding (`dataset_invalid`). The error goes to stderr after the report. |

## JSON output

```console
$ imify datasets validate ./pick-cube --json
{"codebase_version":"v2.1","valid":false,"findings":[{"code":"episode_length_mismatch","severity":"error","path":"data/chunk-000/episode_000000.parquet","message":"Episode 0 has 10 rows; meta/episodes.jsonl declares 11."}]}
```

stderr:

```json
{"error":{"code":"dataset_invalid","message":"The dataset has 1 error.","hint":"Fix the errors listed in the report and run `imify datasets validate ./pick-cube` again."}}
```

`path` is `null` for a finding without a path. Schema: [`datasets-validate.json`](../reference/schemas/datasets-validate.json).

## Checks

The checks run in this order:

1. Top-level entries. `meta/`, `data/` and `videos/` are uploaded. Hidden entries, whose names start with `.`, are skipped without a finding.
2. `meta/info.json` exists and is a JSON object. `codebase_version` is `v2.1` or `v3.<n>`. `fps` is greater than 0. `features`, `data_path` and `chunks_size` are present with the expected types. `data_path` and `video_path` use only the placeholders of their format, written as `{name}` or `{name:0Nd}`.
3. The episodes index, `meta/episodes.jsonl` for v2.1 or `meta/episodes/**/*.parquet` for v3.x, lists each episode with its `length`. The number of episodes equals `total_episodes`.
4. Every `*.parquet` file under `data/` and `meta/` has a readable footer.
5. The data file of each episode exists. Its rows for the episode equal the episode's `length`. Within the episode, `timestamp` increases strictly, and consecutive timestamps differ by 1/fps within 1e-4 s.
6. Every video file that `video_path` expands to exists. Video content is not checked.

Checks 3 to 6 need a readable `meta/info.json` with a supported version. Checks 5 and 6 need a readable episodes index.

Parquet columns are decoded when they are uncompressed or Snappy-compressed. For a data file with another codec, check 5 reports `content_not_checked` and skips the row counts and timestamps of that file.

## Finding codes

An error means the platform rejects the dataset for the same reason. A warning means the platform accepts it.

| Code | Severity | Meaning |
|---|---|---|
| `info_missing` | error | `meta/info.json` does not exist. |
| `info_malformed` | error | `meta/info.json` is not JSON, or a required field is missing or has the wrong type. |
| `codebase_version_unsupported` | error | `codebase_version` is not `v2.1` or `v3.<n>`. |
| `fps_invalid` | error | `fps` is not greater than 0. |
| `parquet_unreadable` | error | A parquet file has no readable footer, or a column read fails. |
| `data_file_missing` | error | The data file of an episode does not exist. |
| `episode_length_mismatch` | error | An episode's row count differs from its `length` in the episodes index. |
| `timestamps_not_monotonic` | error | An episode's `timestamp` does not strictly increase. |
| `unexpected_entry` | warning | A top-level entry other than `meta/`, `data/`, `videos/` or a hidden entry. It is not uploaded. |
| `episode_count_mismatch` | warning | The number of episodes differs from `total_episodes`. |
| `episodes_index_unreadable` | warning | The episodes index is missing or does not parse. The data and video checks are skipped. |
| `video_missing` | warning | A video file that `video_path` expands to does not exist. |
| `timestamp_spacing` | warning | Consecutive timestamps differ from 1/fps by more than 1e-4 s. |
| `content_not_checked` | warning | A parquet column uses a codec other than Snappy, so the row counts and timestamps of that file are not checked. |
