//! The upload flow: session, files, completion.

use std::fmt;
use std::sync::Arc;

use futures_util::stream::{self, StreamExt};
use reqwest::Method;

use super::pace::Pacer;
use super::plan::{INFO_FILE, PlannedFile, UploadPlan};
use super::session::{NewSession, Session, SessionId, WireSession, session_url, sessions_url};
use super::transport::{FileUpload, ProgressFn, StreamingPut, Transport};
use super::{UploadEvent, UploadOptions};
use crate::retry::RetryMode;
use crate::{Client, Error};

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
}

impl fmt::Debug for Uploader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Uploader")
            .field("options", &self.options)
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

    /// Creates an ingest session, sends every file of `plan` and completes the session.
    ///
    /// # Errors
    ///
    /// Returns the first error of [`Uploader::create_session`], [`Uploader::send_files`]
    /// or [`Uploader::complete`]. The session stays open when a file fails; its id is
    /// reported in [`UploadEvent::SessionCreated`].
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
        let created = self.create_session(session).await?;
        self.send_files(&created.id, plan).await?;
        self.complete(&created.id).await
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
        let pacer = Pacer::new(self.options.request_interval);
        let rest = match plan.files().split_first() {
            Some((info, rest)) if info.path() == INFO_FILE => {
                self.send_file(session, plan, info, &pacer).await?;
                rest
            }
            _ => plan.files(),
        };
        let mut transfers = stream::iter(rest)
            .map(|file| self.send_file(session, plan, file, &pacer))
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

    async fn send_file(
        &self,
        session: &SessionId,
        plan: &UploadPlan,
        file: &PlannedFile,
        pacer: &Pacer,
    ) -> Result<(), Error> {
        self.emit(&UploadEvent::FileStarted {
            path: file.path().to_owned(),
            size: file.size(),
        });
        let upload = FileUpload {
            session,
            file,
            local_path: plan.root().join(file.path()),
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
        self.client
            .retry(RetryMode::Idempotent, on_retry, || async move {
                pacer.ready().await;
                self.transport.put(upload).await
            })
            .await?;
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

#[cfg(test)]
mod tests;
