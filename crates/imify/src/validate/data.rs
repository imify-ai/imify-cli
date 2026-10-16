//! The data of each episode: file existence, row count and timestamps.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::episodes::{self, Episode};
use super::info::{Info, Version};
use super::parquet::{self, Number};
use super::template::Arg;
use super::{Finding, FindingCode};

/// Largest accepted deviation of a timestamp step from 1/fps, in seconds.
const SPACING_TOLERANCE: f64 = 1e-4;

/// Checks the data file of every episode. Files in `unreadable` are already reported and
/// are skipped.
pub(super) fn check(
    root: &Path,
    info: &Info,
    episodes: &[Episode],
    unreadable: &BTreeSet<PathBuf>,
    findings: &mut Vec<Finding>,
) {
    for (path, episodes) in by_data_file(info, episodes) {
        if unreadable.contains(&path) {
            continue;
        }
        let checker = FileChecker {
            info,
            path: &path,
            episodes: &episodes,
        };
        checker.check(root, findings);
    }
}

/// Groups the episodes by the data file `data_path` expands to for them.
fn by_data_file<'a>(info: &Info, episodes: &'a [Episode]) -> BTreeMap<PathBuf, Vec<&'a Episode>> {
    let mut files: BTreeMap<PathBuf, Vec<&Episode>> = BTreeMap::new();
    for episode in episodes {
        let path = match (info.version, episode.data) {
            (Version::V2_1, _) => info.data_path.expand(&[
                ("episode_chunk", Arg::Int(episode.index / info.chunks_size)),
                ("episode_index", Arg::Int(episode.index)),
            ]),
            (Version::V3, Some(file)) => info.data_path.expand(&[
                ("chunk_index", Arg::Int(file.chunk)),
                ("file_index", Arg::Int(file.file)),
            ]),
            (Version::V3, None) => continue,
        };
        files.entry(path).or_default().push(episode);
    }
    files
}

/// The timestamps of a data file.
enum Timestamps {
    /// A v2.1 file: the timestamps of the one episode the file holds. Every episode that
    /// `data_path` maps to the file is checked against them.
    Episode(Vec<f64>),
    /// A v3.x file: the timestamps of each episode, by episode index.
    ByEpisode(BTreeMap<u64, Vec<f64>>),
}

impl Timestamps {
    /// The timestamps of the episode `index`, empty when the file has none.
    fn of(&self, index: u64) -> &[f64] {
        match self {
            Self::Episode(timestamps) => timestamps,
            Self::ByEpisode(by_episode) => by_episode.get(&index).map_or(&[], Vec::as_slice),
        }
    }
}

/// Checks one data file and the episodes it holds.
struct FileChecker<'a> {
    info: &'a Info,
    path: &'a Path,
    episodes: &'a [&'a Episode],
}

impl FileChecker<'_> {
    fn check(&self, root: &Path, findings: &mut Vec<Finding>) {
        match self.read_timestamps(root) {
            Ok(timestamps) => {
                for episode in self.episodes {
                    self.check_episode(episode, timestamps.of(episode.index), findings);
                }
            }
            Err(finding) => findings.push(finding),
        }
    }

    /// The timestamps in the file, or the finding that stops the check.
    fn read_timestamps(&self, root: &Path) -> Result<Timestamps, Finding> {
        let full_path = root.join(self.path);
        if fs::symlink_metadata(&full_path).is_err_and(|error| error.kind() == ErrorKind::NotFound)
        {
            return Err(self.finding(FindingCode::DataFileMissing, self.missing_message()));
        }
        let unreadable = |reason| {
            self.finding(
                FindingCode::ParquetUnreadable,
                format!("The file {reason}."),
            )
        };
        let reader = parquet::open(&full_path).map_err(unreadable)?;
        let columns = self.columns();
        if let Some((column, codec)) = parquet::unsupported_codec(&reader, &columns) {
            return Err(self.finding(
                FindingCode::ContentNotChecked,
                format!(
                    "Column {column} uses the codec {codec}, which is not decoded; the row counts and timestamps of this file are not checked."
                ),
            ));
        }
        let rows = parquet::row_count(&reader).map_err(unreadable)?;
        if rows > parquet::MAX_ROWS {
            return Err(self.finding(
                FindingCode::ContentNotChecked,
                format!(
                    "The file declares {rows} rows; the row counts and timestamps of files with more than {} rows are not checked.",
                    parquet::MAX_ROWS
                ),
            ));
        }
        parquet::read_columns(&reader, &columns)
            .and_then(|values| self.timestamps(values))
            .map_err(unreadable)
    }

    /// The columns read from the file.
    fn columns(&self) -> Vec<&'static str> {
        match self.info.version {
            Version::V2_1 => vec!["timestamp"],
            Version::V3 => vec!["episode_index", "timestamp"],
        }
    }

    /// The timestamps of the file in row order: of the one episode of a v2.1 file, or
    /// grouped by `episode_index` for v3.x.
    fn timestamps(&self, columns: Vec<Vec<Number>>) -> Result<Timestamps, String> {
        let mut columns = columns.into_iter();
        match self.info.version {
            Version::V2_1 => {
                let timestamps = columns.next().unwrap_or_default();
                Ok(Timestamps::Episode(
                    timestamps.into_iter().map(Number::as_f64).collect(),
                ))
            }
            Version::V3 => {
                let indices = columns.next().unwrap_or_default();
                let timestamps = columns.next().unwrap_or_default();
                let mut by_episode: BTreeMap<u64, Vec<f64>> = BTreeMap::new();
                for (index, timestamp) in indices.into_iter().zip(timestamps) {
                    let index = index.as_u64().ok_or_else(|| {
                        "column episode_index holds a value that is not a non-negative integer"
                            .to_owned()
                    })?;
                    by_episode
                        .entry(index)
                        .or_default()
                        .push(timestamp.as_f64());
                }
                Ok(Timestamps::ByEpisode(by_episode))
            }
        }
    }

    fn check_episode(&self, episode: &Episode, timestamps: &[f64], findings: &mut Vec<Finding>) {
        let rows = timestamps.len();
        if u64::try_from(rows).ok() != Some(episode.length) {
            findings.push(self.finding(
                FindingCode::EpisodeLengthMismatch,
                format!(
                    "Episode {} has {rows} rows; {} declares {}.",
                    episode.index,
                    episodes::index_name(self.info.version),
                    episode.length
                ),
            ));
        }
        let steps = || {
            timestamps
                .windows(2)
                .enumerate()
                .map(|(frame, pair)| (frame, pair[0], pair[1]))
        };
        if let Some((frame, before, after)) = steps().find(|(_, before, after)| after <= before) {
            findings.push(self.finding(
                FindingCode::TimestampsNotMonotonic,
                format!(
                    "Episode {}: frame {} has timestamp {after} s, not later than {before} s of frame {frame}.",
                    episode.index,
                    frame + 1
                ),
            ));
            return;
        }
        let Some(fps) = self.info.fps else {
            return;
        };
        let period = 1.0 / fps;
        if let Some((frame, before, after)) =
            steps().find(|(_, before, after)| ((after - before) - period).abs() > SPACING_TOLERANCE)
        {
            findings.push(self.finding(
                FindingCode::TimestampSpacing,
                format!(
                    "Episode {}: frames {frame} and {} are {:.4} s apart; at {fps} fps the step is {period:.4} s.",
                    episode.index,
                    frame + 1,
                    after - before
                ),
            ));
        }
    }

    fn missing_message(&self) -> String {
        match self.episodes {
            [episode] => format!("The data file of episode {} does not exist.", episode.index),
            episodes => format!(
                "The data file of {} episodes does not exist.",
                episodes.len()
            ),
        }
    }

    fn finding(&self, code: FindingCode, message: String) -> Finding {
        Finding::new(code, self.path, message)
    }
}
