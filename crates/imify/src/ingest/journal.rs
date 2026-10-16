//! The resume journal: the files an upload stored in its session, one JSON Lines file
//! per API base URL and dataset directory.
//!
//! The first line is a [`Header`] with the session id. Each further line is an [`Entry`]
//! for one file the API stored. A partial last line, left by a crash, is ignored.
//!
//! File system calls run on the blocking thread pool, so writing and flushing an entry
//! does not hold up the tasks that send files.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt;

use super::plan::{PlannedFile, UploadPlan};
use super::session::{NewSession, SessionId};
use super::transport::Digest;
use crate::{Error, JournalError};

const VERSION: u32 = 1;
const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
const HASH_CHUNK_SIZE: usize = 256 * 1024;

/// The first line of a journal.
#[derive(Debug, Serialize, Deserialize)]
struct Header {
    version: u32,
    api_url: String,
    root: PathBuf,
    session: String,
    name: String,
    task_description: String,
}

/// Only the version of a header, read before the rest so that another version is
/// reported as such.
#[derive(Deserialize)]
struct Versioned {
    version: u32,
}

/// One file the API stored.
#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    path: String,
    size: u64,
    modified_ns: Option<u128>,
    sha256: String,
}

/// The journal file of the upload of one dataset directory to one API.
#[derive(Debug, Clone)]
pub(crate) struct JournalFile {
    path: PathBuf,
    api_url: String,
    root: PathBuf,
}

impl JournalFile {
    /// Creates `dir` with mode `0700` when it does not exist and returns the journal of
    /// the upload of `root` to `api_url` in it.
    pub(crate) async fn locate(dir: &Path, api_url: &Url, root: &Path) -> Result<Self, Error> {
        let (owned_dir, api_url, root) = (dir.to_owned(), api_url.clone(), root.to_owned());
        run_blocking(dir, move || {
            Self::locate_blocking(&owned_dir, &api_url, &root)
        })
        .await
    }

    fn locate_blocking(dir: &Path, api_url: &Url, root: &Path) -> Result<Self, Error> {
        DirBuilder::new()
            .recursive(true)
            .mode(DIR_MODE)
            .create(dir)
            .map_err(|source| journal_error(dir, JournalError::Io(source)))?;
        let root = fs::canonicalize(root).map_err(|source| Error::Io {
            path: root.to_owned(),
            source,
        })?;
        let mut key = Sha256::new();
        key.update(api_url.as_str());
        key.update("\n");
        key.update(root.as_os_str().as_bytes());
        let name = format!("{}.jsonl", hex(&key.finalize().into()));
        Ok(Self {
            path: dir.join(name),
            api_url: api_url.as_str().to_owned(),
            root,
        })
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the journal, `None` when it does not exist.
    pub(crate) async fn load(&self) -> Result<Option<Recorded>, Error> {
        let journal = self.clone();
        run_blocking(&self.path, move || journal.load_blocking()).await
    }

    fn load_blocking(&self) -> Result<Option<Recorded>, Error> {
        match fs::read_to_string(&self.path) {
            Ok(text) => self.parse(&text).map(Some),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(self.error(JournalError::Io(source))),
        }
    }

    /// Writes a journal with only the header for `session` and opens it for appending.
    /// The header is written to a temporary file first and renamed into place, so a
    /// journal never exists without a header.
    pub(crate) async fn create(
        &self,
        session: &SessionId,
        settings: &NewSession,
    ) -> Result<JournalWriter, Error> {
        let header = Header {
            version: VERSION,
            api_url: self.api_url.clone(),
            root: self.root.clone(),
            session: session.as_str().to_owned(),
            name: settings.name.clone(),
            task_description: settings.task_description.clone(),
        };
        let journal = self.clone();
        run_blocking(&self.path, move || {
            journal.write_header(&header)?;
            journal.writer_blocking()
        })
        .await
    }

    fn write_header(&self, header: &Header) -> Result<(), Error> {
        let temporary = self.path.with_extension("jsonl.tmp");
        let write = || -> io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(FILE_MODE)
                .open(&temporary)?;
            file.write_all(&json_line(header)?)?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)
        };
        write().map_err(|source| self.error(JournalError::Io(source)))
    }

    /// Opens the existing journal for appending.
    pub(crate) async fn writer(&self) -> Result<JournalWriter, Error> {
        let journal = self.clone();
        run_blocking(&self.path, move || journal.writer_blocking()).await
    }

    fn writer_blocking(&self) -> Result<JournalWriter, Error> {
        let file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|source| self.error(JournalError::Io(source)))?;
        Ok(JournalWriter {
            path: self.path.clone(),
            file: Arc::new(Mutex::new(file)),
        })
    }

    /// Deletes the journal when it exists.
    pub(crate) async fn delete(&self) -> Result<(), Error> {
        let journal = self.clone();
        run_blocking(&self.path, move || journal.delete_blocking()).await
    }

    fn delete_blocking(&self) -> Result<(), Error> {
        match fs::remove_file(&self.path) {
            Err(source) if source.kind() != io::ErrorKind::NotFound => {
                Err(self.error(JournalError::Io(source)))
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn error(&self, reason: JournalError) -> Error {
        journal_error(&self.path, reason)
    }

    fn parse(&self, text: &str) -> Result<Recorded, Error> {
        let corrupt = |message: String| self.error(JournalError::Corrupt(message));
        let complete = text.rfind('\n').map_or("", |end| &text[..end]);
        let mut lines = complete.split('\n').enumerate();
        let header = match lines.next() {
            Some((_, line)) if !line.is_empty() => parse_header(line).map_err(corrupt)?,
            _ => return Err(corrupt("the header is missing".to_owned())),
        };
        let entries = lines
            .map(|(index, line)| {
                serde_json::from_str(line)
                    .map_err(|error| corrupt(format!("line {}: {error}", index + 1)))
            })
            .collect::<Result<_, _>>()?;
        Ok(Recorded {
            path: self.path.clone(),
            header,
            entries,
        })
    }
}

fn parse_header(line: &str) -> Result<Header, String> {
    let header_error = |error: serde_json::Error| format!("line 1: {error}");
    let Versioned { version } = serde_json::from_str(line).map_err(header_error)?;
    if version != VERSION {
        return Err(format!("unknown version {version}"));
    }
    serde_json::from_str(line).map_err(header_error)
}

/// A journal as read from disk.
#[derive(Debug)]
pub(crate) struct Recorded {
    path: PathBuf,
    header: Header,
    entries: Vec<Entry>,
}

impl Recorded {
    /// Returns the journaled session.
    pub(crate) fn session(&self) -> SessionId {
        SessionId::new(self.header.session.clone())
    }

    /// Returns the paths of the files of `plan` that the journal records as stored and
    /// that did not change since. A file with the journaled size and modification time
    /// is unchanged; with only another modification time, it is unchanged when its
    /// SHA-256 equals the journaled one.
    ///
    /// Fails with [`JournalError::Mismatch`] when `settings` has another name or task
    /// description than the journaled session, or a journaled file is not in `plan`.
    pub(crate) async fn stored_files(
        &self,
        plan: &UploadPlan,
        settings: &NewSession,
    ) -> Result<HashSet<String>, Error> {
        self.check_settings(settings)?;
        let planned: HashMap<&str, &PlannedFile> = plan
            .files()
            .iter()
            .map(|file| (file.path(), file))
            .collect();
        let latest: HashMap<&str, &Entry> = self
            .entries
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect();
        let mut journaled = Vec::with_capacity(latest.len());
        for (path, entry) in latest {
            let file = planned.get(path).ok_or_else(|| {
                self.mismatch(format!("{path} is no longer in the dataset directory"))
            })?;
            journaled.push((*file, entry));
        }
        let mut stored = HashSet::new();
        for (file, entry) in journaled {
            if is_unchanged(plan.root(), file, entry).await? {
                stored.insert(file.path().to_owned());
            }
        }
        Ok(stored)
    }

    fn check_settings(&self, settings: &NewSession) -> Result<(), Error> {
        if self.header.name != settings.name {
            return Err(self.mismatch(format!(
                "the journaled session is named {:?}",
                self.header.name
            )));
        }
        if self.header.task_description != settings.task_description {
            return Err(self.mismatch(format!(
                "the journaled session has the task description {:?}",
                self.header.task_description
            )));
        }
        Ok(())
    }

    fn mismatch(&self, message: String) -> Error {
        journal_error(&self.path, JournalError::Mismatch(message))
    }
}

async fn is_unchanged(root: &Path, file: &PlannedFile, entry: &Entry) -> Result<bool, Error> {
    if file.size() != entry.size {
        return Ok(false);
    }
    let modified = nanos_since_epoch(file.modified());
    if modified.is_some() && modified == entry.modified_ns {
        return Ok(true);
    }
    Ok(hex(&file_sha256(&root.join(file.path())).await?) == entry.sha256)
}

async fn file_sha256(path: &Path) -> Result<Digest, Error> {
    let io_error = |source| Error::Io {
        path: path.to_owned(),
        source,
    };
    let mut file = tokio::fs::File::open(path).await.map_err(io_error)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; HASH_CHUNK_SIZE];
    loop {
        let read = file.read(&mut buffer).await.map_err(io_error)?;
        if read == 0 {
            return Ok(hasher.finalize().into());
        }
        hasher.update(&buffer[..read]);
    }
}

/// Appends an entry for each stored file to a journal.
#[derive(Debug)]
pub(crate) struct JournalWriter {
    path: PathBuf,
    file: Arc<Mutex<File>>,
}

impl JournalWriter {
    /// Records that the API stored `file` with the content `digest`. Returns once the
    /// entry is on disk.
    pub(crate) async fn append(&self, file: &PlannedFile, digest: &Digest) -> Result<(), Error> {
        let entry = Entry {
            path: file.path().to_owned(),
            size: file.size(),
            modified_ns: nanos_since_epoch(file.modified()),
            sha256: hex(digest),
        };
        let line = json_line(&entry).map_err(|source| self.error(source))?;
        let journal = Arc::clone(&self.file);
        let path = self.path.clone();
        run_blocking(&self.path, move || {
            // One `write_all` per line under the lock keeps concurrent entries whole.
            let mut journal = journal.lock().unwrap_or_else(PoisonError::into_inner);
            journal
                .write_all(&line)
                .and_then(|()| journal.sync_data())
                .map_err(|source| journal_error(&path, JournalError::Io(source)))
        })
        .await
    }

    fn error(&self, source: io::Error) -> Error {
        journal_error(&self.path, JournalError::Io(source))
    }
}

/// Runs `work` on the blocking thread pool. A task that panics or is cancelled fails
/// with [`JournalError::Io`] for the journal at `path`.
async fn run_blocking<T, F>(path: &Path, work: F) -> Result<T, Error>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, Error> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(join_error) => Err(journal_error(
            path,
            JournalError::Io(io::Error::from(join_error)),
        )),
    }
}

fn json_line(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    Ok(line)
}

fn nanos_since_epoch(time: Option<SystemTime>) -> Option<u128> {
    Some(time?.duration_since(UNIX_EPOCH).ok()?.as_nanos())
}

/// Returns `digest` as lower-case hexadecimal.
pub(crate) fn hex(digest: &Digest) -> String {
    digest
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

fn journal_error(path: &Path, reason: JournalError) -> Error {
    Error::Journal {
        path: path.to_owned(),
        reason,
    }
}

#[cfg(test)]
mod tests;
