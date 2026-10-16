//! Offline structural checks of `LeRobot` v2.1 and v3.x datasets.
//!
//! ```no_run
//! use std::path::Path;
//!
//! let report = imify::validate::validate_dataset(Path::new("./pick-cube"))?;
//! for finding in &report.findings {
//!     println!("{} {}", finding.code.as_str(), finding.message);
//! }
//! assert!(report.is_valid());
//! # Ok::<(), imify::Error>(())
//! ```
//!
//! A finding is an [`Severity::Error`] when the platform rejects the dataset for the same
//! reason, and a [`Severity::Warning`] otherwise. Video files are checked for existence
//! only. Parquet columns compressed with a codec other than Snappy are not decoded.

mod data;
mod episodes;
mod info;
mod parquet;
mod template;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::Error;
use episodes::Episode;
use info::{Info, Version};
use template::Arg;

/// Top-level entries of a dataset that the upload includes.
const UPLOADED_ENTRIES: [&str; 3] = ["meta", "data", "videos"];

/// Checks the dataset in `root` and returns every finding.
///
/// The checks run in this order: top-level entries, `meta/info.json`, the episodes index,
/// parquet footers, the data of each episode, and the video files. Checks that need
/// `meta/info.json` are skipped when it is missing, malformed or declares an unsupported
/// `codebase_version`. The data and video checks are skipped when the episodes index
/// cannot be read.
///
/// # Errors
///
/// Returns [`Error::Io`] when `root` is not a readable directory. A problem inside the
/// dataset, including a file that cannot be read, is a [`Finding`].
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
///
/// use imify::validate::validate_dataset;
///
/// let report = validate_dataset(Path::new("./pick-cube"))?;
/// println!("{} errors", report.errors().count());
/// # Ok::<(), imify::Error>(())
/// ```
pub fn validate_dataset(root: &Path) -> Result<Report, Error> {
    let mut findings = Vec::new();
    check_entries(root, &mut findings)?;
    let declared = info::read(root, &mut findings);
    let episodes = declared
        .info
        .as_ref()
        .and_then(|info| episodes::read(root, info, &mut findings));
    if let (Some(info), Some(episodes)) = (&declared.info, &episodes) {
        check_episode_count(info, episodes, &mut findings);
    }
    let unreadable = parquet::check_footers(root, &mut findings);
    if let (Some(info), Some(episodes)) = (&declared.info, &episodes) {
        data::check(root, info, episodes, &unreadable, &mut findings);
        check_videos(root, info, episodes, &mut findings);
    }
    Ok(Report {
        codebase_version: declared.codebase_version,
        findings,
    })
}

/// The result of [`validate_dataset`].
///
/// # Examples
///
/// ```no_run
/// # let report = imify::validate::validate_dataset(std::path::Path::new("."))?;
/// if !report.is_valid() {
///     for finding in report.errors() {
///         eprintln!("{}: {}", finding.code.as_str(), finding.message);
///     }
/// }
/// # Ok::<(), imify::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Report {
    /// The `codebase_version` declared in `meta/info.json`, when it is a string.
    pub codebase_version: Option<String>,
    /// Every finding, in check order.
    pub findings: Vec<Finding>,
}

impl Report {
    /// Returns `true` when no finding has [`Severity::Error`].
    ///
    /// ```no_run
    /// # let report = imify::validate::validate_dataset(std::path::Path::new("."))?;
    /// let upload_allowed = report.is_valid();
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.errors().next().is_none()
    }

    /// The findings with [`Severity::Error`], in check order.
    ///
    /// ```no_run
    /// # let report = imify::validate::validate_dataset(std::path::Path::new("."))?;
    /// let errors = report.errors().count();
    /// # Ok::<(), imify::Error>(())
    /// ```
    pub fn errors(&self) -> impl Iterator<Item = &Finding> {
        self.with_severity(Severity::Error)
    }

    /// The findings with [`Severity::Warning`], in check order.
    ///
    /// ```no_run
    /// # let report = imify::validate::validate_dataset(std::path::Path::new("."))?;
    /// let warnings = report.warnings().count();
    /// # Ok::<(), imify::Error>(())
    /// ```
    pub fn warnings(&self) -> impl Iterator<Item = &Finding> {
        self.with_severity(Severity::Warning)
    }

    fn with_severity(&self, severity: Severity) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(move |finding| finding.severity() == severity)
    }
}

/// One problem found in a dataset.
///
/// # Examples
///
/// ```no_run
/// # let report = imify::validate::validate_dataset(std::path::Path::new("."))?;
/// for finding in &report.findings {
///     let path = finding.path.as_deref().map(|path| path.display().to_string());
///     println!("{} {:?} {}", finding.severity().as_str(), path, finding.message);
/// }
/// # Ok::<(), imify::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Finding {
    /// What kind of problem it is.
    pub code: FindingCode,
    /// The file or directory concerned, relative to the dataset root.
    pub path: Option<PathBuf>,
    /// What is wrong, as one or more sentences.
    pub message: String,
}

impl Finding {
    fn new(code: FindingCode, path: impl Into<PathBuf>, message: String) -> Self {
        Self {
            code,
            path: Some(path.into()),
            message,
        }
    }

    /// The severity of the finding's code.
    ///
    /// ```no_run
    /// use imify::validate::Severity;
    ///
    /// # let report = imify::validate::validate_dataset(std::path::Path::new("."))?;
    /// let fatal = report
    ///     .findings
    ///     .iter()
    ///     .any(|finding| finding.severity() == Severity::Error);
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn severity(&self) -> Severity {
        self.code.severity()
    }
}

/// Whether a finding makes the dataset invalid.
///
/// ```
/// use imify::validate::{FindingCode, Severity};
///
/// assert_eq!(FindingCode::VideoMissing.severity(), Severity::Warning);
/// assert_eq!(Severity::Warning.as_str(), "warning");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// The platform rejects the dataset for the same reason.
    Error,
    /// The platform accepts the dataset, but part of it is not uploaded or may not be
    /// usable.
    Warning,
}

impl Severity {
    /// The stable `snake_case` name: `error` or `warning`.
    ///
    /// ```
    /// assert_eq!(imify::validate::Severity::Error.as_str(), "error");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

/// The kind of a finding. Each code has a fixed [`Severity`].
///
/// ```
/// use imify::validate::FindingCode;
///
/// assert_eq!(FindingCode::EpisodeLengthMismatch.as_str(), "episode_length_mismatch");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FindingCode {
    /// `meta/info.json` does not exist.
    InfoMissing,
    /// `meta/info.json` is not JSON, or a required field is missing or has the wrong type.
    InfoMalformed,
    /// `codebase_version` is not `v2.1` or `v3.<n>`.
    CodebaseVersionUnsupported,
    /// `fps` is not greater than 0.
    FpsInvalid,
    /// A parquet file has no readable footer, or a column read fails.
    ParquetUnreadable,
    /// The data file of an episode does not exist.
    DataFileMissing,
    /// An episode's row count differs from its `length` in the episodes index.
    EpisodeLengthMismatch,
    /// An episode's `timestamp` does not strictly increase.
    TimestampsNotMonotonic,
    /// A top-level entry other than `meta/`, `data/`, `videos/` or a hidden entry. It is
    /// not uploaded.
    UnexpectedEntry,
    /// The number of episodes differs from `total_episodes`.
    EpisodeCountMismatch,
    /// The episodes index is missing or does not parse.
    EpisodesIndexUnreadable,
    /// A video file that `video_path` expands to does not exist.
    VideoMissing,
    /// Consecutive timestamps differ from 1/fps by more than 1e-4 s.
    TimestampSpacing,
    /// A parquet column uses a compression codec that is not decoded, so the row counts and
    /// timestamps of that file are not checked.
    ContentNotChecked,
}

impl FindingCode {
    /// Every code, in documentation order.
    #[cfg(test)]
    pub(crate) const ALL: [Self; 14] = [
        Self::InfoMissing,
        Self::InfoMalformed,
        Self::CodebaseVersionUnsupported,
        Self::FpsInvalid,
        Self::ParquetUnreadable,
        Self::DataFileMissing,
        Self::EpisodeLengthMismatch,
        Self::TimestampsNotMonotonic,
        Self::UnexpectedEntry,
        Self::EpisodeCountMismatch,
        Self::EpisodesIndexUnreadable,
        Self::VideoMissing,
        Self::TimestampSpacing,
        Self::ContentNotChecked,
    ];

    /// The stable `snake_case` name.
    ///
    /// ```
    /// assert_eq!(imify::validate::FindingCode::InfoMissing.as_str(), "info_missing");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InfoMissing => "info_missing",
            Self::InfoMalformed => "info_malformed",
            Self::CodebaseVersionUnsupported => "codebase_version_unsupported",
            Self::FpsInvalid => "fps_invalid",
            Self::ParquetUnreadable => "parquet_unreadable",
            Self::DataFileMissing => "data_file_missing",
            Self::EpisodeLengthMismatch => "episode_length_mismatch",
            Self::TimestampsNotMonotonic => "timestamps_not_monotonic",
            Self::UnexpectedEntry => "unexpected_entry",
            Self::EpisodeCountMismatch => "episode_count_mismatch",
            Self::EpisodesIndexUnreadable => "episodes_index_unreadable",
            Self::VideoMissing => "video_missing",
            Self::TimestampSpacing => "timestamp_spacing",
            Self::ContentNotChecked => "content_not_checked",
        }
    }

    /// The severity of every finding with this code.
    ///
    /// ```
    /// use imify::validate::{FindingCode, Severity};
    ///
    /// assert_eq!(FindingCode::InfoMissing.severity(), Severity::Error);
    /// ```
    #[must_use]
    pub const fn severity(self) -> Severity {
        match self {
            Self::InfoMissing
            | Self::InfoMalformed
            | Self::CodebaseVersionUnsupported
            | Self::FpsInvalid
            | Self::ParquetUnreadable
            | Self::DataFileMissing
            | Self::EpisodeLengthMismatch
            | Self::TimestampsNotMonotonic => Severity::Error,
            Self::UnexpectedEntry
            | Self::EpisodeCountMismatch
            | Self::EpisodesIndexUnreadable
            | Self::VideoMissing
            | Self::TimestampSpacing
            | Self::ContentNotChecked => Severity::Warning,
        }
    }
}

/// Reports every top-level entry that the upload does not include.
fn check_entries(root: &Path, findings: &mut Vec<Finding>) -> Result<(), Error> {
    let io_error = |source| Error::Io {
        path: root.to_path_buf(),
        source,
    };
    let mut names = Vec::new();
    for entry in fs::read_dir(root).map_err(io_error)? {
        names.push(entry.map_err(io_error)?.file_name());
    }
    names.sort();
    for name in names {
        let text = name.to_string_lossy();
        if text.starts_with('.') || UPLOADED_ENTRIES.contains(&text.as_ref()) {
            continue;
        }
        findings.push(Finding::new(
            FindingCode::UnexpectedEntry,
            name,
            "Not uploaded: only meta/, data/ and videos/ are uploaded.".to_owned(),
        ));
    }
    Ok(())
}

fn check_episode_count(info: &Info, episodes: &[Episode], findings: &mut Vec<Finding>) {
    let Some(declared) = info.total_episodes else {
        return;
    };
    let listed = episodes.len();
    if u64::try_from(listed).ok() != Some(declared) {
        findings.push(Finding::new(
            FindingCode::EpisodeCountMismatch,
            info::INFO_PATH,
            format!(
                "meta/info.json declares total_episodes {declared}; the episodes index lists {listed}."
            ),
        ));
    }
}

/// Reports every video file that `video_path` expands to and that does not exist.
fn check_videos(root: &Path, info: &Info, episodes: &[Episode], findings: &mut Vec<Finding>) {
    let Some(template) = &info.video_path else {
        return;
    };
    // Path to the first episode whose video it is; several v3.x episodes share a file.
    let mut videos: BTreeMap<PathBuf, (u64, &str)> = BTreeMap::new();
    for key in &info.video_keys {
        for episode in episodes {
            let path = match info.version {
                Version::V2_1 => template.expand(&[
                    ("episode_chunk", Arg::Int(episode.index / info.chunks_size)),
                    ("episode_index", Arg::Int(episode.index)),
                    ("video_key", Arg::Str(key)),
                ]),
                Version::V3 => {
                    let Some(file) = episode.videos.get(key) else {
                        continue;
                    };
                    template.expand(&[
                        ("chunk_index", Arg::Int(file.chunk)),
                        ("file_index", Arg::Int(file.file)),
                        ("video_key", Arg::Str(key)),
                    ])
                }
            };
            videos.entry(path).or_insert((episode.index, key));
        }
    }
    for (path, (episode, key)) in videos {
        if !root.join(&path).is_file() {
            findings.push(Finding::new(
                FindingCode::VideoMissing,
                path,
                format!("The {key} video of episode {episode} does not exist."),
            ));
        }
    }
}
