//! The login in `credentials.json`, readable only by the user.

use std::collections::BTreeMap;
use std::fs::DirBuilder;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use imify::Error;
use imify::auth::Credentials;

const FILE_NAME: &str = "credentials.json";

/// Permission bits that let users other than the owner access the file.
const GROUP_AND_OTHER: u32 = 0o077;

/// The file's content: one entry per host key. Entries of other hosts are kept unparsed.
type Entries = BTreeMap<String, serde_json::Value>;

/// The login of one host in `<dir>/credentials.json`.
///
/// The file has mode `0600`; a new directory gets mode `0700`. A file that users other than
/// the owner can access is rejected. Writes replace the file atomically.
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

    fn error(&self, message: impl Into<String>) -> Error {
        Error::CredentialStore {
            source: Box::new(FileStoreError {
                path: self.path.clone(),
                message: message.into(),
            }),
        }
    }

    fn read(&self) -> Result<Entries, Error> {
        let metadata = match std::fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Entries::new()),
            Err(error) => return Err(self.error(error.to_string())),
        };
        let mode = metadata.permissions().mode();
        if mode & GROUP_AND_OTHER != 0 {
            return Err(self.error(format!(
                "other users can access the file (mode {:o}); run `chmod 600 {}`",
                mode & 0o777,
                self.path.display()
            )));
        }
        let content =
            std::fs::read_to_string(&self.path).map_err(|error| self.error(error.to_string()))?;
        serde_json::from_str(&content)
            .map_err(|error| self.error(format!("the file is not valid JSON: {error}")))
    }

    fn write(&self, entries: &Entries) -> Result<(), Error> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|error| self.error(format!("cannot create the directory: {error}")))?;
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
        Ok(())
    }
}

impl imify::auth::CredentialStore for FileStore {
    fn load(&self) -> Result<Option<Credentials>, Error> {
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
        let mut entries = self.read()?;
        let entry =
            serde_json::to_value(credentials).map_err(|error| self.error(error.to_string()))?;
        entries.insert(self.host_key.clone(), entry);
        self.write(&entries)
    }

    fn delete(&self) -> Result<bool, Error> {
        let mut entries = self.read()?;
        if entries.remove(&self.host_key).is_none() {
            return Ok(false);
        }
        self.write(&entries)?;
        Ok(true)
    }
}

/// The credentials file cannot be used.
#[derive(Debug, thiserror::Error)]
#[error("{}: {message}", path.display())]
pub struct FileStoreError {
    path: PathBuf,
    message: String,
}
