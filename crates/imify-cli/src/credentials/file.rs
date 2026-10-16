//! The login in `credentials.json`, readable only by the user.

use std::collections::BTreeMap;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use imify::Error;
use imify::auth::Credentials;
use rustix::fs::FlockOperation;

use crate::private_file::{self, Permissions, PrivateFileError};

const FILE_NAME: &str = "credentials.json";
const LOCK_FILE_NAME: &str = "credentials.json.lock";

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
        store_error(PrivateFileError::new(&self.path, message))
    }

    /// Checks the directory. Returns `false` when it does not exist.
    fn check_dir(&self) -> Result<bool, Error> {
        private_file::check_dir(self.dir()).map_err(store_error)
    }

    fn create_dir(&self) -> Result<(), Error> {
        private_file::create_dir(self.dir()).map_err(store_error)
    }

    /// Takes the advisory lock; it is released when the returned descriptor is dropped.
    fn lock(&self, operation: FlockOperation) -> Result<OwnedFd, Error> {
        private_file::lock(&self.dir().join(LOCK_FILE_NAME), operation).map_err(store_error)
    }

    /// The file's bytes, `None` when it does not exist.
    fn read_bytes(&self, permissions: Permissions) -> Result<Option<Vec<u8>>, Error> {
        private_file::read(&self.path, permissions).map_err(store_error)
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
        let content =
            serde_json::to_vec_pretty(entries).map_err(|error| self.error(error.to_string()))?;
        private_file::write(&self.path, &content).map_err(store_error)
    }

    fn remove(&self) -> Result<(), Error> {
        private_file::remove(&self.path).map_err(store_error)
    }
}

fn store_error(error: PrivateFileError) -> Error {
    Error::CredentialStore {
        source: Box::new(error),
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
