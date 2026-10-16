//! The upload flow: session, files, completion.

use std::borrow::Cow;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::stream::{self, StreamExt};
use reqwest::Method;

use super::journal::{JournalFile, JournalWriter};
use super::pace::Pacer;
use super::plan::{INFO_FILE, PlannedFile, UploadPlan};
use super::session::{NewSession, Session, SessionId, WireSession, session_url, sessions_url};
use super::transport::{FileUpload, ProgressFn, StreamingPut, Transport};
use super::{ResumePolicy, UploadEvent, UploadOptions};
use crate::retry::RetryMode;
use crate::{Client, Error, JournalError};

type EventHandler = Arc<dyn Fn(&UploadEvent) + Send + Sync>;

/// Uploads a dataset directory into an ingest session.
///
/// [`Uploader::upload`] runs the whole flow. [`Uploader::create_session`],
/// [`Uploader::send_files`] and [`Uploader::complete`] run its steps one by one, for
/// example to continue an interrupted upload in an existing session.
///
/// Dropping a future returned by an `Uploader` method stops every transfer it started.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
/// use imify::ingest::{NewSession, UploadEvent, UploadPlan, Uploader};
///
/// let plan = UploadPlan::from_dir("datasets/pick-place")?;
/// let session = Uploader::new(client)
///     .on_event(|event| {
///         if let UploadEvent::FileFinished { path, .. } = event {
///             println!("sent {path}");
///         }
///     })
///     .upload(&plan, &NewSession::new("pick-place"))
///     .await?;
/// println!("session {} is {:?}", session.id, session.status);
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

/// The session of an upload with a journal and the files still to send to it.
struct Journaled<'a> {
    session: SessionId,
    remaining: Cow<'a, UploadPlan>,
    writer: JournalWriter,
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

    /// Makes [`Uploader::upload`] record stored files in a journal below `dir` and apply
    /// `policy`. [`Uploader::create_session`], [`Uploader::send_files`] and
    /// [`Uploader::complete`] never read or write a journal.
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

    /// Creates an ingest session, sends every file of `plan` and completes the session.
    ///
    /// With [`Uploader::journal`], `upload` first applies the [`ResumePolicy`]. To
    /// continue a journaled session, it skips the journaled files whose size and
    /// modification time are unchanged, or whose SHA-256 is unchanged, reports
    /// [`UploadEvent::SessionResumed`] and sends the other files to that session. Without
    /// a journal, it creates a session and writes a journal for it. Each stored file is
    /// recorded before [`UploadEvent::FileFinished`] is reported. The journal is deleted
    /// when [`Uploader::complete`] succeeds and kept after any error.
    ///
    /// # Errors
    ///
    /// Returns the first error of [`Uploader::create_session`], [`Uploader::send_files`]
    /// or [`Uploader::complete`]. The session stays open when a file fails; its id is
    /// reported in [`UploadEvent::SessionCreated`].
    ///
    /// With a journal, also returns [`Error::Journal`], before any request unless the
    /// journal cannot be written:
    ///
    /// - [`JournalError::Missing`] for [`ResumePolicy::Required`] when no journal exists.
    /// - [`JournalError::Mismatch`] when the journaled session has another name or task
    ///   description than `session`, or a journaled file is not in `plan`.
    /// - [`JournalError::Corrupt`] when the journal cannot be parsed.
    /// - [`JournalError::Io`] when the journal or its directory cannot be read, written
    ///   or deleted.
    ///
    /// [`Error::Io`] is returned when the dataset directory or a journaled file cannot
    /// be read.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(uploader: imify::ingest::Uploader) -> Result<(), imify::Error> {
    /// use imify::ingest::{NewSession, UploadPlan};
    ///
    /// let plan = UploadPlan::from_dir("datasets/pick-place")?;
    /// let session = uploader.upload(&plan, &NewSession::new("pick-place")).await?;
    /// println!("{} episodes", session.total_episodes.unwrap_or(0));
    /// # Ok(()) }
    /// ```
    pub async fn upload(&self, plan: &UploadPlan, session: &NewSession) -> Result<Session, Error> {
        if let Some(settings) = &self.journal {
            return self.upload_journaled(settings, plan, session).await;
        }
        let created = self.create_session(session).await?;
        self.send_files(&created.id, plan).await?;
        self.complete(&created.id).await
    }

    async fn upload_journaled(
        &self,
        settings: &JournalSettings,
        plan: &UploadPlan,
        session: &NewSession,
    ) -> Result<Session, Error> {
        let journal =
            JournalFile::locate(&settings.dir, self.client.base_url(), plan.root()).await?;
        let resumed = self
            .resume(&journal, settings.policy, plan, session)
            .await?;
        let journaled = if let Some(resumed) = resumed {
            resumed
        } else {
            self.start_journaled(&journal, plan, session).await?
        };
        self.send_recorded(
            &journaled.session,
            &journaled.remaining,
            Some(&journaled.writer),
        )
        .await?;
        let completed = self.complete(&journaled.session).await?;
        journal.delete().await?;
        Ok(completed)
    }

    /// Applies `policy` to `journal`. Returns the journaled session with the files of
    /// `plan` that are still to send, or `None` when the upload needs a new session.
    async fn resume<'a>(
        &self,
        journal: &JournalFile,
        policy: ResumePolicy,
        plan: &'a UploadPlan,
        settings: &NewSession,
    ) -> Result<Option<Journaled<'a>>, Error> {
        if policy == ResumePolicy::Restart {
            journal.delete().await?;
            return Ok(None);
        }
        let Some(recorded) = journal.load().await? else {
            return match policy {
                ResumePolicy::Required => Err(journal.error(JournalError::Missing)),
                ResumePolicy::IfJournaled | ResumePolicy::Restart => Ok(None),
            };
        };
        let stored = recorded.stored_files(plan, settings).await?;
        let mut remaining = plan.clone();
        remaining.retain(|file| !stored.contains(file.path()));
        let resumed = Journaled {
            session: recorded.session(),
            writer: journal.writer().await?,
            remaining: Cow::Owned(remaining),
        };
        self.emit(&UploadEvent::SessionResumed {
            session: resumed.session.clone(),
            files_done: plan.files().len() - resumed.remaining.files().len(),
            bytes_done: plan.total_bytes() - resumed.remaining.total_bytes(),
        });
        Ok(Some(resumed))
    }

    /// Creates a session and a journal for it, with every file of `plan` still to send.
    async fn start_journaled<'a>(
        &self,
        journal: &JournalFile,
        plan: &'a UploadPlan,
        settings: &NewSession,
    ) -> Result<Journaled<'a>, Error> {
        let created = self.create_session(settings).await?;
        Ok(Journaled {
            writer: journal.create(&created.id, settings).await?,
            session: created.id,
            remaining: Cow::Borrowed(plan),
        })
    }

    /// Creates an ingest session and reports it in [`UploadEvent::SessionCreated`].
    ///
    /// The request is retried only on connection failures and rate limits, because a
    /// repeated request opens a second session.
    ///
    /// # Errors
    ///
    /// Returns the [`Error`] of the request, such as [`Error::Auth`] or
    /// [`Error::Validation`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(uploader: imify::ingest::Uploader) -> Result<(), imify::Error> {
    /// let session = uploader
    ///     .create_session(&imify::ingest::NewSession::new("pick-place"))
    ///     .await?;
    /// println!("session {}", session.id);
    /// # Ok(()) }
    /// ```
    pub async fn create_session(&self, session: &NewSession) -> Result<Session, Error> {
        let created: WireSession = self
            .client
            .execute_at(
                RetryMode::NotIdempotent,
                Method::POST,
                sessions_url(&self.client)?,
                Some(&session.to_json()),
            )
            .await?;
        let created = Session::from(created);
        self.emit(&UploadEvent::SessionCreated {
            session: created.id.clone(),
        });
        Ok(created)
    }

    /// Sends every file of `plan` to the session.
    ///
    /// `meta/info.json` is sent alone first when the plan contains it. The other files
    /// follow with [`UploadOptions::concurrency`] in flight at once. A file is retried on
    /// network errors, server errors and rate limits, from its first byte.
    ///
    /// # Errors
    ///
    /// Returns the error of the first file that fails after its retries, such as
    /// [`Error::Validation`] for a rejected file or [`Error::Io`] for a file that cannot
    /// be read or changed size since planning. No further file starts and the files in
    /// flight are stopped.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(uploader: imify::ingest::Uploader) -> Result<(), imify::Error> {
    /// use imify::ingest::{SessionId, UploadPlan};
    ///
    /// let plan = UploadPlan::from_dir("datasets/pick-place")?;
    /// uploader.send_files(&SessionId::new("3f2c9a"), &plan).await?;
    /// # Ok(()) }
    /// ```
    pub async fn send_files(&self, session: &SessionId, plan: &UploadPlan) -> Result<(), Error> {
        self.send_recorded(session, plan, None).await
    }

    /// Sends every file of `plan` like [`Uploader::send_files`] and records each stored
    /// file in `journal`.
    async fn send_recorded(
        &self,
        session: &SessionId,
        plan: &UploadPlan,
        journal: Option<&JournalWriter>,
    ) -> Result<(), Error> {
        let batch = Batch {
            session,
            plan,
            journal,
            pacer: Pacer::new(self.options.request_interval),
        };
        let rest = match plan.files().split_first() {
            Some((info, rest)) if info.path() == INFO_FILE => {
                self.send_file(&batch, info).await?;
                rest
            }
            _ => plan.files(),
        };
        let mut transfers = stream::iter(rest)
            .map(|file| self.send_file(&batch, file))
            .buffer_unordered(self.options.concurrency.get());
        while let Some(result) = transfers.next().await {
            result?;
        }
        Ok(())
    }

    /// Completes the session: the API checks the files and catalogues the dataset.
    /// Reports [`UploadEvent::Completing`] first.
    ///
    /// Completing is idempotent: a complete session is returned again. The request is
    /// retried on network errors, server errors and rate limits.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when the API rejects the dataset, or another
    /// [`Error`] of the request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(uploader: imify::ingest::Uploader) -> Result<(), imify::Error> {
    /// let session = uploader
    ///     .complete(&imify::ingest::SessionId::new("3f2c9a"))
    ///     .await?;
    /// println!("{:?}", session.status);
    /// # Ok(()) }
    /// ```
    pub async fn complete(&self, session: &SessionId) -> Result<Session, Error> {
        self.emit(&UploadEvent::Completing);
        let completed: WireSession = self
            .client
            .execute_at(
                RetryMode::Idempotent,
                Method::POST,
                session_url(&self.client, session, ["complete"])?,
                None,
            )
            .await?;
        Ok(Session::from(completed))
    }

    async fn send_file(&self, batch: &Batch<'_>, file: &PlannedFile) -> Result<(), Error> {
        self.emit(&UploadEvent::FileStarted {
            path: file.path().to_owned(),
            size: file.size(),
        });
        let pacer = &batch.pacer;
        let upload = FileUpload {
            session: batch.session,
            file,
            local_path: batch.plan.root().join(file.path()),
            progress: self.progress_handler(file),
        };
        let upload = &upload;
        let on_retry = |attempt, error: &Error, delay| {
            if matches!(error, Error::RateLimited { .. }) {
                pacer.pause(delay);
                self.emit(&UploadEvent::RateLimited { pause: delay });
            }
            self.emit(&UploadEvent::FileRetrying {
                path: file.path().to_owned(),
                attempt,
                reason: error.to_string(),
            });
        };
        let digest = self
            .client
            .retry(RetryMode::Idempotent, on_retry, || async move {
                pacer.ready().await;
                self.transport.put(upload).await
            })
            .await?;
        if let Some(journal) = batch.journal {
            journal.append(file, &digest).await?;
        }
        self.emit(&UploadEvent::FileFinished {
            path: file.path().to_owned(),
            size: file.size(),
        });
        Ok(())
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

/// The files of one plan being sent to one session.
struct Batch<'a> {
    session: &'a SessionId,
    plan: &'a UploadPlan,
    journal: Option<&'a JournalWriter>,
    pacer: Pacer,
}

#[cfg(test)]
mod tests;
