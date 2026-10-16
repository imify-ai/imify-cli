use std::fs::{self, File};
use std::path::{Path, PathBuf};

use imify_test_support::{DatasetBuilder, Defect, Format, SyntheticDataset};
use parquet::file::reader::{FileReader, SerializedFileReader};
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
    for format in [Format::V2_1, Format::V3_0] {
        let dataset = DatasetBuilder::new(format).video_size(100).build().unwrap();

        let videos: Vec<&PathBuf> = dataset
            .files()
            .iter()
            .filter(|path| path.starts_with("videos"))
            .collect();
        assert!(!videos.is_empty());
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
