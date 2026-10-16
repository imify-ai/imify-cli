//! The list of files an upload sends.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::Error;

/// Path of the file that is uploaded before all others.
pub(crate) const INFO_FILE: &str = "meta/info.json";

const UPLOADED_DIRECTORIES: [&str; 3] = ["meta", "data", "videos"];

/// The files of a dataset directory that an upload sends.
///
/// The plan contains the regular files below the top-level directories `meta/`, `data/`
/// and `videos/`, with `meta/info.json` first and the rest sorted by path. Symbolic links
/// to files are followed. Everything else is listed in [`UploadPlan::skipped`]: other
/// files and directories, paths with a segment that starts with `.`, symbolic links to
/// directories and names that are not valid UTF-8. Skipped directories are not entered
/// and appear once, with a trailing `/`.
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
    skipped: Vec<String>,
}

impl UploadPlan {
    /// Lists the files to upload from the dataset directory `root`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when `root/meta/info.json` is not a readable file or a
    /// directory below `root` cannot be read.
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
        let metadata = fs::metadata(&info).map_err(|source| Error::Io {
            path: info.clone(),
            source,
        })?;
        if !metadata.is_file() {
            return Err(Error::Io {
                path: info,
                source: io::Error::other("not a regular file"),
            });
        }
        let mut walk = Walk::default();
        walk.directory(&root, "")?;
        walk.files.sort_by(|a, b| {
            (a.path != INFO_FILE)
                .cmp(&(b.path != INFO_FILE))
                .then_with(|| a.path.cmp(&b.path))
        });
        walk.skipped.sort();
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

    /// Returns the relative paths below the root that are not uploaded, sorted.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let plan = imify::ingest::UploadPlan::from_dir("datasets/pick-place")?;
    /// for path in plan.skipped() {
    ///     println!("skipped {path}");
    /// }
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn skipped(&self) -> &[String] {
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
}

impl PlannedFile {
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
}

#[derive(Default)]
struct Walk {
    files: Vec<PlannedFile>,
    skipped: Vec<String>,
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
                self.skipped
                    .push(format!("{prefix}{}", name.to_string_lossy()));
                continue;
            };
            let path = format!("{prefix}{name}");
            let file_type = entry.file_type().map_err(read_error)?;
            let entry_path = entry.path();
            let is_uploaded = !name.starts_with('.')
                && (!prefix.is_empty() || UPLOADED_DIRECTORIES.contains(&name));
            if file_type.is_dir() {
                if is_uploaded {
                    self.directory(&entry_path, &format!("{path}/"))?;
                } else {
                    self.skipped.push(format!("{path}/"));
                }
                continue;
            }
            let metadata = fs::metadata(&entry_path).map_err(|source| Error::Io {
                path: entry_path.clone(),
                source,
            })?;
            if is_uploaded && metadata.is_file() && !prefix.is_empty() {
                self.files.push(PlannedFile {
                    path,
                    size: metadata.len(),
                    modified: metadata.modified().ok(),
                });
            } else if metadata.is_dir() {
                self.skipped.push(format!("{path}/"));
            } else {
                self.skipped.push(path);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
