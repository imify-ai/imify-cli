//! The episodes index: `meta/episodes.jsonl` for v2.1, `meta/episodes/**/*.parquet` for v3.x.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::info::{Info, Version};
use super::parquet;
use super::{Finding, FindingCode};

/// Path of the v2.1 episodes index relative to the dataset root.
pub(super) const V2_1_INDEX: &str = "meta/episodes.jsonl";
/// Directory of the v3.x episodes index relative to the dataset root.
pub(super) const V3_INDEX: &str = "meta/episodes";

/// One entry of the episodes index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Episode {
    pub(super) index: u64,
    /// Number of frames.
    pub(super) length: u64,
    /// The data file, v3.x only.
    pub(super) data: Option<FileRef>,
    /// The video file per video key, v3.x only.
    pub(super) videos: BTreeMap<String, FileRef>,
}

/// The chunk and file index of a v3.x data or video file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileRef {
    pub(super) chunk: u64,
    pub(super) file: u64,
}

/// The episodes index in index order, or `None` after reporting why it cannot be read.
pub(super) fn read(root: &Path, info: &Info, findings: &mut Vec<Finding>) -> Option<Vec<Episode>> {
    let (path, episodes) = match info.version {
        Version::V2_1 => (V2_1_INDEX, read_jsonl(root)),
        Version::V3 => (V3_INDEX, read_parquet(root, info, findings)),
    };
    match episodes {
        Ok(episodes) => Some(episodes),
        Err(reason) => {
            findings.push(Finding::new(
                FindingCode::EpisodesIndexUnreadable,
                path,
                format!("{reason}; the data and video checks are skipped."),
            ));
            None
        }
    }
}

/// The path of the episodes index relative to the dataset root.
pub(super) fn index_name(version: Version) -> &'static str {
    match version {
        Version::V2_1 => V2_1_INDEX,
        Version::V3 => V3_INDEX,
    }
}

fn read_jsonl(root: &Path) -> Result<Vec<Episode>, String> {
    let text = fs::read_to_string(root.join(V2_1_INDEX)).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("{V2_1_INDEX} does not exist")
        } else {
            format!("{V2_1_INDEX} cannot be read: {error}")
        }
    })?;
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(number, line)| {
            jsonl_episode(line).ok_or_else(|| {
                format!(
                    "line {} of {V2_1_INDEX} is not an object with an episode_index and a length",
                    number + 1
                )
            })
        })
        .collect()
}

fn jsonl_episode(line: &str) -> Option<Episode> {
    let value: Value = serde_json::from_str(line).ok()?;
    Some(Episode {
        index: value.get("episode_index")?.as_u64()?,
        length: value.get("length")?.as_u64()?,
        data: None,
        videos: BTreeMap::new(),
    })
}

fn read_parquet(
    root: &Path,
    info: &Info,
    findings: &mut Vec<Finding>,
) -> Result<Vec<Episode>, String> {
    let mut files = Vec::new();
    parquet::collect_parquet_files(root, Path::new(V3_INDEX), &mut files, findings);
    files.sort();
    if files.is_empty() {
        return Err(format!("{V3_INDEX}/ has no parquet file"));
    }
    let mut episodes = Vec::new();
    for file in files {
        episodes.extend(read_parquet_file(root, &file, info)?);
    }
    Ok(episodes)
}

fn read_parquet_file(root: &Path, file: &PathBuf, info: &Info) -> Result<Vec<Episode>, String> {
    let mut names = vec![
        "episode_index".to_owned(),
        "length".to_owned(),
        "data/chunk_index".to_owned(),
        "data/file_index".to_owned(),
    ];
    for key in &info.video_keys {
        names.push(format!("videos/{key}/chunk_index"));
        names.push(format!("videos/{key}/file_index"));
    }
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let unreadable = |reason: String| format!("{} {reason}", file.display());
    let reader = parquet::open(&root.join(file)).map_err(unreadable)?;
    let columns = parquet::read_columns(&reader, &names).map_err(unreadable)?;

    let rows = columns.first().map_or(0, Vec::len);
    (0..rows)
        .map(|row| {
            let cell = |column: usize| {
                columns
                    .get(column)
                    .and_then(|values| values.get(row))
                    .and_then(|value| value.as_u64())
                    .ok_or_else(|| {
                        unreadable(format!(
                            "column {} holds a value that is not a non-negative integer",
                            names[column]
                        ))
                    })
            };
            let file_ref = |column: usize| -> Result<FileRef, String> {
                Ok(FileRef {
                    chunk: cell(column)?,
                    file: cell(column + 1)?,
                })
            };
            let mut videos = BTreeMap::new();
            for (offset, key) in info.video_keys.iter().enumerate() {
                videos.insert(key.clone(), file_ref(4 + 2 * offset)?);
            }
            Ok(Episode {
                index: cell(0)?,
                length: cell(1)?,
                data: Some(file_ref(2)?),
                videos,
            })
        })
        .collect()
}
