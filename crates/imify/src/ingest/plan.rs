//! The list of files an upload sends.

use std::fs::{self, File, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::Error;
use crate::local_file;

/// Path of the file that is uploaded before all others.
pub(crate) const INFO_FILE: &str = "meta/info.json";

const UPLOADED_DIRECTORIES: [&str; 3] = ["meta", "data", "videos"];

/// The files of a dataset directory that an upload sends.
///
/// The plan contains the regular files below the top-level directories `meta/`, `data/`
/// and `videos/`, with `meta/info.json` first and the rest sorted by path. Symbolic links
/// are not followed. Everything else is listed in [`UploadPlan::skipped`] with a
/// [`SkipReason`]: other files and directories, paths with a segment that starts with
/// `.`, symbolic links to files or directories, entries that are neither a regular file
/// nor a directory, and names that are not valid UTF-8. Skipped directories are not
/// entered and appear once, with a trailing `/`.
///
/// # Examples
///
/// ```no_run
/// use imify::ingest::UploadPlan;
///
/// let plan = UploadPlan::from_dir("datasets/pick-place")?;
/// println!("{} files, {} bytes", plan.files().len(), plan.total_bytes());
/// # Ok::<(), imify::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct UploadPlan {
    root: PathBuf,
    files: Vec<PlannedFile>,
    skipped: Vec<SkippedEntry>,
}

impl UploadPlan {
    /// Lists the files to upload from the dataset directory `root`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when `root/meta/info.json` is missing, a symbolic link or not
    /// a regular file, or a directory below `root` cannot be read.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let plan = imify::ingest::UploadPlan::from_dir("datasets/pick-place")?;
    /// assert_eq!(plan.files()[0].path(), "meta/info.json");
    /// # Ok::<(), imify::Error>(())
    /// ```
    pub fn from_dir(root: impl Into<PathBuf>) -> Result<Self, Error> {
        let root = root.into();
        let info = root.join(INFO_FILE);
        let metadata = fs::symlink_metadata(&info).map_err(|source| Error::Io {
            path: info.clone(),
            source,
        })?;
        if !metadata.is_file() {
            let reason = if metadata.is_symlink() {
                "the path is a symbolic link"
            } else {
                "the path is not a regular file"
            };
            return Err(Error::Io {
                path: info,
                source: io::Error::new(io::ErrorKind::InvalidInput, reason),
            });
        }
        let mut walk = Walk::default();
        walk.directory(&root, "")?;
        walk.files.sort_by(|a, b| {
            (a.path != INFO_FILE)
                .cmp(&(b.path != INFO_FILE))
                .then_with(|| a.path.cmp(&b.path))
        });
        walk.skipped.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Self {
            root,
            files: walk.files,
            skipped: walk.skipped,
        })
    }

    /// Returns the dataset directory.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let plan = imify::ingest::UploadPlan::from_dir("datasets/pick-place")?;
    /// assert_eq!(plan.root(), std::path::Path::new("datasets/pick-place"));
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the files to upload, `meta/info.json` first, then sorted by path.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let plan = imify::ingest::UploadPlan::from_dir("datasets/pick-place")?;
    /// for file in plan.files() {
    ///     println!("{} {}", file.path(), file.size());
    /// }
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn files(&self) -> &[PlannedFile] {
        &self.files
    }

    /// Returns the entries below the root that are not uploaded, sorted by path.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let plan = imify::ingest::UploadPlan::from_dir("datasets/pick-place")?;
    /// for entry in plan.skipped() {
    ///     println!("skipped {} ({})", entry.path(), entry.reason().as_str());
    /// }
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn skipped(&self) -> &[SkippedEntry] {
        &self.skipped
    }

    /// Returns the sum of the sizes of [`UploadPlan::files`] in bytes.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let plan = imify::ingest::UploadPlan::from_dir("datasets/pick-place")?;
    /// println!("{} bytes to send", plan.total_bytes());
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(PlannedFile::size).sum()
    }

    /// Keeps only the files for which `keep` returns `true`, such as files not uploaded
    /// yet.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::collections::HashSet;
    ///
    /// let finished: HashSet<String> = HashSet::from(["meta/info.json".to_owned()]);
    /// let mut plan = imify::ingest::UploadPlan::from_dir("datasets/pick-place")?;
    /// plan.retain(|file| !finished.contains(file.path()));
    /// # Ok::<(), imify::Error>(())
    /// ```
    pub fn retain(&mut self, mut keep: impl FnMut(&PlannedFile) -> bool) {
        self.files.retain(|file| keep(file));
    }
}

/// A file in an [`UploadPlan`].
///
/// # Examples
///
/// ```no_run
/// let plan = imify::ingest::UploadPlan::from_dir("datasets/pick-place")?;
/// let info = &plan.files()[0];
/// assert_eq!((info.path(), info.size() > 0), ("meta/info.json", true));
/// # Ok::<(), imify::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    path: String,
    size: u64,
    modified: Option<SystemTime>,
    device: u64,
    inode: u64,
}

impl PlannedFile {
    fn from_metadata(path: String, metadata: &Metadata) -> Self {
        Self {
            path,
            size: metadata.len(),
            modified: metadata.modified().ok(),
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    /// Returns the path relative to the dataset directory, separated by `/`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let plan = imify::ingest::UploadPlan::from_dir("datasets/pick-place")?;
    /// assert_eq!(plan.files()[0].path(), "meta/info.json");
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the size in bytes at planning time.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let plan = imify::ingest::UploadPlan::from_dir("datasets/pick-place")?;
    /// println!("{} bytes", plan.files()[0].size());
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Returns the modification time at planning time, `None` when the platform does
    /// not report one.
    pub(crate) fn modified(&self) -> Option<SystemTime> {
        self.modified
    }

    /// Opens the file at `local_path` for reading without following a symbolic link.
    ///
    /// Fails with [`Error::Io`] when the path is not a regular file, or its device, inode
    /// or size differs from planning time.
    pub(crate) fn open(&self, local_path: &Path) -> Result<File, Error> {
        let io_error = |source| Error::Io {
            path: local_path.to_owned(),
            source,
        };
        let (file, metadata) = local_file::open_regular(local_path).map_err(io_error)?;
        if (metadata.dev(), metadata.ino()) != (self.device, self.inode) {
            return Err(io_error(io::Error::other(
                "the file was replaced since planning",
            )));
        }
        if metadata.len() != self.size {
            return Err(io_error(self.changed_size()));
        }
        Ok(file)
    }

    /// The error for a file whose size differs from planning time.
    pub(crate) fn changed_size(&self) -> io::Error {
        io::Error::other(format!(
            "file size changed since planning, expected {} bytes",
            self.size
        ))
    }
}

/// An entry of a dataset directory that an [`UploadPlan`] does not upload.
///
/// # Examples
///
/// ```no_run
/// use imify::ingest::{SkipReason, UploadPlan};
///
/// let plan = UploadPlan::from_dir("datasets/pick-place")?;
/// for entry in plan.skipped() {
///     if entry.reason() == SkipReason::SymbolicLink {
///         println!("not uploaded: {} is a symbolic link", entry.path());
///     }
/// }
/// # Ok::<(), imify::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedEntry {
    path: String,
    reason: SkipReason,
}

impl SkippedEntry {
    /// Returns the path relative to the dataset directory, separated by `/`. A skipped
    /// directory ends with `/`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let plan = imify::ingest::UploadPlan::from_dir("datasets/pick-place")?;
    /// for entry in plan.skipped() {
    ///     println!("{}", entry.path());
    /// }
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns why the entry is not uploaded.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let plan = imify::ingest::UploadPlan::from_dir("datasets/pick-place")?;
    /// for entry in plan.skipped() {
    ///     println!("{}: {}", entry.path(), entry.reason().as_str());
    /// }
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn reason(&self) -> SkipReason {
        self.reason
    }
}

/// Why an [`UploadPlan`] does not upload an entry.
///
/// # Examples
///
/// ```
/// use imify::ingest::SkipReason;
///
/// assert_eq!(SkipReason::SymbolicLink.as_str(), "symbolic_link");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SkipReason {
    /// The entry is not below `meta/`, `data/` or `videos/`.
    OutsideDataset,
    /// A segment of the path starts with `.`.
    Hidden,
    /// The entry is a symbolic link, to a file or to a directory.
    SymbolicLink,
    /// The entry is neither a regular file nor a directory, such as a FIFO or a socket.
    NotRegularFile,
    /// The name is not valid UTF-8.
    InvalidName,
}

impl SkipReason {
    /// The stable `snake_case` name.
    ///
    /// ```
    /// assert_eq!(imify::ingest::SkipReason::Hidden.as_str(), "hidden");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OutsideDataset => "outside_dataset",
            Self::Hidden => "hidden",
            Self::SymbolicLink => "symbolic_link",
            Self::NotRegularFile => "not_regular_file",
            Self::InvalidName => "invalid_name",
        }
    }
}

#[derive(Default)]
struct Walk {
    files: Vec<PlannedFile>,
    skipped: Vec<SkippedEntry>,
}

impl Walk {
    /// Adds the entries of `directory`, whose path relative to the root is `prefix`
    /// (empty for the root, otherwise ending in `/`).
    fn directory(&mut self, directory: &Path, prefix: &str) -> Result<(), Error> {
        let read_error = |source| Error::Io {
            path: directory.to_owned(),
            source,
        };
        for entry in fs::read_dir(directory).map_err(read_error)? {
            let entry = entry.map_err(read_error)?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                self.skip(
                    format!("{prefix}{}", name.to_string_lossy()),
                    SkipReason::InvalidName,
                );
                continue;
            };
            // Does not follow a symbolic link.
            let metadata = entry.metadata().map_err(read_error)?;
            let path = format!("{prefix}{name}");
            let shown = if metadata.is_dir() {
                format!("{path}/")
            } else {
                path.clone()
            };
            let top_level_kept = !prefix.is_empty()
                || (UPLOADED_DIRECTORIES.contains(&name)
                    && (metadata.is_dir() || metadata.is_symlink()));
            if name.starts_with('.') {
                self.skip(shown, SkipReason::Hidden);
            } else if !top_level_kept {
                self.skip(shown, SkipReason::OutsideDataset);
            } else if metadata.is_symlink() {
                self.skip(shown, SkipReason::SymbolicLink);
            } else if metadata.is_dir() {
                self.directory(&entry.path(), &shown)?;
            } else if metadata.is_file() {
                self.files.push(PlannedFile::from_metadata(path, &metadata));
            } else {
                self.skip(shown, SkipReason::NotRegularFile);
            }
        }
        Ok(())
    }

    fn skip(&mut self, path: String, reason: SkipReason) {
        self.skipped.push(SkippedEntry { path, reason });
    }
}

#[cfg(test)]
mod tests;
