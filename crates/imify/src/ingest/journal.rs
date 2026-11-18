//! The resume journal: the recording of an upload and the files storage has, one JSON
//! Lines file per API base URL and dataset directory.
//!
//! The first line is a [`Header`] with the recording id, the `requestId` of its create
//! and its settings. Each further line is an [`Entry`] for one file storage has, with the
//! SHA-256 announced for it. A partial last line, left by a crash, is ignored.
//!
//! File system calls run on the blocking thread pool, so writing and flushing an entry
//! does not hold up the tasks that send files.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Url;
use rustix::fs::OFlags;
use rustix::process::geteuid;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::plan::{PlannedFile, UploadPlan};
use super::recording::{NewRecording, RecordingId, RequestId};
use super::transport::Digest;
use crate::{Error, JournalError, local_file};

/// The journal format: 2 since uploads go into recordings. Version 1 journals hold an
/// ingest session of the old API.
const VERSION: u32 = 2;
/// Largest journal read.
const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;
const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

/// The first line of a journal.
#[derive(Debug, Serialize, Deserialize)]
struct Header {
    version: u32,
    api_url: String,
    root: PathBuf,
    /// The `{r}` of `recordings/{r}`.
    recording: String,
    /// The `requestId` the recording was created with.
    request_id: String,
    settings: Settings,
}

/// The settings of the journaled recording, as [`NewRecording`] holds them.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Settings {
    display_name: String,
    task: String,
    robot: Option<String>,
    operator: Option<String>,
    site: Option<String>,
    start_time: Option<String>,
    labels: BTreeMap<String, String>,
}

impl From<&NewRecording> for Settings {
    fn from(recording: &NewRecording) -> Self {
        Self {
            display_name: recording.display_name.clone(),
            task: recording.task.clone(),
            robot: recording.robot.clone(),
            operator: recording.operator.clone(),
            site: recording.site.clone(),
            start_time: recording.start_time.clone(),
            labels: recording.labels.clone(),
        }
    }
}

/// Only the version of a header, read before the rest so that another version is
/// reported as such.
#[derive(Deserialize)]
struct Versioned {
    version: u32,
}

/// One file storage has.
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
            .and_then(|()| check_private(dir))
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
        let bytes = match local_file::read_capped(&self.path, MAX_JOURNAL_BYTES) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source)
                if matches!(
                    source.kind(),
                    io::ErrorKind::InvalidInput | io::ErrorKind::FileTooLarge
                ) =>
            {
                return Err(self.error(JournalError::Corrupt(source.to_string())));
            }
            Err(source) => return Err(self.error(JournalError::Io(source))),
        };
        let text = String::from_utf8(bytes)
            .map_err(|error| self.error(JournalError::Corrupt(error.to_string())))?;
        let recorded = self.parse(&text)?;
        if recorded.header.api_url != self.api_url || recorded.header.root != self.root {
            return Err(self.error(JournalError::Corrupt(
                "the header names another API URL or dataset directory".to_owned(),
            )));
        }
        Ok(Some(recorded))
    }

    /// Writes a journal with only the header for `recording` and opens it for
    /// appending. The header is written to a temporary file first and renamed into place,
    /// so a journal never exists without a header.
    pub(crate) async fn create(
        &self,
        recording: &RecordingId,
        request_id: &RequestId,
        settings: &NewRecording,
    ) -> Result<JournalWriter, Error> {
        let header = Header {
            version: VERSION,
            api_url: self.api_url.clone(),
            root: self.root.clone(),
            recording: recording.as_str().to_owned(),
            request_id: request_id.as_str().to_owned(),
            settings: Settings::from(settings),
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
            // Removes a file or link left by a crash; the link itself, not its target.
            match fs::remove_file(&temporary) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(no_follow())
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
            .custom_flags(no_follow())
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
            Some((_, line)) if !line.is_empty() => {
                parse_header(line).map_err(|reason| self.error(reason))?
            }
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

fn parse_header(line: &str) -> Result<Header, JournalError> {
    let header_error = |error: serde_json::Error| JournalError::Corrupt(format!("line 1: {error}"));
    let Versioned { version } = serde_json::from_str(line).map_err(header_error)?;
    if version < VERSION {
        return Err(JournalError::Outdated(version));
    }
    if version != VERSION {
        return Err(JournalError::Corrupt(format!("unknown version {version}")));
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
    /// Returns the journaled recording.
    pub(crate) fn recording(&self) -> RecordingId {
        RecordingId::new(self.header.recording.clone())
    }

    /// Returns the SHA-256 the journal records for the files of `plan` whose size and
    /// modification time did not change since.
    ///
    /// Fails with [`JournalError::Mismatch`] when `settings` differ from the settings of
    /// the journaled recording, or a journaled file is not in `plan`, and with
    /// [`JournalError::Corrupt`] for an entry whose SHA-256 is not 64 hexadecimal digits.
    pub(crate) fn unchanged_digests(
        &self,
        plan: &UploadPlan,
        settings: &NewRecording,
    ) -> Result<HashMap<String, Digest>, Error> {
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
        let mut unchanged = HashMap::new();
        for (path, entry) in latest {
            let file = planned.get(path).ok_or_else(|| {
                self.mismatch(format!("{path} is no longer in the dataset directory"))
            })?;
            let modified = nanos_since_epoch(file.modified());
            if file.size() == entry.size && modified.is_some() && modified == entry.modified_ns {
                let digest = parse_hex(&entry.sha256).ok_or_else(|| {
                    journal_error(
                        &self.path,
                        JournalError::Corrupt(format!("{path} has an invalid SHA-256")),
                    )
                })?;
                unchanged.insert(path.to_owned(), digest);
            }
        }
        Ok(unchanged)
    }

    fn check_settings(&self, settings: &NewRecording) -> Result<(), Error> {
        let journaled = &self.header.settings;
        let wanted = Settings::from(settings);
        if journaled.display_name != wanted.display_name {
            return Err(self.mismatch(format!(
                "the journaled recording is named {:?}",
                journaled.display_name
            )));
        }
        if journaled.task != wanted.task {
            return Err(self.mismatch(format!(
                "the journaled recording has the task {:?}",
                journaled.task
            )));
        }
        if *journaled != wanted {
            return Err(self.mismatch(
                "the journaled recording has another robot, operator, site, start time or labels"
                    .to_owned(),
            ));
        }
        Ok(())
    }

    fn mismatch(&self, message: String) -> Error {
        journal_error(&self.path, JournalError::Mismatch(message))
    }
}

/// Appends an entry for each file storage has to a journal.
#[derive(Debug)]
pub(crate) struct JournalWriter {
    path: PathBuf,
    file: Arc<Mutex<File>>,
}

impl JournalWriter {
    /// Records that storage has `file` with the content `digest`. Returns once the entry
    /// is on disk.
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

/// The `open` flag that fails on a symbolic link as the last path component.
fn no_follow() -> i32 {
    OFlags::NOFOLLOW.bits().cast_signed()
}

/// Fails with [`io::ErrorKind::PermissionDenied`] unless `dir` is a directory owned by the
/// current user and not writable by group or others.
fn check_private(dir: &Path) -> io::Result<()> {
    let metadata = fs::metadata(dir)?;
    let refused = |reason: &str| {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the journal directory {reason}"),
        ))
    };
    if !metadata.is_dir() {
        return refused("is not a directory");
    }
    if metadata.uid() != geteuid().as_raw() {
        return refused("is owned by another user");
    }
    if metadata.mode() & 0o022 != 0 {
        return refused("is writable by group or others; run `chmod 700` on it");
    }
    Ok(())
}

fn json_line(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    Ok(line)
}

fn nanos_since_epoch(time: Option<SystemTime>) -> Option<u128> {
    Some(time?.duration_since(UNIX_EPOCH).ok()?.as_nanos())
}

/// Returns the digest of 64 hexadecimal digits, `None` for other text.
fn parse_hex(text: &str) -> Option<Digest> {
    if text.len() != 64 {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (byte, pair) in digest.iter_mut().zip(text.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(digest)
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
