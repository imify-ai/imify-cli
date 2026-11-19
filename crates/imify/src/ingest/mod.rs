//! Uploads datasets into recordings.
//!
//! ```no_run
//! # async fn run() -> Result<(), imify::Error> {
//! use imify::Client;
//! use imify::auth::StaticToken;
//! use imify::ingest::{NewRecording, UploadEvent, UploadPlan, Uploader};
//!
//! let client = Client::builder("https://api.example.com")
//!     .token_source(StaticToken::new("<access token>"))
//!     .build()?;
//! let plan = UploadPlan::from_dir("datasets/pick-place")?;
//! let recording = Uploader::new(client)
//!     .on_event(|event| {
//!         if let UploadEvent::FileFinished { path, size } = event {
//!             println!("{path}: {size} bytes");
//!         }
//!     })
//!     .upload(&plan, &NewRecording::new("pick-place").task("Pick up the red cube"))
//!     .await?;
//! println!("recording {} is {}", recording.id, recording.state);
//! # Ok(()) }
//! ```
//!
//! An upload computes the SHA-256 of every file, creates a recording, announces the files
//! with their sizes and SHA-256, sends each file with one `PUT` straight to storage at the
//! presigned URL the API returns, and completes the recording. File bytes never pass
//! through the API, and requests to storage carry no `Authorization` header. Each file is
//! streamed from disk, so memory use does not grow with file size. Request starts of all
//! files are spaced by [`UploadOptions::request_interval`]. A rate-limited API request is
//! reported in [`UploadEvent::RateLimited`] and sent again after the pause.
//!
//! # Resume
//!
//! ```no_run
//! # async fn run(client: imify::Client) -> Result<(), imify::Error> {
//! use imify::ingest::{NewRecording, ResumePolicy, UploadPlan, Uploader};
//!
//! let plan = UploadPlan::from_dir("datasets/pick-place")?;
//! let recording = Uploader::new(client)
//!     .journal("/home/robot/.local/state/imify/uploads", ResumePolicy::IfJournaled)
//!     .upload(&plan, &NewRecording::new("pick-place"))
//!     .await?;
//! # Ok(()) }
//! ```
//!
//! With [`Uploader::journal`], [`Uploader::upload`] records the recording and each
//! uploaded file in a journal file, one per API base URL and dataset directory. Running
//! the same upload again continues the journaled recording: the API reports which files
//! storage has, and only the others are sent. The journal is deleted when the recording
//! is complete.

use std::num::NonZeroUsize;
use std::time::Duration;

mod digest;
mod engine;
#[cfg(test)]
mod fake_api;
mod journal;
pub(crate) mod pace;
mod plan;
mod recording;
mod transport;

pub use engine::Uploader;
pub use plan::{PlannedFile, SkipReason, SkippedEntry, UploadPlan};
pub use recording::{NewRecording, Recording, RecordingId, RecordingState, RequestId};

const DEFAULT_CONCURRENCY: NonZeroUsize = match NonZeroUsize::new(4) {
    Some(value) => value,
    None => NonZeroUsize::MIN,
};

/// Settings of an [`Uploader`].
///
/// # Examples
///
/// ```
/// use std::num::NonZeroUsize;
/// use std::time::Duration;
/// use imify::ingest::UploadOptions;
///
/// let mut options = UploadOptions::default();
/// options.concurrency = NonZeroUsize::new(2).unwrap();
/// options.request_interval = Duration::ZERO;
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UploadOptions {
    /// Files in flight at once. Default: 4.
    pub concurrency: NonZeroUsize,
    /// Minimum time between the starts of two file requests, retries included.
    /// Default: 100 ms. `Duration::ZERO` disables spacing.
    pub request_interval: Duration,
}

impl Default for UploadOptions {
    fn default() -> Self {
        Self {
            concurrency: DEFAULT_CONCURRENCY,
            request_interval: Duration::from_millis(100),
        }
    }
}

/// What [`Uploader::upload`] does with the journal set by [`Uploader::journal`].
///
/// # Examples
///
/// ```
/// use imify::ingest::ResumePolicy;
///
/// fn policy(resume: bool, restart: bool) -> ResumePolicy {
///     match (resume, restart) {
///         (true, _) => ResumePolicy::Required,
///         (_, true) => ResumePolicy::Restart,
///         _ => ResumePolicy::IfJournaled,
///     }
/// }
///
/// assert_eq!(policy(false, false), ResumePolicy::default());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ResumePolicy {
    /// Continue the journaled recording when a journal exists, otherwise start a new
    /// one.
    #[default]
    IfJournaled,
    /// Continue the journaled recording. Fail with
    /// [`JournalError::Missing`](crate::JournalError::Missing) when no journal exists.
    Required,
    /// Delete an existing journal and start a new recording. The journaled recording
    /// stays open on the server.
    Restart,
}

/// Progress of an upload, passed to the handler set with [`Uploader::on_event`].
///
/// Events of one file arrive in order: `FileStarted`, any number of `FileProgress` and
/// `FileRetrying`, then `FileFinished` when storage has the file.
///
/// # Examples
///
/// ```
/// use imify::ingest::UploadEvent;
///
/// fn describe(event: &UploadEvent) -> Option<String> {
///     match event {
///         UploadEvent::FileProgress { path, bytes_sent } => Some(format!("{path}: {bytes_sent}")),
///         UploadEvent::RateLimited { pause } => Some(format!("paused for {pause:?}")),
///         _ => None,
///     }
/// }
///
/// assert_eq!(
///     describe(&UploadEvent::FileProgress { path: "meta/info.json".into(), bytes_sent: 512 }),
///     Some("meta/info.json: 512".to_owned())
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UploadEvent {
    /// The recording exists. It stays open when the upload fails later.
    RecordingCreated {
        /// Id of the recording.
        recording: RecordingId,
    },
    /// The upload continues a journaled recording. Sent instead of `RecordingCreated`.
    RecordingResumed {
        /// Id of the recording.
        recording: RecordingId,
        /// Files of the plan that storage has and that are not sent again. Every file
        /// when the upload of the recording is done.
        files_done: usize,
        /// Total size of those files in bytes.
        bytes_done: u64,
    },
    /// A file transfer starts.
    FileStarted {
        /// Path relative to the dataset directory.
        path: String,
        /// Size in bytes.
        size: u64,
    },
    /// Part of a file was handed to the network.
    FileProgress {
        /// Path relative to the dataset directory.
        path: String,
        /// Bytes sent by the current request so far.
        bytes_sent: u64,
    },
    /// A file is sent again from its first byte after a failure.
    FileRetrying {
        /// Path relative to the dataset directory.
        path: String,
        /// Number of the retry, starting at 1.
        attempt: u32,
        /// Description of the failure.
        reason: String,
    },
    /// Storage has a file.
    FileFinished {
        /// Path relative to the dataset directory.
        path: String,
        /// Size in bytes.
        size: u64,
    },
    /// The API rate-limited a request. It is sent again after `pause`.
    RateLimited {
        /// How long the request waits.
        pause: Duration,
    },
    /// The recording is being completed.
    Completing,
}

#[cfg(test)]
mod tests;
