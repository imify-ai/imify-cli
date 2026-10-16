use std::fs::{self, File};
use std::path::{Path, PathBuf};

use imify_test_support::{Compression, DatasetBuilder, Defect, Format, SyntheticDataset};
use parquet::basic::{Compression as Codec, GzipLevel};
use parquet::file::metadata::ColumnChunkMetaData;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;
use serde_json::Value;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

fn paths(list: &[&str]) -> Vec<PathBuf> {
    list.iter().map(PathBuf::from).collect()
}

fn read_info(root: &Path) -> TestResult<Value> {
    Ok(serde_json::from_slice(&fs::read(
        root.join("meta/info.json"),
    )?)?)
}

#[test]
fn info_declares_the_configured_version_fps_episodes_and_frames() {
    for (format, version) in [(Format::V2_1, "v2.1"), (Format::V3_0, "v3.0")] {
        let dataset = DatasetBuilder::new(format)
            .episodes(3)
            .frames_per_episode(4)
            .fps(15)
            .build()
            .unwrap();

        let info = read_info(dataset.root()).unwrap();
        assert_eq!(info["codebase_version"], version);
        assert_eq!(info["fps"], 15);
        assert_eq!(info["total_episodes"], 3);
        assert_eq!(info["total_frames"], 12);
    }
}

#[test]
fn default_v2_1_build_writes_the_v2_1_layout() {
    let dataset = DatasetBuilder::new(Format::V2_1).build().unwrap();

    assert_eq!(
        dataset.files(),
        paths(&[
            "data/chunk-000/episode_000000.parquet",
            "data/chunk-000/episode_000001.parquet",
            "meta/episodes.jsonl",
            "meta/episodes_stats.jsonl",
            "meta/info.json",
            "meta/tasks.jsonl",
            "videos/chunk-000/observation.images.front/episode_000000.mp4",
            "videos/chunk-000/observation.images.front/episode_000001.mp4",
        ])
    );
}

#[test]
fn default_v3_0_build_writes_the_v3_0_layout() {
    let dataset = DatasetBuilder::new(Format::V3_0).build().unwrap();

    assert_eq!(
        dataset.files(),
        paths(&[
            "data/chunk-000/file-000.parquet",
            "meta/episodes/chunk-000/file-000.parquet",
            "meta/info.json",
            "meta/stats.json",
            "meta/tasks.parquet",
            "videos/observation.images.front/chunk-000/file-000.mp4",
        ])
    );
}

#[test]
fn v2_1_path_templates_resolve_to_written_files() {
    let dataset = DatasetBuilder::new(Format::V2_1).build().unwrap();
    let info = read_info(dataset.root()).unwrap();
    let data_path = info["data_path"].as_str().unwrap();
    let video_path = info["video_path"].as_str().unwrap();

    for episode in ["000000", "000001"] {
        let resolve = |template: &str| {
            template
                .replace("{episode_chunk:03d}", "000")
                .replace("{episode_index:06d}", episode)
                .replace("{video_key}", "observation.images.front")
        };
        assert!(dataset.root().join(resolve(data_path)).is_file());
        assert!(dataset.root().join(resolve(video_path)).is_file());
    }
}

#[test]
fn v3_0_path_templates_resolve_to_written_files() {
    let dataset = DatasetBuilder::new(Format::V3_0).build().unwrap();
    let info = read_info(dataset.root()).unwrap();
    let resolve = |key: &str| {
        info[key]
            .as_str()
            .unwrap()
            .replace("{chunk_index:03d}", "000")
            .replace("{file_index:03d}", "000")
            .replace("{video_key}", "observation.images.front")
    };

    assert!(dataset.root().join(resolve("data_path")).is_file());
    assert!(dataset.root().join(resolve("video_path")).is_file());
}

fn parquet_rows(path: &Path) -> TestResult<usize> {
    let reader = SerializedFileReader::new(File::open(path)?)?;
    let rows = reader.get_row_iter(None)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.len())
}

fn data_rows(dataset: &SyntheticDataset) -> TestResult<usize> {
    dataset
        .files()
        .iter()
        .filter(|path| path.starts_with("data"))
        .map(|path| parquet_rows(&dataset.root().join(path)))
        .sum()
}

#[test]
fn data_files_hold_one_row_per_frame() {
    for format in [Format::V2_1, Format::V3_0] {
        let dataset = DatasetBuilder::new(format)
            .episodes(3)
            .frames_per_episode(7)
            .build()
            .unwrap();

        assert_eq!(data_rows(&dataset).unwrap(), 21);
    }
}

#[test]
fn v2_1_episodes_jsonl_has_one_line_per_episode() {
    let dataset = DatasetBuilder::new(Format::V2_1)
        .episodes(3)
        .build()
        .unwrap();

    let episodes = fs::read_to_string(dataset.root().join("meta/episodes.jsonl")).unwrap();

    assert_eq!(episodes.lines().count(), 3);
}

#[test]
fn v3_0_episodes_parquet_has_one_row_per_episode() {
    let dataset = DatasetBuilder::new(Format::V3_0)
        .episodes(3)
        .build()
        .unwrap();

    let rows = parquet_rows(
        &dataset
            .root()
            .join("meta/episodes/chunk-000/file-000.parquet"),
    )
    .unwrap();

    assert_eq!(rows, 3);
}

fn contents(dataset: &SyntheticDataset) -> TestResult<Vec<(PathBuf, Vec<u8>)>> {
    let mut contents = Vec::new();
    for path in dataset.files() {
        contents.push((path.clone(), fs::read(dataset.root().join(path))?));
    }
    Ok(contents)
}

#[test]
fn builds_with_the_same_configuration_are_byte_identical() {
    for format in [Format::V2_1, Format::V3_0] {
        let builder = DatasetBuilder::new(format).cameras(&["front", "wrist"]);

        let first = builder.clone().build().unwrap();
        let second = builder.build().unwrap();

        assert_ne!(first.root(), second.root());
        assert_eq!(contents(&first).unwrap(), contents(&second).unwrap());
    }
}

#[test]
fn every_video_file_has_distinct_content() {
    let dataset = DatasetBuilder::new(Format::V2_1)
        .cameras(&["front", "wrist"])
        .build()
        .unwrap();

    let mut videos: Vec<Vec<u8>> = contents(&dataset)
        .unwrap()
        .into_iter()
        .filter(|(path, _)| path.starts_with("videos"))
        .map(|(_, bytes)| bytes)
        .collect();
    let count = videos.len();
    videos.sort();
    videos.dedup();

    assert_eq!(count, 4);
    assert_eq!(videos.len(), count);
}

fn walk(root: &Path, dir: &Path, found: &mut Vec<PathBuf>) -> TestResult<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            walk(root, &path, found)?;
        } else {
            found.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

#[test]
fn files_lists_every_file_under_root_relative_and_sorted() {
    for format in [Format::V2_1, Format::V3_0] {
        let dataset = DatasetBuilder::new(format).build().unwrap();
        let mut on_disk = Vec::new();
        walk(dataset.root(), dataset.root(), &mut on_disk).unwrap();
        on_disk.sort();

        assert_eq!(dataset.files(), on_disk);
    }
}

#[test]
fn dropping_the_dataset_removes_its_directory() {
    let dataset = DatasetBuilder::new(Format::V3_0).build().unwrap();
    let root = dataset.root().to_path_buf();
    assert!(root.is_dir());

    drop(dataset);

    assert!(!root.exists());
}

#[test]
fn write_to_writes_the_dataset_into_the_given_directory() {
    let dir = tempfile::tempdir().unwrap();

    let files = DatasetBuilder::new(Format::V2_1)
        .write_to(dir.path())
        .unwrap();

    assert!(files.contains(&PathBuf::from("meta/info.json")));
    for file in files {
        assert!(dir.path().join(file).is_file());
    }
}

#[test]
fn an_empty_camera_list_writes_no_videos_and_no_video_features() {
    for format in [Format::V2_1, Format::V3_0] {
        let dataset = DatasetBuilder::new(format).cameras(&[]).build().unwrap();

        let info = read_info(dataset.root()).unwrap();
        let dtypes: Vec<&Value> = info["features"]
            .as_object()
            .unwrap()
            .values()
            .map(|feature| &feature["dtype"])
            .collect();
        assert!(!dataset.root().join("videos").exists());
        assert!(!dtypes.contains(&&Value::from("video")));
        assert_eq!(info["video_path"], Value::Null);
    }
}

#[test]
fn every_camera_gets_a_video_feature_and_video_files() {
    let dataset = DatasetBuilder::new(Format::V3_0)
        .cameras(&["left", "wrist"])
        .build()
        .unwrap();

    let info = read_info(dataset.root()).unwrap();
    assert_eq!(
        info["features"]["observation.images.left"]["dtype"],
        "video"
    );
    assert_eq!(
        info["features"]["observation.images.wrist"]["dtype"],
        "video"
    );
    assert!(dataset.files().contains(&PathBuf::from(
        "videos/observation.images.wrist/chunk-000/file-000.mp4"
    )));
}

#[test]
fn video_size_sets_the_length_of_every_video_file() {
    for (format, video_count) in [(Format::V2_1, 2), (Format::V3_0, 1)] {
        let dataset = DatasetBuilder::new(format).video_size(100).build().unwrap();

        let videos: Vec<&PathBuf> = dataset
            .files()
            .iter()
            .filter(|path| path.starts_with("videos"))
            .collect();
        assert_eq!(videos.len(), video_count, "{format:?}");
        for video in videos {
            assert_eq!(fs::metadata(dataset.root().join(video)).unwrap().len(), 100);
        }
    }
}

/// Builds the default v2.1 dataset with and without `defect` and returns both.
fn with_and_without(defect: Defect) -> TestResult<(SyntheticDataset, SyntheticDataset)> {
    let builder = DatasetBuilder::new(Format::V2_1);
    Ok((builder.clone().build()?, builder.defect(defect).build()?))
}

/// Paths that are added, removed or changed between the two datasets.
fn changed_paths(before: &SyntheticDataset, after: &SyntheticDataset) -> TestResult<Vec<PathBuf>> {
    let before = contents(before)?;
    let after = contents(after)?;
    let mut changed: Vec<PathBuf> = before
        .iter()
        .filter(|entry| !after.contains(entry))
        .chain(after.iter().filter(|entry| !before.contains(entry)))
        .map(|(path, _)| path.clone())
        .collect();
    changed.sort();
    changed.dedup();
    Ok(changed)
}

/// `info.json` of both datasets with `key` removed.
fn info_without(
    key: &str,
    before: &SyntheticDataset,
    after: &SyntheticDataset,
) -> TestResult<(Value, Value)> {
    let strip = |dataset: &SyntheticDataset| -> TestResult<Value> {
        let mut info = read_info(dataset.root())?;
        if let Some(fields) = info.as_object_mut() {
            fields.remove(key);
        }
        Ok(info)
    };
    Ok((strip(before)?, strip(after)?))
}

#[test]
fn missing_info_omits_info_json() {
    let (before, after) = with_and_without(Defect::MissingInfo).unwrap();

    assert_eq!(
        changed_paths(&before, &after).unwrap(),
        paths(&["meta/info.json"])
    );
    assert!(!after.root().join("meta/info.json").exists());
}

#[test]
fn malformed_info_writes_info_json_that_is_not_json() {
    let (before, after) = with_and_without(Defect::MalformedInfo).unwrap();

    let bytes = fs::read(after.root().join("meta/info.json")).unwrap();
    assert_eq!(
        changed_paths(&before, &after).unwrap(),
        paths(&["meta/info.json"])
    );
    assert!(serde_json::from_slice::<Value>(&bytes).is_err());
}

#[test]
fn codebase_version_overrides_the_declared_version_only() {
    let (before, after) = with_and_without(Defect::CodebaseVersion("v1.6".to_owned())).unwrap();

    let (rest_before, rest_after) = info_without("codebase_version", &before, &after).unwrap();
    assert_eq!(
        changed_paths(&before, &after).unwrap(),
        paths(&["meta/info.json"])
    );
    assert_eq!(read_info(after.root()).unwrap()["codebase_version"], "v1.6");
    assert_eq!(rest_before, rest_after);
}

#[test]
fn fps_overrides_the_declared_fps_only() {
    let (before, after) = with_and_without(Defect::Fps(-1)).unwrap();

    let (rest_before, rest_after) = info_without("fps", &before, &after).unwrap();
    assert_eq!(
        changed_paths(&before, &after).unwrap(),
        paths(&["meta/info.json"])
    );
    assert_eq!(read_info(after.root()).unwrap()["fps"], -1);
    assert_eq!(rest_before, rest_after);
}

#[test]
fn total_episodes_overrides_the_declared_episode_count_only() {
    let (before, after) = with_and_without(Defect::TotalEpisodes(5)).unwrap();

    let (rest_before, rest_after) = info_without("total_episodes", &before, &after).unwrap();
    assert_eq!(
        changed_paths(&before, &after).unwrap(),
        paths(&["meta/info.json"])
    );
    assert_eq!(read_info(after.root()).unwrap()["total_episodes"], 5);
    assert_eq!(rest_before, rest_after);
}

#[test]
fn stray_file_adds_a_file_at_the_given_path() {
    let (before, after) = with_and_without(Defect::StrayFile("notes/todo.txt".to_owned())).unwrap();

    assert_eq!(
        changed_paths(&before, &after).unwrap(),
        paths(&["notes/todo.txt"])
    );
    assert!(after.root().join("notes/todo.txt").is_file());
}

#[test]
fn stray_file_outside_the_dataset_root_is_rejected() {
    for path in ["../escape.txt", "/tmp/escape.txt", ""] {
        let builder = DatasetBuilder::new(Format::V2_1).defect(Defect::StrayFile(path.to_owned()));

        let error = builder.build().unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{path}");
    }
}

#[test]
fn corrupt_data_file_truncates_the_first_data_file_to_16_bytes() {
    let (before, after) = with_and_without(Defect::CorruptDataFile).unwrap();
    let corrupt = after.root().join("data/chunk-000/episode_000000.parquet");

    assert_eq!(
        changed_paths(&before, &after).unwrap(),
        paths(&["data/chunk-000/episode_000000.parquet"])
    );
    assert_eq!(fs::metadata(&corrupt).unwrap().len(), 16);
    assert!(SerializedFileReader::new(File::open(&corrupt).unwrap()).is_err());
}

/// The `timestamp` values of every data file.
fn data_timestamps(dataset: &SyntheticDataset) -> TestResult<Vec<Field>> {
    let mut timestamps = Vec::new();
    for path in dataset
        .files()
        .iter()
        .filter(|path| path.starts_with("data"))
    {
        let reader = SerializedFileReader::new(File::open(dataset.root().join(path))?)?;
        for row in reader.get_row_iter(None)? {
            let row = row?;
            let timestamp = row
                .get_column_iter()
                .find(|(name, _)| *name == "timestamp")
                .map(|(_, field)| field.clone());
            timestamps.extend(timestamp);
        }
    }
    Ok(timestamps)
}

#[test]
fn text_timestamps_write_the_text_into_every_timestamp_of_every_data_file() {
    for format in [Format::V2_1, Format::V3_0] {
        let dataset = DatasetBuilder::new(format)
            .defect(Defect::TextTimestamps("late".to_owned()))
            .build()
            .unwrap();

        let timestamps = data_timestamps(&dataset).unwrap();

        assert_eq!(timestamps.len(), 20);
        assert!(
            timestamps
                .iter()
                .all(|field| *field == Field::Str("late".to_owned())),
            "{timestamps:?}"
        );
    }
}

#[test]
fn missing_video_omits_the_first_video_file() {
    let (before, after) = with_and_without(Defect::MissingVideo).unwrap();
    let missing = "videos/chunk-000/observation.images.front/episode_000000.mp4";

    assert_eq!(changed_paths(&before, &after).unwrap(), paths(&[missing]));
    assert!(!after.root().join(missing).exists());
}

#[test]
fn missing_video_omits_the_first_v3_0_video_file() {
    let dataset = DatasetBuilder::new(Format::V3_0)
        .defect(Defect::MissingVideo)
        .build()
        .unwrap();

    assert!(!dataset.root().join("videos").exists());
    assert!(
        dataset
            .files()
            .iter()
            .all(|path| !path.starts_with("videos"))
    );
}

#[test]
fn defects_combine() {
    let dataset = DatasetBuilder::new(Format::V3_0)
        .defect(Defect::Fps(0))
        .defect(Defect::MissingVideo)
        .build()
        .unwrap();

    assert_eq!(read_info(dataset.root()).unwrap()["fps"], 0);
    assert!(!dataset.root().join("videos").exists());
}

#[test]
fn missing_data_file_omits_the_first_data_file() {
    let (before, after) = with_and_without(Defect::MissingDataFile).unwrap();

    assert_eq!(
        changed_paths(&before, &after).unwrap(),
        paths(&["data/chunk-000/episode_000000.parquet"])
    );
}

#[test]
fn missing_data_file_omits_the_only_v3_0_data_file() {
    let dataset = DatasetBuilder::new(Format::V3_0)
        .defect(Defect::MissingDataFile)
        .build()
        .unwrap();

    assert!(!dataset.root().join("data").exists());
}

#[test]
fn missing_episodes_index_omits_episodes_jsonl_only() {
    let (before, after) = with_and_without(Defect::MissingEpisodesIndex).unwrap();

    assert_eq!(
        changed_paths(&before, &after).unwrap(),
        paths(&["meta/episodes.jsonl"])
    );
}

#[test]
fn missing_episodes_index_omits_the_v3_0_episodes_directory() {
    let dataset = DatasetBuilder::new(Format::V3_0)
        .defect(Defect::MissingEpisodesIndex)
        .build()
        .unwrap();

    assert!(!dataset.root().join("meta/episodes").exists());
    assert!(dataset.root().join("meta/info.json").is_file());
}

/// The values of the numeric column `name` of a parquet file, as `f64`.
fn column(path: &Path, name: &str) -> TestResult<Vec<f64>> {
    let reader = SerializedFileReader::new(File::open(path)?)?;
    let mut values = Vec::new();
    for row in reader.get_row_iter(None)? {
        let row = row?;
        for (column, field) in row.get_column_iter() {
            if column != name {
                continue;
            }
            #[expect(clippy::cast_precision_loss, reason = "test values are small")]
            let value = match field {
                Field::Long(value) => *value as f64,
                Field::Float(value) => f64::from(*value),
                other => return Err(format!("unexpected value {other:?}").into()),
            };
            values.push(value);
        }
    }
    Ok(values)
}

fn declared_lengths(dataset: &SyntheticDataset, format: Format) -> TestResult<Vec<f64>> {
    match format {
        Format::V2_1 => fs::read_to_string(dataset.root().join("meta/episodes.jsonl"))?
            .lines()
            .map(|line| {
                let episode: Value = serde_json::from_str(line)?;
                episode["length"]
                    .as_f64()
                    .ok_or_else(|| "length is not a number".into())
            })
            .collect(),
        Format::V3_0 => column(
            &dataset
                .root()
                .join("meta/episodes/chunk-000/file-000.parquet"),
            "length",
        ),
    }
}

#[test]
fn episode_length_declares_one_frame_more_for_episode_0() {
    for format in [Format::V2_1, Format::V3_0] {
        let dataset = DatasetBuilder::new(format)
            .episodes(3)
            .frames_per_episode(4)
            .defect(Defect::EpisodeLength)
            .build()
            .unwrap();

        assert_eq!(
            declared_lengths(&dataset, format).unwrap(),
            [5.0, 4.0, 4.0],
            "{format:?}"
        );
        assert_eq!(data_rows(&dataset).unwrap(), 12, "{format:?}");
    }
}

#[test]
fn episode_length_changes_the_v2_1_episodes_index_only() {
    let (before, after) = with_and_without(Defect::EpisodeLength).unwrap();

    assert_eq!(
        changed_paths(&before, &after).unwrap(),
        paths(&["meta/episodes.jsonl"])
    );
}

/// The timestamps of the first data file, in row order.
fn first_timestamps(dataset: &SyntheticDataset) -> TestResult<Vec<f64>> {
    let first = dataset
        .files()
        .iter()
        .find(|path| path.starts_with("data"))
        .ok_or("no data file")?;
    column(&dataset.root().join(first), "timestamp")
}

fn timestamps_with(format: Format, defect: Defect) -> TestResult<Vec<f64>> {
    let dataset = DatasetBuilder::new(format)
        .episodes(2)
        .frames_per_episode(4)
        .fps(4)
        .defect(defect)
        .build()?;
    first_timestamps(&dataset)
}

#[test]
fn non_monotonic_timestamps_swaps_the_first_two_timestamps_of_episode_0() {
    let v2_1 = timestamps_with(Format::V2_1, Defect::NonMonotonicTimestamps).unwrap();
    let v3_0 = timestamps_with(Format::V3_0, Defect::NonMonotonicTimestamps).unwrap();

    assert_eq!(v2_1, [0.25, 0.0, 0.5, 0.75]);
    assert_eq!(v3_0, [0.25, 0.0, 0.5, 0.75, 0.0, 0.25, 0.5, 0.75]);
}

#[test]
fn timestamp_gap_delays_episode_0_after_its_first_frame_by_one_frame() {
    let v2_1 = timestamps_with(Format::V2_1, Defect::TimestampGap).unwrap();
    let v3_0 = timestamps_with(Format::V3_0, Defect::TimestampGap).unwrap();

    assert_eq!(v2_1, [0.0, 0.5, 0.75, 1.0]);
    assert_eq!(v3_0, [0.0, 0.5, 0.75, 1.0, 0.0, 0.25, 0.5, 0.75]);
}

#[test]
fn non_monotonic_timestamps_change_the_first_v2_1_data_file_only() {
    let (before, after) = with_and_without(Defect::NonMonotonicTimestamps).unwrap();

    assert_eq!(
        changed_paths(&before, &after).unwrap(),
        paths(&["data/chunk-000/episode_000000.parquet"])
    );
}

#[test]
fn timestamp_gap_changes_the_first_v2_1_data_file_and_the_episode_stats() {
    let (before, after) = with_and_without(Defect::TimestampGap).unwrap();

    assert_eq!(
        changed_paths(&before, &after).unwrap(),
        paths(&[
            "data/chunk-000/episode_000000.parquet",
            "meta/episodes_stats.jsonl"
        ])
    );
}

fn codecs(path: &Path) -> TestResult<Vec<Codec>> {
    let reader = SerializedFileReader::new(File::open(path)?)?;
    Ok(reader
        .metadata()
        .row_groups()
        .iter()
        .flat_map(|group| group.columns().iter().map(ColumnChunkMetaData::compression))
        .collect())
}

#[test]
fn data_compression_sets_the_codec_of_data_files_only() {
    for format in [Format::V2_1, Format::V3_0] {
        for (compression, codec) in [
            (Compression::Uncompressed, Codec::UNCOMPRESSED),
            (Compression::Snappy, Codec::SNAPPY),
            (Compression::Gzip, Codec::GZIP(GzipLevel::default())),
        ] {
            let dataset = DatasetBuilder::new(format)
                .data_compression(compression)
                .build()
                .unwrap();

            for path in dataset.files() {
                if path.extension().is_none_or(|ext| ext != "parquet") {
                    continue;
                }
                let expected = if path.starts_with("data") {
                    codec
                } else {
                    Codec::UNCOMPRESSED
                };
                let found = codecs(&dataset.root().join(path)).unwrap();
                assert!(
                    found.iter().all(|found| *found == expected),
                    "{format:?} {path:?}: {found:?}"
                );
            }
            assert_eq!(
                data_rows(&dataset).unwrap(),
                20,
                "{format:?} {compression:?}"
            );
        }
    }
}
