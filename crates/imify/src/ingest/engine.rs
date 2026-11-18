//! The upload flow: recording, files, completion.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::stream::{self, StreamExt};
use reqwest::{Method, Url};
use serde::Deserialize;

use super::journal::{JournalFile, JournalWriter, Recorded, hex};
use super::pace::Pacer;
use super::plan::{PlannedFile, UploadPlan};
use super::recording::{
    NewRecording, Recording, RecordingId, RecordingState, RequestId, WireRecording, check_paths,
    method_url, recording_url, recordings_url,
};
use super::transport::{Digest, FileUpload, ProgressFn, StreamingPut, Transport, UploadTarget};
use super::{ResumePolicy, UploadEvent, UploadOptions, digest};
use crate::client::is_loopback;
use crate::retry::RetryMode;
use crate::{Client, Error, JournalError, UploadError, resource};

type EventHandler = Arc<dyn Fn(&UploadEvent) + Send + Sync>;

/// Files announced per `files:batchCreate` call: the most the API takes.
const ANNOUNCE_BATCH: usize = 1000;

/// Uploads a dataset directory into a recording.
///
/// [`Uploader::upload`] runs the whole flow. [`Uploader::create_recording`],
/// [`Uploader::send_files`] and [`Uploader::complete`] run its steps one by one, for
/// example to continue an interrupted upload of an existing recording.
///
/// Dropping a future returned by an `Uploader` method stops every transfer it started.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
/// use imify::ingest::{NewRecording, UploadEvent, UploadPlan, Uploader};
///
/// let plan = UploadPlan::from_dir("datasets/pick-place")?;
/// let recording = Uploader::new(client)
///     .on_event(|event| {
///         if let UploadEvent::FileFinished { path, .. } = event {
///             println!("sent {path}");
///         }
///     })
///     .upload(&plan, &NewRecording::new("pick-place"))
///     .await?;
/// println!("recording {} is {}", recording.id, recording.state);
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct Uploader {
    client: Client,
    transport: Arc<dyn Transport>,
    options: UploadOptions,
    handler: EventHandler,
    journal: Option<JournalSettings>,
}

#[derive(Debug, Clone)]
struct JournalSettings {
    dir: PathBuf,
    policy: ResumePolicy,
}

impl fmt::Debug for Uploader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Uploader")
            .field("options", &self.options)
            .field("journal", &self.journal)
            .finish_non_exhaustive()
    }
}

impl Uploader {
    /// Creates an uploader that sends requests through `client`, with default
    /// [`UploadOptions`] and no event handler.
    ///
    /// # Examples
    ///
    /// ```
    /// let client = imify::Client::builder("https://api.example.com").build()?;
    /// let uploader = imify::ingest::Uploader::new(client);
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn new(client: Client) -> Self {
        let transport = Arc::new(StreamingPut::new(client.clone()));
        Self::with_transport(client, transport)
    }

    pub(crate) fn with_transport(client: Client, transport: Arc<dyn Transport>) -> Self {
        Self {
            client,
            transport,
            options: UploadOptions::default(),
            handler: Arc::new(|_| {}),
            journal: None,
        }
    }

    /// Replaces the upload options.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::num::NonZeroUsize;
    /// use imify::ingest::{UploadOptions, Uploader};
    ///
    /// let mut options = UploadOptions::default();
    /// options.concurrency = NonZeroUsize::new(8).unwrap();
    /// let client = imify::Client::builder("https://api.example.com").build()?;
    /// let uploader = Uploader::new(client).options(options);
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn options(mut self, options: UploadOptions) -> Self {
        self.options = options;
        self
    }

    /// Sets the function that receives every [`UploadEvent`]. It is called from the
    /// tasks that send files, possibly from several threads at once, and must return
    /// quickly.
    ///
    /// # Examples
    ///
    /// ```
    /// let client = imify::Client::builder("https://api.example.com").build()?;
    /// let uploader = imify::ingest::Uploader::new(client)
    ///     .on_event(|event| println!("{event:?}"));
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn on_event(mut self, handler: impl Fn(&UploadEvent) + Send + Sync + 'static) -> Self {
        self.handler = Arc::new(handler);
        self
    }

    /// Makes [`Uploader::upload`] record the recording and its uploaded files in a
    /// journal below `dir` and apply `policy`. [`Uploader::create_recording`],
    /// [`Uploader::send_files`] and [`Uploader::complete`] never read or write a journal.
    ///
    /// The journal is a file in `dir`, one per API base URL and canonical dataset
    /// directory, with mode `0600`. `upload` creates `dir` with mode `0700` when it does
    /// not exist.
    ///
    /// # Examples
    ///
    /// ```
    /// use imify::ingest::{ResumePolicy, Uploader};
    ///
    /// let client = imify::Client::builder("https://api.example.com").build()?;
    /// let uploader = Uploader::new(client)
    ///     .journal("/home/robot/.local/state/imify/uploads", ResumePolicy::Required);
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn journal(mut self, dir: impl Into<PathBuf>, policy: ResumePolicy) -> Self {
        self.journal = Some(JournalSettings {
            dir: dir.into(),
            policy,
        });
        self
    }

    /// Creates a recording, uploads every file of `plan` to storage and completes the
    /// recording.
    ///
    /// `upload` first checks `recording` with [`NewRecording::check`] and the length of
    /// every path, then computes the SHA-256 of every file: the API takes both before
    /// the first byte.
    ///
    /// With [`Uploader::journal`], `upload` then applies the [`ResumePolicy`]. To continue
    /// a journaled recording, it reads the recording: when its upload is done, `upload`
    /// returns it. Otherwise it announces every file again and uploads the files storage
    /// does not have, reported in [`UploadEvent::RecordingResumed`]. The SHA-256 of a
    /// journaled file whose size and modification time are unchanged is read from the
    /// journal. Without a journal, it creates a recording and writes a journal for it.
    /// Each uploaded file is recorded before [`UploadEvent::FileFinished`] is reported.
    /// The journal is deleted when [`Uploader::complete`] succeeds and kept after any
    /// error.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] before any request when a value of `recording` or a
    /// path is outside the limits of the API, and the first error of
    /// [`Uploader::create_recording`], [`Uploader::send_files`] or
    /// [`Uploader::complete`]. The recording stays open when a file fails; its id is
    /// reported in [`UploadEvent::RecordingCreated`].
    ///
    /// With a journal, also returns [`Error::Journal`], before any request unless the
    /// journal cannot be written:
    ///
    /// - [`JournalError::Missing`] for [`ResumePolicy::Required`] when no journal exists.
    /// - [`JournalError::Mismatch`] when the journaled recording has other settings than
    ///   `recording`, or a journaled file is not in `plan`.
    /// - [`JournalError::Outdated`] when an older SDK version wrote the journal.
    /// - [`JournalError::Corrupt`] when the journal cannot be parsed.
    /// - [`JournalError::Io`] when the journal or its directory cannot be read, written
    ///   or deleted.
    ///
    /// [`Error::Io`] is returned when the dataset directory or a file cannot be read.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(uploader: imify::ingest::Uploader) -> Result<(), imify::Error> {
    /// use imify::ingest::{NewRecording, UploadPlan};
    ///
    /// let plan = UploadPlan::from_dir("datasets/pick-place")?;
    /// let recording = uploader.upload(&plan, &NewRecording::new("pick-place")).await?;
    /// println!("{} episodes", recording.episode_count);
    /// # Ok(()) }
    /// ```
    pub async fn upload(
        &self,
        plan: &UploadPlan,
        recording: &NewRecording,
    ) -> Result<Recording, Error> {
        recording.check()?;
        check_paths(plan)?;
        if let Some(settings) = &self.journal {
            return self.upload_journaled(settings, plan, recording).await;
        }
        let digests = digest::all(plan, &HashMap::new()).await?;
        let created = self
            .create_recording(recording, &RequestId::random()?)
            .await?;
        if created.state != RecordingState::Uploading {
            return Ok(created);
        }
        let files = Files::new(plan, &digests, None);
        self.send_announced(&created.id, &files, Announce::Fresh)
            .await?;
        self.complete(&created.id).await
    }

    async fn upload_journaled(
        &self,
        settings: &JournalSettings,
        plan: &UploadPlan,
        recording: &NewRecording,
    ) -> Result<Recording, Error> {
        let journal =
            JournalFile::locate(&settings.dir, self.client.base_url(), plan.root()).await?;
        let Some(recorded) = self.journaled(&journal, settings.policy).await? else {
            return self.start_journaled(&journal, plan, recording).await;
        };
        let known = recorded.unchanged_digests(plan, recording)?;
        let id = recorded.recording();
        let current = self.get_recording(&id).await?;
        if current.state != RecordingState::Uploading {
            self.emit(&UploadEvent::RecordingResumed {
                recording: id,
                files_done: plan.files().len(),
                bytes_done: plan.total_bytes(),
            });
            journal.delete().await?;
            return Ok(current);
        }
        let digests = digest::all(plan, &known).await?;
        let writer = journal.writer().await?;
        let files = Files::new(plan, &digests, Some((&writer, &known)));
        self.send_announced(&id, &files, Announce::Resumed).await?;
        let completed = self.complete(&id).await?;
        journal.delete().await?;
        Ok(completed)
    }

    /// Applies `policy` to `journal`. Returns the journal to continue, or `None` when the
    /// upload needs a new recording.
    async fn journaled(
        &self,
        journal: &JournalFile,
        policy: ResumePolicy,
    ) -> Result<Option<Recorded>, Error> {
        if policy == ResumePolicy::Restart {
            journal.delete().await?;
            return Ok(None);
        }
        let recorded = journal.load().await?;
        if recorded.is_none() && policy == ResumePolicy::Required {
            return Err(journal.error(JournalError::Missing));
        }
        Ok(recorded)
    }

    /// Creates a recording and a journal for it, and uploads every file of `plan`.
    async fn start_journaled(
        &self,
        journal: &JournalFile,
        plan: &UploadPlan,
        recording: &NewRecording,
    ) -> Result<Recording, Error> {
        let digests = digest::all(plan, &HashMap::new()).await?;
        let request_id = RequestId::random()?;
        let created = self.create_recording(recording, &request_id).await?;
        if created.state != RecordingState::Uploading {
            return Ok(created);
        }
        let writer = journal.create(&created.id, &request_id, recording).await?;
        let journaled = HashMap::new();
        let files = Files::new(plan, &digests, Some((&writer, &journaled)));
        self.send_announced(&created.id, &files, Announce::Fresh)
            .await?;
        let completed = self.complete(&created.id).await?;
        journal.delete().await?;
        Ok(completed)
    }

    /// Creates a recording (`POST /api/v1/recordings`) and reports it in
    /// [`UploadEvent::RecordingCreated`].
    ///
    /// The request is retried on network errors, server errors and rate limits: a
    /// repeated create with the same `request_id` returns the first recording, in its
    /// current state. When that state is not [`RecordingState::Uploading`], the upload of
    /// the recording is done.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when a value of `recording` is outside the limits of
    /// the API, and the other [`Error`] of the request, such as [`Error::Auth`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(uploader: imify::ingest::Uploader) -> Result<(), imify::Error> {
    /// use imify::ingest::{NewRecording, RequestId};
    ///
    /// let recording = uploader
    ///     .create_recording(&NewRecording::new("pick-place"), &RequestId::random()?)
    ///     .await?;
    /// println!("recording {}", recording.id);
    /// # Ok(()) }
    /// ```
    pub async fn create_recording(
        &self,
        recording: &NewRecording,
        request_id: &RequestId,
    ) -> Result<Recording, Error> {
        recording.check()?;
        let created: WireRecording = self
            .client
            .execute_reporting(
                RetryMode::Idempotent,
                Method::POST,
                recordings_url(&self.client)?,
                Some(&recording.to_json(request_id)),
                self.rate_limit_reporter(),
            )
            .await?;
        let created = Recording::from(created);
        self.emit(&UploadEvent::RecordingCreated {
            recording: created.id.clone(),
        });
        Ok(created)
    }

    /// Returns the recording `id` (`GET /api/v1/recordings/{id}`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] when no recording has the id, and the other [`Error`]
    /// variants as for every request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(uploader: imify::ingest::Uploader) -> Result<(), imify::Error> {
    /// let recording = uploader
    ///     .get_recording(&imify::ingest::RecordingId::new("3f2c9a"))
    ///     .await?;
    /// println!("{}", recording.state);
    /// # Ok(()) }
    /// ```
    pub async fn get_recording(&self, id: &RecordingId) -> Result<Recording, Error> {
        let recording: WireRecording =
            resource::get(&self.client, recording_url(&self.client, id, None)?).await?;
        Ok(Recording::from(recording))
    }

    /// Uploads every file of `plan` that storage does not have to the recording `id`.
    ///
    /// Computes the SHA-256 of every file, announces the files with
    /// `POST /api/v1/recordings/{id}/files:batchCreate` (at most 1000 per call) and
    /// sends each file that is not uploaded with one `PUT` to its presigned URL, with
    /// [`UploadOptions::concurrency`] files in flight at once. A file is sent again from
    /// its first byte on network errors, server errors and rate limits; after a 403 of
    /// storage with a fresh URL, once; after a mismatching checksum, once.
    ///
    /// # Errors
    ///
    /// Returns the error of the first file that fails after its retries: [`Error::Upload`]
    /// for a file storage refuses, [`Error::Conflict`] with the reason `FILE_CONFLICT` for
    /// a path the recording knows with another size or SHA-256, and [`Error::Io`] for a
    /// file that cannot be read or changed size. No further file starts and the files in
    /// flight are stopped.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(uploader: imify::ingest::Uploader) -> Result<(), imify::Error> {
    /// use imify::ingest::{RecordingId, UploadPlan};
    ///
    /// let plan = UploadPlan::from_dir("datasets/pick-place")?;
    /// uploader.send_files(&RecordingId::new("3f2c9a"), &plan).await?;
    /// # Ok(()) }
    /// ```
    pub async fn send_files(&self, id: &RecordingId, plan: &UploadPlan) -> Result<(), Error> {
        check_paths(plan)?;
        let digests = digest::all(plan, &HashMap::new()).await?;
        let files = Files::new(plan, &digests, None);
        self.send_announced(id, &files, Announce::Fresh).await
    }

    /// Completes the recording (`POST /api/v1/recordings/{id}:complete`): the API reads
    /// the dataset and catalogues its episodes. Reports [`UploadEvent::Completing`]
    /// first.
    ///
    /// Completing is idempotent: a repeated call returns the same dataset and episode
    /// count with the current state. The request is retried on network errors, server
    /// errors and rate limits.
    ///
    /// # Errors
    ///
    /// Returns [`Error::FailedPrecondition`] with the reason `FILES_NOT_UPLOADED` and one
    /// violation per missing file, [`Error::Validation`] with the reason
    /// `INVALID_DATASET` when the uploaded `meta/` is not a readable `LeRobot` dataset, or
    /// another [`Error`] of the request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(uploader: imify::ingest::Uploader) -> Result<(), imify::Error> {
    /// let recording = uploader
    ///     .complete(&imify::ingest::RecordingId::new("3f2c9a"))
    ///     .await?;
    /// println!("{}", recording.state);
    /// # Ok(()) }
    /// ```
    pub async fn complete(&self, id: &RecordingId) -> Result<Recording, Error> {
        self.emit(&UploadEvent::Completing);
        let completed: WireRecording = self
            .client
            .execute_reporting(
                RetryMode::Idempotent,
                Method::POST,
                method_url(&self.client, id, "complete")?,
                None,
                self.rate_limit_reporter(),
            )
            .await?;
        Ok(Recording::from(completed))
    }

    /// Announces the files and uploads those storage does not have. For a resumed
    /// upload, reports [`UploadEvent::RecordingResumed`] with the files storage has.
    async fn send_announced(
        &self,
        id: &RecordingId,
        files: &Files<'_>,
        announce: Announce,
    ) -> Result<(), Error> {
        let announced = self.announce(id, files.plan.files(), files.digests).await?;
        let mut pending = Vec::new();
        let mut targets = HashMap::new();
        let (mut files_done, mut bytes_done) = (0, 0);
        for file in files.plan.files() {
            if let Some(Announced::Pending(target)) = announced.get(file.path()) {
                pending.push(file);
                targets.insert(file.path().to_owned(), target.clone());
            } else {
                files.record(file).await?;
                files_done += 1;
                bytes_done += file.size();
            }
        }
        if announce == Announce::Resumed {
            self.emit(&UploadEvent::RecordingResumed {
                recording: id.clone(),
                files_done,
                bytes_done,
            });
        }
        let batch = Batch {
            recording: id,
            files,
            pacer: Pacer::new(self.options.request_interval),
            targets: Mutex::new(targets),
        };
        let mut transfers = stream::iter(pending)
            .map(|file| self.send_file(&batch, file))
            .buffer_unordered(self.options.concurrency.get());
        while let Some(result) = transfers.next().await {
            result?;
        }
        Ok(())
    }

    /// Announces `files` with `files:batchCreate`, at most [`ANNOUNCE_BATCH`] per call.
    /// Returns what the API answered for each path.
    async fn announce(
        &self,
        id: &RecordingId,
        files: &[PlannedFile],
        digests: &HashMap<String, Digest>,
    ) -> Result<HashMap<String, Announced>, Error> {
        let url = recording_url(&self.client, id, Some("files:batchCreate"))?;
        let parent = id.name();
        let mut announced = HashMap::with_capacity(files.len());
        for chunk in files.chunks(ANNOUNCE_BATCH) {
            let requests: Vec<serde_json::Value> = chunk
                .iter()
                .map(|file| {
                    serde_json::json!({
                        "parent": parent,
                        "file": {
                            "path": file.path(),
                            "sizeBytes": file.size(),
                            "sha256": hex(&digests[file.path()]),
                        },
                    })
                })
                .collect();
            let answer: AnnouncedFiles = self
                .client
                .execute_reporting(
                    RetryMode::Idempotent,
                    Method::POST,
                    url.clone(),
                    Some(&serde_json::json!({ "requests": requests })),
                    self.rate_limit_reporter(),
                )
                .await?;
            for file in answer.files {
                let state = file.state()?;
                announced.insert(file.path, state);
            }
            if let Some(missing) = chunk
                .iter()
                .find(|file| !announced.contains_key(file.path()))
            {
                return Err(Error::UnexpectedResponse {
                    status: 200,
                    message: format!("files:batchCreate does not list {}", missing.path()),
                });
            }
        }
        Ok(announced)
    }

    async fn send_file(&self, batch: &Batch<'_>, file: &PlannedFile) -> Result<(), Error> {
        self.emit(&UploadEvent::FileStarted {
            path: file.path().to_owned(),
            size: file.size(),
        });
        let retries = AtomicU32::new(0);
        self.transfer(batch, file, &retries).await?;
        batch.files.record(file).await?;
        self.emit(&UploadEvent::FileFinished {
            path: file.path().to_owned(),
            size: file.size(),
        });
        Ok(())
    }

    /// Sends `file` until storage holds it. An expired URL is renewed once, and a file
    /// whose bytes do not match its SHA-256 is read once more; any other failure ends the
    /// transfer.
    async fn transfer(
        &self,
        batch: &Batch<'_>,
        file: &PlannedFile,
        retries: &AtomicU32,
    ) -> Result<(), Error> {
        let mut target = batch.target(file)?;
        let (mut url_renewed, mut file_reread) = (false, false);
        loop {
            let Err(error) = self.put_with_retries(batch, file, &target, retries).await else {
                return Ok(());
            };
            match &error {
                Error::Upload {
                    reason: UploadError::UrlExpired,
                    ..
                } if !url_renewed => {
                    url_renewed = true;
                    self.report_retry(file, retries, &error);
                    match self.renew_target(batch, file).await? {
                        Some(renewed) => target = renewed,
                        None => return Ok(()),
                    }
                }
                Error::Upload {
                    reason: UploadError::ChecksumMismatch,
                    ..
                } if !file_reread => {
                    file_reread = true;
                    self.report_retry(file, retries, &error);
                }
                _ => return Err(error),
            }
        }
    }

    /// Sends `file` to `target`, again from its first byte after network errors, storage
    /// errors and rate limits, as the retry policy allows.
    async fn put_with_retries(
        &self,
        batch: &Batch<'_>,
        file: &PlannedFile,
        target: &UploadTarget,
        retries: &AtomicU32,
    ) -> Result<(), Error> {
        let upload = FileUpload {
            target,
            file,
            local_path: batch.files.plan.root().join(file.path()),
            progress: self.progress_handler(file),
        };
        let (upload, pacer) = (&upload, &batch.pacer);
        self.client
            .retry(
                RetryMode::Idempotent,
                |_, error, _| self.report_retry(file, retries, error),
                || async move {
                    pacer.ready().await;
                    self.transport.put(upload).await
                },
            )
            .await
    }

    /// Reports that `file` is sent again after `error`, counting the retries of the file.
    fn report_retry(&self, file: &PlannedFile, retries: &AtomicU32, error: &Error) {
        self.emit(&UploadEvent::FileRetrying {
            path: file.path().to_owned(),
            attempt: retries.fetch_add(1, Ordering::Relaxed) + 1,
            reason: error.to_string(),
        });
    }

    /// Announces `file` again for a fresh upload URL. `None` when storage has the file.
    async fn renew_target(
        &self,
        batch: &Batch<'_>,
        file: &PlannedFile,
    ) -> Result<Option<UploadTarget>, Error> {
        let announced = self
            .announce(
                batch.recording,
                std::slice::from_ref(file),
                batch.files.digests,
            )
            .await?;
        Ok(match announced.get(file.path()) {
            Some(Announced::Pending(target)) => Some(target.clone()),
            _ => None,
        })
    }

    /// The retry handler of an API request: reports each rate limit with its pause.
    fn rate_limit_reporter(&self) -> impl FnMut(u32, &Error, Duration) + '_ {
        |_, error, delay| {
            if matches!(error, Error::RateLimited { .. }) {
                self.emit(&UploadEvent::RateLimited { pause: delay });
            }
        }
    }

    fn progress_handler(&self, file: &PlannedFile) -> ProgressFn {
        let handler = Arc::clone(&self.handler);
        let path = file.path().to_owned();
        Arc::new(move |bytes_sent| {
            handler(&UploadEvent::FileProgress {
                path: path.clone(),
                bytes_sent,
            });
        })
    }

    fn emit(&self, event: &UploadEvent) {
        (self.handler)(event);
    }
}

/// Whether the announced files belong to a resumed upload, which reports the files
/// storage already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Announce {
    Fresh,
    Resumed,
}

/// The files of a plan with their SHA-256, and the journal that records the uploaded
/// ones together with the files it already records.
struct Files<'a> {
    plan: &'a UploadPlan,
    digests: &'a HashMap<String, Digest>,
    journal: Option<(&'a JournalWriter, &'a HashMap<String, Digest>)>,
}

impl<'a> Files<'a> {
    fn new(
        plan: &'a UploadPlan,
        digests: &'a HashMap<String, Digest>,
        journal: Option<(&'a JournalWriter, &'a HashMap<String, Digest>)>,
    ) -> Self {
        Self {
            plan,
            digests,
            journal,
        }
    }

    /// Records in the journal that storage has `file`, unless the journal records it.
    async fn record(&self, file: &PlannedFile) -> Result<(), Error> {
        match self.journal {
            Some((writer, journaled)) if !journaled.contains_key(file.path()) => {
                writer.append(file, &self.digests[file.path()]).await
            }
            _ => Ok(()),
        }
    }
}

/// The files of one plan being sent to one recording.
struct Batch<'a> {
    recording: &'a RecordingId,
    files: &'a Files<'a>,
    pacer: Pacer,
    targets: Mutex<HashMap<String, UploadTarget>>,
}

impl Batch<'_> {
    /// The upload target the announcement gave `file`.
    fn target(&self, file: &PlannedFile) -> Result<UploadTarget, Error> {
        self.targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(file.path())
            .ok_or_else(|| Error::UnexpectedResponse {
                status: 200,
                message: format!("files:batchCreate gave {} no upload URL", file.path()),
            })
    }
}

/// What `files:batchCreate` answered for one path.
#[derive(Debug, Clone)]
enum Announced {
    /// Storage has the file, or the API offers no URL for it: nothing to send.
    Done,
    /// Send the file to the target.
    Pending(UploadTarget),
}

/// The answer of `files:batchCreate`.
#[derive(Deserialize)]
struct AnnouncedFiles {
    #[serde(default)]
    files: Vec<AnnouncedFile>,
}

/// One file of the answer of `files:batchCreate`. A file in state `PENDING` or
/// `SIZE_MISMATCH`, or in a state this SDK does not know, has an `uploadUri` while
/// storage does not have it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnnouncedFile {
    path: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    upload_uri: Option<String>,
    #[serde(default)]
    upload_headers: Option<HashMap<String, String>>,
}

impl AnnouncedFile {
    /// What to do with the file. Fails with [`Error::UnexpectedResponse`] for an upload
    /// URL that is not `https`, or `http` on `localhost`, `127.0.0.1` or `::1`.
    fn state(&self) -> Result<Announced, Error> {
        let Some(uri) = self
            .upload_uri
            .as_deref()
            .filter(|_| self.state != "UPLOADED")
        else {
            return Ok(Announced::Done);
        };
        let unsafe_url = |why: &str| Error::UnexpectedResponse {
            status: 200,
            message: format!("the upload URL of {} {why}", self.path),
        };
        let uri = Url::parse(uri).map_err(|_| unsafe_url("is not a URL"))?;
        if !(uri.scheme() == "https" || (uri.scheme() == "http" && is_loopback(&uri))) {
            return Err(unsafe_url(
                "must use https unless its host is localhost, 127.0.0.1 or ::1",
            ));
        }
        let mut headers: Vec<(String, String)> = self
            .upload_headers
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();
        headers.sort();
        Ok(Announced::Pending(UploadTarget { uri, headers }))
    }
}

#[cfg(test)]
mod tests;
