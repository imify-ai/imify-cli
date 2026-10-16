//! Files in the configuration directory that only the user can read and change.
//!
//! A new directory gets mode `0700` and a written file mode `0600`. A directory owned by
//! another user or writable by other users is rejected, as is a file that is not a regular
//! file owned by the user. Symbolic links are not followed. Writes replace the file atomically
//! and sync the directory.

use std::fs::{DirBuilder, File};
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use rustix::fs::{FileType, FlockOperation, Mode, OFlags};
use rustix::io::Errno;

/// The largest file read, in bytes.
const MAX_FILE_SIZE: u64 = 1024 * 1024;

/// Permission bits that let users other than the owner change the directory.
const GROUP_AND_OTHER_WRITE: u32 = 0o022;

/// A file or directory that cannot be used, and why.
#[derive(Debug, thiserror::Error)]
#[error("{}: {message}", path.display())]
pub struct PrivateFileError {
    path: PathBuf,
    message: String,
}

impl PrivateFileError {
    pub fn new(path: &Path, message: impl Into<String>) -> Self {
        Self {
            path: path.to_path_buf(),
            message: message.into(),
        }
    }
}

/// Which permissions [`read`] accepts on the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permissions {
    /// Other users must have no access.
    OwnerOnly,
    /// Any mode.
    Any,
}

/// The directory of `path`.
fn parent(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new("."))
}

/// Checks that `dir` is a directory of the user that other users cannot write to. Returns
/// `false` when it does not exist.
///
/// # Errors
///
/// Returns an error when `dir` is not a directory, belongs to another user, is writable by
/// other users, or cannot be inspected.
pub fn check_dir(dir: &Path) -> Result<bool, PrivateFileError> {
    let metadata = match std::fs::metadata(dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(PrivateFileError::new(dir, error.to_string())),
    };
    if !metadata.is_dir() {
        return Err(PrivateFileError::new(dir, "not a directory"));
    }
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(PrivateFileError::new(
            dir,
            "the directory belongs to another user; set IMIFY_CONFIG_DIR to a directory you own",
        ));
    }
    if metadata.mode() & GROUP_AND_OTHER_WRITE != 0 {
        return Err(PrivateFileError::new(
            dir,
            format!(
                "other users can write to the directory; run `chmod 700 {}`",
                dir.display()
            ),
        ));
    }
    Ok(true)
}

/// Creates `dir` and its parents with mode `0700` where they do not exist, then checks it as
/// [`check_dir`] does.
///
/// # Errors
///
/// Returns an error when the directory cannot be created or fails the check.
pub fn create_dir(dir: &Path) -> Result<(), PrivateFileError> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|error| {
            PrivateFileError::new(dir, format!("cannot create the directory: {error}"))
        })?;
    check_dir(dir).map(drop)
}

/// Takes an advisory lock on the file at `path`, creating it with mode `0600`. The lock is
/// released when the returned descriptor is dropped.
///
/// # Errors
///
/// Returns an error when the lock file cannot be opened or locked.
pub fn lock(path: &Path, operation: FlockOperation) -> Result<OwnedFd, PrivateFileError> {
    let lock = rustix::fs::open(
        path,
        OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|error| PrivateFileError::new(path, format!("cannot open the lock file: {error}")))?;
    rustix::fs::flock(&lock, operation)
        .map_err(|error| PrivateFileError::new(path, format!("cannot lock: {error}")))?;
    Ok(lock)
}

/// The bytes of the file at `path`, `None` when it does not exist.
///
/// # Errors
///
/// Returns an error when the file is a symbolic link, not a regular file, belongs to another
/// user, is larger than 1 MiB, cannot be read, or, with [`Permissions::OwnerOnly`], other
/// users can access it.
pub fn read(path: &Path, permissions: Permissions) -> Result<Option<Vec<u8>>, PrivateFileError> {
    let error = |message: String| PrivateFileError::new(path, message);
    let fd = match rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(Errno::NOENT) => return Ok(None),
        Err(Errno::LOOP) => return Err(error("the file is a symbolic link".to_owned())),
        Err(other) => return Err(error(other.to_string())),
    };
    let stat = rustix::fs::fstat(&fd).map_err(|other| error(other.to_string()))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(error("not a regular file".to_owned()));
    }
    if stat.st_uid != rustix::process::geteuid().as_raw() {
        return Err(error("the file belongs to another user".to_owned()));
    }
    // `st_mode` is 16 bits wide on macOS and 32 bits on Linux; `Mode` hides the width.
    let mode = Mode::from_raw_mode(stat.st_mode) & (Mode::RWXU | Mode::RWXG | Mode::RWXO);
    if permissions == Permissions::OwnerOnly && mode.intersects(Mode::RWXG | Mode::RWXO) {
        return Err(error(format!(
            "other users can access the file (mode {:o}); run `chmod 600 {}`",
            mode.bits(),
            path.display()
        )));
    }
    let mut content = Vec::new();
    File::from(fd)
        .take(MAX_FILE_SIZE + 1)
        .read_to_end(&mut content)
        .map_err(|other| error(other.to_string()))?;
    if content.len() as u64 > MAX_FILE_SIZE {
        return Err(error("the file is larger than 1 MiB".to_owned()));
    }
    Ok(Some(content))
}

/// Replaces the file at `path` with `content`, with mode `0600`. The directory must exist.
///
/// # Errors
///
/// Returns an error when the temporary file cannot be written or renamed, or the directory
/// cannot be synced.
pub fn write(path: &Path, content: &[u8]) -> Result<(), PrivateFileError> {
    let error = |message: String| PrivateFileError::new(path, message);
    // `NamedTempFile` creates the file with mode 0600.
    let mut temporary = tempfile::NamedTempFile::new_in(parent(path))
        .map_err(|other| error(format!("cannot create a temporary file: {other}")))?;
    temporary
        .write_all(content)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|other| error(other.to_string()))?;
    temporary
        .persist(path)
        .map_err(|other| error(other.error.to_string()))?;
    sync_dir(parent(path))
}

/// Removes the file at `path`.
///
/// # Errors
///
/// Returns an error when the file cannot be removed or the directory cannot be synced.
pub fn remove(path: &Path) -> Result<(), PrivateFileError> {
    std::fs::remove_file(path).map_err(|error| PrivateFileError::new(path, error.to_string()))?;
    sync_dir(parent(path))
}

/// Makes a rename or removal in `dir` durable.
fn sync_dir(dir: &Path) -> Result<(), PrivateFileError> {
    rustix::fs::open(
        dir,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .and_then(|fd| rustix::fs::fsync(&fd))
    .map_err(|error| PrivateFileError::new(dir, format!("cannot sync the directory: {error}")))
}
