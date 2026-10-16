//! `LeRobot` v2.1 layout: one data file and one video file per episode.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::layout::{self, Frames, Layout, Shape, TASK};

const DATA_PATH: &str = "data/chunk-{episode_chunk:03d}/episode_{episode_index:06d}.parquet";
const VIDEO_PATH: &str =
    "videos/chunk-{episode_chunk:03d}/{video_key}/episode_{episode_index:06d}.mp4";

pub(crate) fn layout(shape: &Shape) -> io::Result<Layout> {
    let mut files = BTreeMap::new();
    let mut episodes = Vec::new();
    let mut episodes_stats = Vec::new();

    for episode in 0..shape.episodes {
        let chunk = episode / layout::CHUNKS_SIZE;
        let frames = Frames::of_episodes(shape, [episode]);
        files.insert(
            PathBuf::from(format!(
                "data/chunk-{chunk:03}/episode_{episode:06}.parquet"
            )),
            frames.to_parquet()?,
        );
        for key in shape.video_keys() {
            let path = format!("videos/chunk-{chunk:03}/{key}/episode_{episode:06}.mp4");
            files.insert(
                PathBuf::from(&path),
                layout::placeholder_video(&path, shape.video_size),
            );
        }
        episodes.push(json!({
            "episode_index": episode,
            "tasks": [TASK],
            "length": shape.frames_per_episode,
        }));
        episodes_stats.push(json!({ "episode_index": episode, "stats": frames.stats() }));
    }

    files.insert(
        PathBuf::from("meta/episodes.jsonl"),
        layout::jsonl_file(episodes)?,
    );
    files.insert(
        PathBuf::from("meta/episodes_stats.jsonl"),
        layout::jsonl_file(episodes_stats)?,
    );
    files.insert(
        PathBuf::from("meta/tasks.jsonl"),
        layout::jsonl_file([json!({ "task_index": 0, "task": TASK })])?,
    );

    Ok(Layout {
        info: info(shape),
        files,
    })
}

fn info(shape: &Shape) -> Value {
    let videos = shape.episodes * u32::try_from(shape.cameras.len()).unwrap_or(u32::MAX);
    let mut info = layout::info(shape, "v2.1");
    info.insert(
        "total_chunks".into(),
        json!(shape.episodes.div_ceil(layout::CHUNKS_SIZE)),
    );
    info.insert("total_videos".into(), json!(videos));
    info.insert("data_path".into(), json!(DATA_PATH));
    let video_path = if shape.cameras.is_empty() {
        Value::Null
    } else {
        json!(VIDEO_PATH)
    };
    info.insert("video_path".into(), video_path);
    Value::Object(info)
}
