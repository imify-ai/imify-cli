//! The target directory of a download. Every operation below it is relative to a
//! directory file descriptor and follows no symbolic link.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};
use rustix::io::Errno;
use tokio::io::AsyncWriteExt;

use super::manifest::Manifest;
use crate::{DownloadError, Error};

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

const NOT_FILE_OR_DIRECTORY: &str = "an entry that is neither a regular file nor a directory";

/// An open target directory.
#[derive(Debug, Clone)]
pub(crate) struct Target {
    dir: Arc<OwnedFd>,
    root: PathBuf,
}

/// The paths and sizes a scan accepts.
struct Expected {
    files: HashMap<String, u64>,
    directories: HashSet<String>,
}

impl Expected {
    /// Returns `true` for a manifest file with the manifest size and `false` for the
    /// `.part` file of a manifest file. Describes any other regular file as the error.
    fn regular_file(&self, relative: &str, size: u64) -> Result<bool, String> {
        match self.files.get(relative) {
            Some(&listed) if listed == size => Ok(true),
            Some(listed) => Err(format!(
                "a file with {size} bytes, the manifest lists {listed}"
            )),
            None if relative
                .strip_suffix(".part")
                .is_some_and(|path| self.files.contains_key(path)) =>
            {
                Ok(false)
            }
            None => Err("a file not in the manifest".to_owned()),
        }
    }
}

impl Target {
    /// Creates the directory at `path` with its parents when it does not exist, and
    /// opens it. `path` itself may be a symbolic link.
    pub(crate) async fn open(path: &Path) -> Result<Self, Error> {
        let path = path.to_owned();
        blocking(path.clone(), move || {
            let failed = |source: io::Error| io_error(path.clone(), source);
            std::fs::create_dir_all(&path).map_err(failed)?;
            let root = std::fs::canonicalize(&path).map_err(failed)?;
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
            let dir = rustix::fs::open(&root, flags, Mode::empty())
                .map_err(|errno| failed(errno.into()))?;
            Ok(Self {
                dir: Arc::new(dir),
                root,
            })
        })
        .await
    }

    /// The canonical path of the directory.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// The path of the manifest file `relative` below the directory.
    pub(crate) fn path_of(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Returns the paths of the manifest files that exist with the manifest size.
    ///
    /// Fails with [`DownloadError::Foreign`] for any entry other than a directory on the
    /// way to a manifest file, a regular file at a manifest path with the manifest size,
    /// or a regular `<manifest path>.part` file.
    pub(crate) async fn scan(&self, manifest: &Manifest) -> Result<HashSet<String>, Error> {
        let mut expected = Expected {
            files: HashMap::new(),
            directories: HashSet::new(),
        };
        for entry in manifest.entries() {
            expected.files.insert(entry.path.clone(), entry.size);
            for (end, _) in entry.path.match_indices('/') {
                expected.directories.insert(entry.path[..end].to_owned());
            }
        }
        let target = self.clone();
        blocking(self.root.clone(), move || {
            let mut complete = HashSet::new();
            target.scan_dir(&target.dir, "", &expected, &mut complete)?;
            Ok(complete)
        })
        .await
    }

    fn scan_dir(
        &self,
        dir: &OwnedFd,
        prefix: &str,
        expected: &Expected,
        complete: &mut HashSet<String>,
    ) -> Result<(), Error> {
        let failed = |source: io::Error| io_error(self.root.join(prefix), source);
        let entries = Dir::read_from(dir).map_err(|errno| failed(errno.into()))?;
        for entry in entries {
            let entry = entry.map_err(|errno| failed(errno.into()))?;
            let name = OsStr::from_bytes(entry.file_name().to_bytes());
            if name == "." || name == ".." {
                continue;
            }
            let relative = Path::new(prefix).join(name).to_string_lossy().into_owned();
            let refuse = |what: String| foreign(self.root.join(&relative), what);
            let stat = rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|errno| failed(errno.into()))?;
            match FileType::from_raw_mode(stat.st_mode) {
                FileType::Directory if expected.directories.contains(&relative) => {
                    let sub = rustix::fs::openat(dir, name, DIRECTORY_FLAGS, Mode::empty())
                        .map_err(|errno| open_error(self.root.join(&relative), errno))?;
                    self.scan_dir(&sub, &relative, expected, complete)?;
                }
                FileType::RegularFile => {
                    let size = u64::try_from(stat.st_size).unwrap_or_default();
                    if expected.regular_file(&relative, size).map_err(refuse)? {
                        complete.insert(relative);
                    }
                }
                FileType::Directory => {
                    return Err(refuse("a directory not in the manifest".to_owned()));
                }
                FileType::Symlink => return Err(refuse("a symbolic link".to_owned())),
                _ => return Err(refuse(NOT_FILE_OR_DIRECTORY.to_owned())),
            }
        }
        Ok(())
    }

    /// Creates the parent directories of the manifest file `relative`, removes an existing
    /// `<relative>.part` and creates it as a new, empty file.
    pub(crate) async fn create_part(&self, relative: &str) -> Result<PartFile, Error> {
        let target = self.clone();
        let relative = relative.to_owned();
        blocking(self.path_of(&relative), move || {
            target.create_part_blocking(&relative)
        })
        .await
    }

    fn create_part_blocking(&self, relative: &str) -> Result<PartFile, Error> {
        let (directories, name) = relative.rsplit_once('/').unwrap_or(("", relative));
        let mut dir = rustix::fs::openat(&*self.dir, ".", DIRECTORY_FLAGS, Mode::empty())
            .map_err(|errno| io_error(self.root.clone(), errno.into()))?;
        let mut walked = self.root.clone();
        for component in directories
            .split('/')
            .filter(|component| !component.is_empty())
        {
            walked.push(component);
            match rustix::fs::mkdirat(&dir, component, Mode::RWXU | Mode::RWXG | Mode::RWXO) {
                Ok(()) | Err(Errno::EXIST) => {}
                Err(errno) => return Err(io_error(walked, errno.into())),
            }
            dir = rustix::fs::openat(&dir, component, DIRECTORY_FLAGS, Mode::empty())
                .map_err(|errno| open_error(walked.clone(), errno))?;
        }
        let part_name = format!("{name}.part");
        let part_path = self.root.join(format!("{relative}.part"));
        // A new file, never an existing one: an existing `.part` may be a hard link to a
        // file outside the target.
        match rustix::fs::unlinkat(&dir, part_name.as_str(), AtFlags::empty()) {
            Ok(()) | Err(Errno::NOENT) => {}
            Err(errno) => return Err(io_error(part_path, errno.into())),
        }
        let flags =
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mode = Mode::RUSR | Mode::WUSR | Mode::RGRP | Mode::ROTH;
        let file = File::from(
            rustix::fs::openat(&dir, part_name.as_str(), flags, mode)
                .map_err(|errno| open_error(part_path.clone(), errno))?,
        );
        Ok(PartFile {
            dir,
            name: name.to_owned(),
            part_name,
            path: self.path_of(relative),
            part_path,
            file: tokio::fs::File::from_std(file),
        })
    }
}

/// The open `.part` file of one manifest file.
#[derive(Debug)]
pub(crate) struct PartFile {
    dir: OwnedFd,
    name: String,
    part_name: String,
    path: PathBuf,
    part_path: PathBuf,
    file: tokio::fs::File,
}

impl PartFile {
    /// Appends `bytes`.
    pub(crate) async fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.file
            .write_all(bytes)
            .await
            .map_err(|source| io_error(self.part_path.clone(), source))
    }

    /// Syncs the file and then renames it to the manifest path.
    pub(crate) async fn finish(mut self) -> Result<(), Error> {
        let failed = |source: io::Error| io_error(self.part_path.clone(), source);
        self.file.flush().await.map_err(failed)?;
        self.file.sync_all().await.map_err(failed)?;
        let path = self.path.clone();
        blocking(path.clone(), move || {
            rustix::fs::renameat(
                &self.dir,
                self.part_name.as_str(),
                &self.dir,
                self.name.as_str(),
            )
            .map_err(|errno| io_error(path, errno.into()))
        })
        .await
    }
}

/// Runs `operation` on the blocking thread pool. `path` names the file in the error when
/// the task fails to complete.
async fn blocking<T: Send + 'static>(
    path: PathBuf,
    operation: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(operation)
        .await
        .unwrap_or_else(|join_error| Err(io_error(path, io::Error::from(join_error))))
}

/// The error for a failed open below the target. `O_NOFOLLOW` reports a symbolic link as
/// `ELOOP`, `O_DIRECTORY` reports a link or a file as `ENOTDIR`, and `O_EXCL` reports an
/// entry created after the `.part` file was removed as `EEXIST`.
fn open_error(path: PathBuf, errno: Errno) -> Error {
    match errno {
        Errno::LOOP | Errno::NOTDIR => foreign(
            path,
            "a symbolic link or a file in place of a directory or file".to_owned(),
        ),
        Errno::EXIST => foreign(path, "an entry created during the download".to_owned()),
        other => io_error(path, other.into()),
    }
}

fn foreign(path: PathBuf, what: String) -> Error {
    Error::Download {
        path,
        reason: DownloadError::Foreign(what),
    }
}

fn io_error(path: PathBuf, source: io::Error) -> Error {
    Error::Download {
        path,
        reason: DownloadError::Io(source),
    }
}

#[cfg(test)]
mod tests;
