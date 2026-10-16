//! `LeRobot` v3.0 layout: all episodes in one data file and one video file per camera.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::layout::{self, Frames, Layout, Shape, TASK};
use crate::parquet::{self, Column};

const DATA_PATH: &str = "data/chunk-{chunk_index:03d}/file-{file_index:03d}.parquet";
const VIDEO_PATH: &str = "videos/{video_key}/chunk-{chunk_index:03d}/file-{file_index:03d}.mp4";

pub(crate) fn layout(shape: &Shape) -> io::Result<Layout> {
    let frames = Frames::of_episodes(shape, 0..shape.episodes);
    let mut files = BTreeMap::new();
    files.insert(
        PathBuf::from("data/chunk-000/file-000.parquet"),
        frames.to_parquet()?,
    );
    for key in shape.video_keys() {
        let path = format!("videos/{key}/chunk-000/file-000.mp4");
        files.insert(
            PathBuf::from(&path),
            layout::placeholder_video(&path, shape.video_size),
        );
    }
    files.insert(
        PathBuf::from("meta/episodes/chunk-000/file-000.parquet"),
        episodes(shape)?,
    );
    files.insert(
        PathBuf::from("meta/stats.json"),
        layout::json_file(&frames.stats())?,
    );
    files.insert(
        PathBuf::from("meta/tasks.parquet"),
        parquet::encode(&[
            Column::int64("task_index", vec![0]),
            Column::utf8("task", vec![TASK.to_owned()]),
        ])?,
    );
    Ok(Layout {
        info: info(shape),
        files,
    })
}

/// The episodes index: one row per episode with its frame range and the location of its
/// data and video.
fn episodes(shape: &Shape) -> io::Result<Vec<u8>> {
    let episodes: Vec<i64> = (0..shape.episodes).map(i64::from).collect();
    let frames = i64::from(shape.frames_per_episode);
    let zeros = || vec![0_i64; episodes.len()];

    let mut columns = vec![
        Column::int64("episode_index", episodes.clone()),
        Column::int64("length", vec![frames; episodes.len()]),
        Column::int64("data/chunk_index", zeros()),
        Column::int64("data/file_index", zeros()),
        Column::int64(
            "dataset_from_index",
            episodes.iter().map(|e| e * frames).collect(),
        ),
        Column::int64(
            "dataset_to_index",
            episodes.iter().map(|e| (e + 1) * frames).collect(),
        ),
    ];
    let duration = shape.episode_duration();
    #[expect(
        clippy::cast_precision_loss,
        reason = "episode counts are far below 2^24"
    )]
    let starts: Vec<f32> = episodes.iter().map(|&e| e as f32 * duration).collect();
    for key in shape.video_keys() {
        columns.push(Column::int64(format!("videos/{key}/chunk_index"), zeros()));
        columns.push(Column::int64(format!("videos/{key}/file_index"), zeros()));
        columns.push(Column::float(
            format!("videos/{key}/from_timestamp"),
            starts.clone(),
        ));
        columns.push(Column::float(
            format!("videos/{key}/to_timestamp"),
            starts.iter().map(|start| start + duration).collect(),
        ));
    }
    columns.push(Column::int64("meta/episodes/chunk_index", zeros()));
    columns.push(Column::int64("meta/episodes/file_index", zeros()));
    parquet::encode(&columns)
}

fn info(shape: &Shape) -> Value {
    let mut info = layout::info(shape, "v3.0");
    info.insert("data_files_size_in_mb".into(), json!(100));
    info.insert("video_files_size_in_mb".into(), json!(500));
    info.insert("data_path".into(), json!(DATA_PATH));
    let video_path = if shape.cameras.is_empty() {
        Value::Null
    } else {
        json!(VIDEO_PATH)
    };
    info.insert("video_path".into(), video_path);
    Value::Object(info)
}
