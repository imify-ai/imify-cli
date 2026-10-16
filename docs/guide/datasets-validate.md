# Validate a dataset

```console
$ imify datasets validate ./pick-cube
warning  unexpected_entry         notes.txt                              Not uploaded: only meta/, data/ and videos/ are uploaded.
error    episode_length_mismatch  data/chunk-000/episode_000000.parquet  Episode 0 has 10 rows; meta/episodes.jsonl declares 11.
1 error, 1 warning
error: The dataset has 1 error.
hint: Fix the errors listed in the report and run `imify datasets validate ./pick-cube` again.
```

`imify datasets validate <path>` checks a local LeRobot dataset in format v2.1 or v3.x before an upload. It reads the files only: it needs no network access, no token and no API URL. `<path>` is the dataset root, the directory that contains `meta/info.json`.

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
{"codebase_version":"v2.1","findings":[{"code":"unexpected_entry","message":"Not uploaded: only meta/, data/ and videos/ are uploaded.","path":"notes.txt","severity":"warning"},{"code":"episode_length_mismatch","message":"Episode 0 has 10 rows; meta/episodes.jsonl declares 11.","path":"data/chunk-000/episode_000000.parquet","severity":"error"}],"valid":false}
```

stderr:

```json
{"error":{"code":"dataset_invalid","message":"The dataset has 1 error.","hint":"Fix the errors listed in the report and run `imify datasets validate ./pick-cube` again."}}
```

`path` is `null` for a finding without a path. Schema: [`datasets-validate.json`](../reference/schemas/datasets-validate.json).

## Checks

The checks run in this order:

1. Top-level entries. `meta/`, `data/` and `videos/` are uploaded. Hidden entries, whose names start with `.`, are skipped without a finding.
2. `meta/info.json` exists and is a JSON object. `codebase_version` is `v2.1` or `v3.<n>`. `fps` is greater than 0. `features`, `data_path` and `chunks_size` are present with the expected types. `data_path` and `video_path` use only the placeholders of their format, written as `{name}` or, zero-padded, as `{name:06d}` with a width of at most 20. Each is at most 1024 bytes long and a relative path without a `..` component. The name of a video feature is not empty, has no `/` and does not start with `.`.
3. The episodes index, `meta/episodes.jsonl` for v2.1 or `meta/episodes/**/*.parquet` for v3.x, lists each episode with its `length`. The number of episodes equals `total_episodes`.
4. Every `*.parquet` file under `data/` and `meta/` has a readable footer.
5. The data file of each episode exists. Its rows for the episode equal the episode's `length`. Within the episode, `timestamp` increases strictly, and consecutive timestamps differ by 1/fps within 1e-4 s.
6. Every video file that `video_path` expands to exists. Video content is not checked.

Checks 3 to 6 need a readable `meta/info.json` with a supported version. Checks 5 and 6 need a readable episodes index.

Parquet columns are decoded when they are uncompressed or Snappy-compressed. For a data file with another codec, check 5 reports `content_not_checked` and skips the row counts and timestamps of that file.

Files are read only when they are regular files; symbolic links are not followed. A symbolic link, a FIFO or a device in place of `meta/info.json`, the episodes index or a parquet file is reported as unreadable, and in place of a video as `video_missing`, because the upload skips it. Limits:

| Input | Limit | Finding when exceeded |
|---|---|---|
| `meta/info.json`, `meta/episodes.jsonl` | 64 MiB | `info_malformed`, `episodes_index_unreadable` |
| Rows of a data file, from its footer | 10,000,000 | `content_not_checked` |
| Rows of an episodes index file, from its footer | 10,000,000 | `episodes_index_unreadable` |

A parquet file that makes the parquet reader fail in any other way is reported as `parquet_unreadable`.

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
| `video_missing` | warning | A video file that `video_path` expands to does not exist or is not a regular file. |
| `timestamp_spacing` | warning | Consecutive timestamps differ from 1/fps by more than 1e-4 s. |
| `content_not_checked` | warning | A parquet column uses a codec other than Snappy, or a data file has more than 10,000,000 rows, so the row counts and timestamps of that file are not checked. |
