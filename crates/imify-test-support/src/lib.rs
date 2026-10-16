//! Synthetic `LeRobot` datasets for tests.
//!
//! ```
//! use imify_test_support::{DatasetBuilder, Defect, Format};
//!
//! let dataset = DatasetBuilder::new(Format::V3_0)
//!     .episodes(3)
//!     .defect(Defect::Fps(0))
//!     .build()?;
//!
//! assert!(dataset.root().join("meta/info.json").is_file());
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! A dataset holds `meta/` files, uncompressed parquet data and placeholder video files
//! that are not real MP4. The same configuration always produces byte-identical files.
#![deny(missing_docs)]

mod layout;
mod parquet;
mod v2_1;
mod v3_0;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::layout::{Layout, Shape};

const INFO_PATH: &str = "meta/info.json";
const MALFORMED_INFO: &[u8] = b"{\"codebase_version\": \n";
const STRAY_CONTENT: &[u8] = b"stray file\n";
const CORRUPT_LENGTH: usize = 16;

/// `LeRobot` dataset format version.
///
/// ```
/// use imify_test_support::{DatasetBuilder, Format};
///
/// let dataset = DatasetBuilder::new(Format::V2_1).build()?;
/// assert!(dataset.root().join("meta/episodes.jsonl").is_file());
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `codebase_version` `v2.1`: one data file and one video file per episode, metadata
    /// in JSON Lines.
    V2_1,
    /// `codebase_version` `v3.0`: all episodes in one data file and one video file per
    /// camera, metadata in parquet.
    V3_0,
}

/// A deviation from a well-formed dataset.
///
/// Each variant changes the dataset in exactly one respect.
///
/// ```
/// use imify_test_support::{DatasetBuilder, Defect, Format};
///
/// let dataset = DatasetBuilder::new(Format::V2_1)
///     .defect(Defect::MissingInfo)
///     .build()?;
/// assert!(!dataset.root().join("meta/info.json").exists());
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Defect {
    /// `meta/info.json` is not written. Takes precedence over [`Defect::MalformedInfo`].
    MissingInfo,
    /// `meta/info.json` is not valid JSON.
    MalformedInfo,
    /// `meta/info.json` declares this `codebase_version`.
    CodebaseVersion(String),
    /// `meta/info.json` declares this `fps`, for example `0` or `-1`.
    Fps(i64),
    /// `meta/info.json` declares this `total_episodes` instead of the written count.
    TotalEpisodes(u64),
    /// An extra file at this path relative to the dataset root, for example `notes.txt`.
    StrayFile(String),
    /// The first data parquet file is truncated to its first 16 bytes.
    CorruptDataFile,
    /// The first video file is not written.
    MissingVideo,
}

/// Configures and writes a synthetic dataset.
///
/// Defaults: 2 episodes of 10 frames each, 30 fps, one camera `front`, 1 KiB per video
/// file, no defects.
///
/// ```
/// use imify_test_support::{DatasetBuilder, Format};
///
/// let dataset = DatasetBuilder::new(Format::V3_0)
///     .episodes(4)
///     .frames_per_episode(5)
///     .fps(15)
///     .cameras(&["left", "wrist"])
///     .video_size(64)
///     .build()?;
/// assert_eq!(dataset.files().len(), 7);
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct DatasetBuilder {
    format: Format,
    shape: Shape,
    defects: Vec<Defect>,
}

impl DatasetBuilder {
    /// Creates a builder for `format` with the default configuration.
    ///
    /// ```
    /// use imify_test_support::{DatasetBuilder, Format};
    ///
    /// let builder = DatasetBuilder::new(Format::V2_1);
    /// ```
    #[must_use]
    pub fn new(format: Format) -> Self {
        Self {
            format,
            shape: Shape {
                episodes: 2,
                frames_per_episode: 10,
                fps: 30,
                cameras: vec!["front".to_owned()],
                video_size: 1024,
            },
            defects: Vec::new(),
        }
    }

    /// Sets the number of episodes.
    ///
    /// ```
    /// use imify_test_support::{DatasetBuilder, Format};
    ///
    /// let builder = DatasetBuilder::new(Format::V2_1).episodes(5);
    /// ```
    #[must_use]
    pub fn episodes(mut self, n: u32) -> Self {
        self.shape.episodes = n;
        self
    }

    /// Sets the number of frames in every episode.
    ///
    /// ```
    /// use imify_test_support::{DatasetBuilder, Format};
    ///
    /// let builder = DatasetBuilder::new(Format::V2_1).frames_per_episode(100);
    /// ```
    #[must_use]
    pub fn frames_per_episode(mut self, n: u32) -> Self {
        self.shape.frames_per_episode = n;
        self
    }

    /// Sets the frame rate declared in `meta/info.json` and used for frame timestamps.
    ///
    /// ```
    /// use imify_test_support::{DatasetBuilder, Format};
    ///
    /// let builder = DatasetBuilder::new(Format::V2_1).fps(15);
    /// ```
    #[must_use]
    pub fn fps(mut self, fps: u32) -> Self {
        self.shape.fps = fps;
        self
    }

    /// Sets the cameras. Each camera `name` adds the video feature
    /// `observation.images.<name>` and its video files.
    ///
    /// An empty list writes no video features and no `videos/` directory.
    ///
    /// ```
    /// use imify_test_support::{DatasetBuilder, Format};
    ///
    /// let dataset = DatasetBuilder::new(Format::V3_0).cameras(&[]).build()?;
    /// assert!(!dataset.root().join("videos").exists());
    /// # Ok::<(), std::io::Error>(())
    /// ```
    #[must_use]
    pub fn cameras(mut self, names: &[&str]) -> Self {
        self.shape.cameras = names.iter().map(|&name| name.to_owned()).collect();
        self
    }

    /// Sets the byte length of every placeholder video file.
    ///
    /// ```
    /// use imify_test_support::{DatasetBuilder, Format};
    ///
    /// let builder = DatasetBuilder::new(Format::V3_0).video_size(1 << 20);
    /// ```
    #[must_use]
    pub fn video_size(mut self, bytes: usize) -> Self {
        self.shape.video_size = bytes;
        self
    }

    /// Adds a defect. Defects combine; for repeated overrides of the same `info.json`
    /// field, the last one wins.
    ///
    /// ```
    /// use imify_test_support::{DatasetBuilder, Defect, Format};
    ///
    /// let builder = DatasetBuilder::new(Format::V2_1)
    ///     .defect(Defect::StrayFile("notes.txt".to_owned()))
    ///     .defect(Defect::MissingVideo);
    /// ```
    #[must_use]
    pub fn defect(mut self, defect: Defect) -> Self {
        self.defects.push(defect);
        self
    }

    /// Writes the dataset into a new temporary directory, removed when the returned
    /// [`SyntheticDataset`] is dropped.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] when a [`Defect::StrayFile`] path is empty,
    /// absolute or leaves the dataset root, and any error from creating or writing files.
    ///
    /// ```
    /// use imify_test_support::{DatasetBuilder, Format};
    ///
    /// let dataset = DatasetBuilder::new(Format::V2_1).build()?;
    /// assert!(dataset.root().join("meta/info.json").is_file());
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn build(self) -> io::Result<SyntheticDataset> {
        let dir = TempDir::with_prefix("imify-dataset-")?;
        let files = self.write_to(dir.path())?;
        Ok(SyntheticDataset { dir, files })
    }

    /// Writes the dataset into `root` and returns the written paths, relative to `root`
    /// and sorted. Existing files at those paths are overwritten.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] when a [`Defect::StrayFile`] path is empty,
    /// absolute or leaves the dataset root, and any error from creating or writing files.
    ///
    /// ```
    /// use imify_test_support::{DatasetBuilder, Format};
    ///
    /// let dir = tempfile::tempdir()?;
    /// let files = DatasetBuilder::new(Format::V3_0).write_to(dir.path())?;
    /// assert!(files.iter().any(|path| path.ends_with("info.json")));
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn write_to(self, root: &Path) -> io::Result<Vec<PathBuf>> {
        let files = self.files()?;
        for (path, bytes) in &files {
            let path = root.join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, bytes)?;
        }
        Ok(files.into_keys().collect())
    }

    /// Every file of the dataset with its content, defects applied.
    fn files(&self) -> io::Result<BTreeMap<PathBuf, Vec<u8>>> {
        let Layout {
            mut info,
            mut files,
        } = match self.format {
            Format::V2_1 => v2_1::layout(&self.shape)?,
            Format::V3_0 => v3_0::layout(&self.shape)?,
        };
        for defect in &self.defects {
            override_info(&mut info, defect);
        }
        if let Some(bytes) = self.info_file(&info)? {
            files.insert(PathBuf::from(INFO_PATH), bytes);
        }
        for defect in &self.defects {
            apply_to_files(&mut files, defect)?;
        }
        Ok(files)
    }

    fn info_file(&self, info: &Value) -> io::Result<Option<Vec<u8>>> {
        if self.defects.contains(&Defect::MissingInfo) {
            Ok(None)
        } else if self.defects.contains(&Defect::MalformedInfo) {
            Ok(Some(MALFORMED_INFO.to_vec()))
        } else {
            layout::json_file(info).map(Some)
        }
    }
}

fn override_info(info: &mut Value, defect: &Defect) {
    let (key, value) = match defect {
        Defect::CodebaseVersion(version) => ("codebase_version", json!(version)),
        Defect::Fps(fps) => ("fps", json!(fps)),
        Defect::TotalEpisodes(episodes) => ("total_episodes", json!(episodes)),
        _ => return,
    };
    info[key] = value;
}

fn apply_to_files(files: &mut BTreeMap<PathBuf, Vec<u8>>, defect: &Defect) -> io::Result<()> {
    match defect {
        Defect::StrayFile(path) => {
            files.insert(relative_path(path)?, STRAY_CONTENT.to_vec());
        }
        Defect::CorruptDataFile => {
            if let Some(bytes) = first_under(files, "data") {
                bytes.truncate(CORRUPT_LENGTH);
            }
        }
        Defect::MissingVideo => {
            let first = files
                .keys()
                .find(|path| path.starts_with("videos"))
                .cloned();
            if let Some(path) = first {
                files.remove(&path);
            }
        }
        _ => {}
    }
    Ok(())
}

fn first_under<'a>(
    files: &'a mut BTreeMap<PathBuf, Vec<u8>>,
    dir: &str,
) -> Option<&'a mut Vec<u8>> {
    files
        .iter_mut()
        .find(|(path, _)| path.starts_with(dir))
        .map(|(_, bytes)| bytes)
}

fn relative_path(path: &str) -> io::Result<PathBuf> {
    let path = PathBuf::from(path);
    let is_relative = path.components().next().is_some()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if is_relative {
        Ok(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "stray file path must be relative to the dataset root: {}",
                path.display()
            ),
        ))
    }
}

/// A dataset in a temporary directory that is removed on drop.
///
/// ```
/// use imify_test_support::{DatasetBuilder, Format};
///
/// let dataset = DatasetBuilder::new(Format::V2_1).build()?;
/// let root = dataset.root().to_path_buf();
/// drop(dataset);
/// assert!(!root.exists());
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct SyntheticDataset {
    dir: TempDir,
    files: Vec<PathBuf>,
}

impl SyntheticDataset {
    /// The dataset root directory.
    ///
    /// ```
    /// use imify_test_support::{DatasetBuilder, Format};
    ///
    /// let dataset = DatasetBuilder::new(Format::V2_1).build()?;
    /// assert!(dataset.root().is_dir());
    /// # Ok::<(), std::io::Error>(())
    /// ```
    #[must_use]
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Every written file, relative to [`root`](Self::root) and sorted.
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use imify_test_support::{DatasetBuilder, Format};
    ///
    /// let dataset = DatasetBuilder::new(Format::V3_0).build()?;
    /// assert!(dataset.files().contains(&Path::new("meta/info.json").to_path_buf()));
    /// # Ok::<(), std::io::Error>(())
    /// ```
    #[must_use]
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
}
