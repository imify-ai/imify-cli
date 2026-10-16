//! Content shared by the v2.1 and v3.0 layouts.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::parquet::{self, Column};

/// Rows per chunk directory, declared as `chunks_size` in `info.json`.
pub(crate) const CHUNKS_SIZE: u32 = 1000;

/// The single task every frame belongs to.
pub(crate) const TASK: &str = "Pick up the cube.";

/// Dataset dimensions, independent of the format.
#[derive(Debug, Clone)]
pub(crate) struct Shape {
    pub(crate) episodes: u32,
    pub(crate) frames_per_episode: u32,
    pub(crate) fps: u32,
    pub(crate) cameras: Vec<String>,
    pub(crate) video_size: usize,
}

impl Shape {
    pub(crate) fn total_frames(&self) -> u64 {
        u64::from(self.episodes) * u64::from(self.frames_per_episode)
    }

    pub(crate) fn video_keys(&self) -> impl Iterator<Item = String> + '_ {
        self.cameras
            .iter()
            .map(|camera| format!("observation.images.{camera}"))
    }

    /// Duration of one episode in seconds.
    pub(crate) fn episode_duration(&self) -> f32 {
        frames_to_seconds(self.frames_per_episode, self.fps)
    }
}

/// A dataset layout before defects are applied: `info.json` as a value, every other
/// file as bytes keyed by its path relative to the dataset root.
#[derive(Debug)]
pub(crate) struct Layout {
    pub(crate) info: Value,
    pub(crate) files: BTreeMap<PathBuf, Vec<u8>>,
}

/// Fields of `info.json` common to both formats.
pub(crate) fn info(shape: &Shape, codebase_version: &str) -> Map<String, Value> {
    let mut info = Map::new();
    info.insert("codebase_version".into(), json!(codebase_version));
    info.insert("robot_type".into(), Value::Null);
    info.insert("total_episodes".into(), json!(shape.episodes));
    info.insert("total_frames".into(), json!(shape.total_frames()));
    info.insert("total_tasks".into(), json!(1));
    info.insert("chunks_size".into(), json!(CHUNKS_SIZE));
    info.insert("fps".into(), json!(shape.fps));
    info.insert(
        "splits".into(),
        json!({ "train": format!("0:{}", shape.episodes) }),
    );
    info.insert("features".into(), features(shape));
    info
}

fn features(shape: &Shape) -> Value {
    let scalar = |dtype: &str| json!({ "dtype": dtype, "shape": [1], "names": null });
    let mut features = Map::new();
    features.insert("timestamp".into(), scalar("float32"));
    for name in INT64_COLUMNS {
        features.insert(name.into(), scalar("int64"));
    }
    for key in shape.video_keys() {
        features.insert(
            key,
            json!({
                "dtype": "video",
                "shape": [480, 640, 3],
                "names": ["height", "width", "channels"],
                "info": {
                    "video.height": 480,
                    "video.width": 640,
                    "video.codec": "av1",
                    "video.pix_fmt": "yuv420p",
                    "video.is_depth_map": false,
                    "video.fps": shape.fps,
                    "video.channels": 3,
                    "has_audio": false,
                },
            }),
        );
    }
    Value::Object(features)
}

const INT64_COLUMNS: [&str; 4] = ["frame_index", "episode_index", "index", "task_index"];

/// Frame rows of the given episodes, in episode order.
#[derive(Debug, Default)]
pub(crate) struct Frames {
    timestamp: Vec<f32>,
    frame_index: Vec<i64>,
    episode_index: Vec<i64>,
    index: Vec<i64>,
    task_index: Vec<i64>,
}

impl Frames {
    pub(crate) fn of_episodes(shape: &Shape, episodes: impl IntoIterator<Item = u32>) -> Self {
        let mut frames = Self::default();
        for episode in episodes {
            for frame in 0..shape.frames_per_episode {
                let index =
                    u64::from(episode) * u64::from(shape.frames_per_episode) + u64::from(frame);
                frames.timestamp.push(frames_to_seconds(frame, shape.fps));
                frames.frame_index.push(i64::from(frame));
                frames.episode_index.push(i64::from(episode));
                frames.index.push(to_i64(index));
                frames.task_index.push(0);
            }
        }
        frames
    }

    /// The frames as a data parquet file.
    pub(crate) fn to_parquet(&self) -> io::Result<Vec<u8>> {
        parquet::encode(&[
            Column::float("timestamp", self.timestamp.clone()),
            Column::int64("frame_index", self.frame_index.clone()),
            Column::int64("episode_index", self.episode_index.clone()),
            Column::int64("index", self.index.clone()),
            Column::int64("task_index", self.task_index.clone()),
        ])
    }

    /// Per-feature `min`, `max`, `mean`, `std` and `count` of the scalar features.
    pub(crate) fn stats(&self) -> Value {
        let columns: [(&str, Vec<f64>); 5] = [
            (
                "timestamp",
                self.timestamp.iter().copied().map(f64::from).collect(),
            ),
            ("frame_index", to_f64(&self.frame_index)),
            ("episode_index", to_f64(&self.episode_index)),
            ("index", to_f64(&self.index)),
            ("task_index", to_f64(&self.task_index)),
        ];
        let stats = columns
            .into_iter()
            .map(|(name, values)| (name.to_owned(), column_stats(&values)))
            .collect::<Map<_, _>>();
        Value::Object(stats)
    }
}

fn column_stats(values: &[f64]) -> Value {
    let count = values.len();
    if count == 0 {
        return json!({ "min": [0.0], "max": [0.0], "mean": [0.0], "std": [0.0], "count": [0] });
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "frame counts are far below 2^52"
    )]
    let n = count as f64;
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mean = values.iter().sum::<f64>() / n;
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / n;
    json!({
        "min": [min],
        "max": [max],
        "mean": [mean],
        "std": [variance.sqrt()],
        "count": [count],
    })
}

fn to_f64(values: &[i64]) -> Vec<f64> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "frame indices are far below 2^52"
    )]
    values.iter().map(|&value| value as f64).collect()
}

/// Converts an in-range `u64` to `i64`, saturating at `i64::MAX`.
pub(crate) fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn frames_to_seconds(frames: u32, fps: u32) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "frame counts are far below 2^24"
    )]
    let seconds = frames as f32 / fps.max(1) as f32;
    seconds
}

/// Placeholder video content: `size` pseudo-random bytes seeded from `path`, so that every
/// path gets distinct, reproducible bytes.
pub(crate) fn placeholder_video(path: &str, size: usize) -> Vec<u8> {
    let mut state = fnv1a(path.as_bytes()) | 1;
    let mut bytes = Vec::with_capacity(size);
    while bytes.len() < size {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let remaining = size - bytes.len();
        bytes.extend_from_slice(&state.to_le_bytes()[..remaining.min(8)]);
    }
    bytes
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Serializes `value` as pretty JSON terminated by a line feed.
pub(crate) fn json_file(value: &Value) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Serializes `lines` as JSON Lines, one compact object per line.
pub(crate) fn jsonl_file(lines: impl IntoIterator<Item = Value>) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for line in lines {
        serde_json::to_writer(&mut bytes, &line).map_err(io::Error::other)?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}
