//! The login in `credentials.json`, readable only by the user.

use std::collections::BTreeMap;
use std::fs::{DirBuilder, File};
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use imify::Error;
use imify::auth::Credentials;
use rustix::fs::{FileType, FlockOperation, Mode, OFlags};
use rustix::io::Errno;

const FILE_NAME: &str = "credentials.json";
const LOCK_FILE_NAME: &str = "credentials.json.lock";

/// The largest credentials file read, in bytes.
const MAX_FILE_SIZE: u64 = 1024 * 1024;

/// Permission bits that let users other than the owner access the file.
const GROUP_AND_OTHER: u32 = 0o077;

/// Permission bits that let users other than the owner change the directory.
const GROUP_AND_OTHER_WRITE: u32 = 0o022;

/// The file's content: one entry per host key. Entries of other hosts are kept unparsed.
type Entries = BTreeMap<String, serde_json::Value>;

/// The login of one host in `<dir>/credentials.json`.
///
/// The file has mode `0600`; a new directory gets mode `0700`. A file that is not a regular
/// file owned by the user, or that other users can access, is rejected, as is a directory
/// owned by another user or writable by other users. Symbolic links are not followed. An
/// advisory lock on `credentials.json.lock` serializes readers and writers; writes replace
/// the file atomically and sync the directory.
#[derive(Debug, Clone)]
pub struct FileStore {
    path: PathBuf,
    host_key: String,
}

/// How strictly [`FileStore::read_bytes`] checks the file's permissions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Permissions {
    /// Other users must have no access.
    OwnerOnly,
    /// Any mode; used by `delete`, which rewrites or removes the file.
    Any,
}

impl FileStore {
    pub fn new(dir: &Path, host_key: &str) -> Self {
        Self {
            path: dir.join(FILE_NAME),
            host_key: host_key.to_owned(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("."))
    }

    fn error(&self, message: impl Into<String>) -> Error {
        error_at(&self.path, message)
    }

    /// Checks the directory. Returns `false` when it does not exist.
    fn check_dir(&self) -> Result<bool, Error> {
        let dir = self.dir();
        let metadata = match std::fs::metadata(dir) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error_at(dir, error.to_string())),
        };
        if !metadata.is_dir() {
            return Err(error_at(dir, "not a directory"));
        }
        if metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(error_at(
                dir,
                "the directory belongs to another user; set IMIFY_CONFIG_DIR to a directory you own",
            ));
        }
        if metadata.mode() & GROUP_AND_OTHER_WRITE != 0 {
            return Err(error_at(
                dir,
                format!(
                    "other users can write to the directory; run `chmod 700 {}`",
                    dir.display()
                ),
            ));
        }
        Ok(true)
    }

    fn create_dir(&self) -> Result<(), Error> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(self.dir())
            .map_err(|error| self.error(format!("cannot create the directory: {error}")))?;
        self.check_dir().map(drop)
    }

    /// Takes the advisory lock; it is released when the returned descriptor is dropped.
    fn lock(&self, operation: FlockOperation) -> Result<OwnedFd, Error> {
        let path = self.dir().join(LOCK_FILE_NAME);
        let lock = rustix::fs::open(
            &path,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|error| error_at(&path, format!("cannot open the lock file: {error}")))?;
        rustix::fs::flock(&lock, operation)
            .map_err(|error| error_at(&path, format!("cannot lock: {error}")))?;
        Ok(lock)
    }

    /// The file's bytes, `None` when it does not exist.
    fn read_bytes(&self, permissions: Permissions) -> Result<Option<Vec<u8>>, Error> {
        let fd = match rustix::fs::open(
            &self.path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => return Ok(None),
            Err(Errno::LOOP) => return Err(self.error("the file is a symbolic link")),
            Err(error) => return Err(self.error(error.to_string())),
        };
        let stat = rustix::fs::fstat(&fd).map_err(|error| self.error(error.to_string()))?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(self.error("not a regular file"));
        }
        if stat.st_uid != rustix::process::geteuid().as_raw() {
            return Err(self.error("the file belongs to another user"));
        }
        let mode = u32::from(stat.st_mode) & 0o777;
        if permissions == Permissions::OwnerOnly && mode & GROUP_AND_OTHER != 0 {
            return Err(self.error(format!(
                "other users can access the file (mode {mode:o}); run `chmod 600 {}`",
                self.path.display()
            )));
        }
        let mut content = Vec::new();
        File::from(fd)
            .take(MAX_FILE_SIZE + 1)
            .read_to_end(&mut content)
            .map_err(|error| self.error(error.to_string()))?;
        if content.len() as u64 > MAX_FILE_SIZE {
            return Err(self.error("the file is larger than 1 MiB"));
        }
        Ok(Some(content))
    }

    fn parse(&self, content: &[u8]) -> Result<Entries, Error> {
        serde_json::from_slice(content)
            .map_err(|error| self.error(format!("the file is not valid JSON: {error}")))
    }

    fn read(&self) -> Result<Entries, Error> {
        match self.read_bytes(Permissions::OwnerOnly)? {
            Some(content) => self.parse(&content),
            None => Ok(Entries::new()),
        }
    }

    fn write(&self, entries: &Entries) -> Result<(), Error> {
        let dir = self.dir();
        let content =
            serde_json::to_vec_pretty(entries).map_err(|error| self.error(error.to_string()))?;
        // `NamedTempFile` creates the file with mode 0600.
        let mut temporary = tempfile::NamedTempFile::new_in(dir)
            .map_err(|error| self.error(format!("cannot create a temporary file: {error}")))?;
        temporary
            .write_all(&content)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|error| self.error(error.to_string()))?;
        temporary
            .persist(&self.path)
            .map_err(|error| self.error(error.error.to_string()))?;
        self.sync_dir()
    }

    fn remove(&self) -> Result<(), Error> {
        std::fs::remove_file(&self.path).map_err(|error| self.error(error.to_string()))?;
        self.sync_dir()
    }

    /// Makes the rename or removal in the directory durable.
    fn sync_dir(&self) -> Result<(), Error> {
        let dir = self.dir();
        rustix::fs::open(
            dir,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .and_then(|fd| rustix::fs::fsync(&fd))
        .map_err(|error| error_at(dir, format!("cannot sync the directory: {error}")))
    }
}

fn error_at(path: &Path, message: impl Into<String>) -> Error {
    Error::CredentialStore {
        source: Box::new(FileStoreError {
            path: path.to_path_buf(),
            message: message.into(),
        }),
    }
}

impl imify::auth::CredentialStore for FileStore {
    fn load(&self) -> Result<Option<Credentials>, Error> {
        if !self.check_dir()? {
            return Ok(None);
        }
        let _lock = self.lock(FlockOperation::LockShared)?;
        let Some(entry) = self.read()?.remove(&self.host_key) else {
            return Ok(None);
        };
        serde_json::from_value(entry).map(Some).map_err(|error| {
            self.error(format!(
                "the entry for {} is not an imify login: {error}",
                self.host_key
            ))
        })
    }

    fn save(&self, credentials: &Credentials) -> Result<(), Error> {
        self.create_dir()?;
        let _lock = self.lock(FlockOperation::LockExclusive)?;
        let mut entries = self.read()?;
        let entry =
            serde_json::to_value(credentials).map_err(|error| self.error(error.to_string()))?;
        entries.insert(self.host_key.clone(), entry);
        self.write(&entries)
    }

    /// Removes the host's entry, also from a file with a wider mode or an entry that is not
    /// a login. A file that is not valid JSON is removed. An empty file is removed.
    fn delete(&self) -> Result<bool, Error> {
        if !self.check_dir()? {
            return Ok(false);
        }
        let _lock = self.lock(FlockOperation::LockExclusive)?;
        let Some(content) = self.read_bytes(Permissions::Any)? else {
            return Ok(false);
        };
        let Ok(mut entries) = self.parse(&content) else {
            self.remove()?;
            return Ok(true);
        };
        if entries.remove(&self.host_key).is_none() {
            return Ok(false);
        }
        if entries.is_empty() {
            self.remove()?;
        } else {
            self.write(&entries)?;
        }
        Ok(true)
    }
}

/// The credentials file or its directory cannot be used.
#[derive(Debug, thiserror::Error)]
#[error("{}: {message}", path.display())]
pub struct FileStoreError {
    path: PathBuf,
    message: String,
}
