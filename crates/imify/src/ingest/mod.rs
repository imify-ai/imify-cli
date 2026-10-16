//! Uploads datasets into ingest sessions.
//!
//! ```no_run
//! # async fn run() -> Result<(), imify::Error> {
//! use imify::Client;
//! use imify::auth::StaticToken;
//! use imify::ingest::{NewSession, UploadEvent, UploadPlan, Uploader};
//!
//! let client = Client::builder("https://api.example.com")
//!     .token_source(StaticToken::new("<access token>"))
//!     .build()?;
//! let plan = UploadPlan::from_dir("datasets/pick-place")?;
//! let session = Uploader::new(client)
//!     .on_event(|event| {
//!         if let UploadEvent::FileFinished { path, size } = event {
//!             println!("{path}: {size} bytes");
//!         }
//!     })
//!     .upload(&plan, &NewSession::new("pick-place").task_description("Pick up the red cube"))
//!     .await?;
//! println!("session {} is {:?}", session.id, session.status);
//! # Ok(()) }
//! ```
//!
//! An upload creates a session, sends `meta/info.json`, sends the remaining files with
//! bounded concurrency and completes the session. Each file is streamed from disk, so
//! memory use does not grow with file size. Request starts of all files are spaced by
//! [`UploadOptions::request_interval`]; a rate-limited response holds all starts for the
//! requested delay.

use std::num::NonZeroUsize;
use std::time::Duration;

mod engine;
mod pace;
mod plan;
mod session;
mod transport;

pub use engine::Uploader;
pub use plan::{PlannedFile, UploadPlan};
pub use session::{NewSession, Session, SessionId, SessionStatus};

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

/// Progress of an upload, passed to the handler set with [`Uploader::on_event`].
///
/// Events of one file arrive in order: `FileStarted`, any number of `FileProgress` and
/// `FileRetrying`, then `FileFinished` when the file is stored.
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
    /// The ingest session exists. It stays open when the upload fails later.
    SessionCreated {
        /// Id of the session.
        session: SessionId,
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
    /// The API stored a file.
    FileFinished {
        /// Path relative to the dataset directory.
        path: String,
        /// Size in bytes.
        size: u64,
    },
    /// The API rate-limited a request. No request starts during `pause`.
    RateLimited {
        /// How long request starts are held.
        pause: Duration,
    },
    /// The session is being completed.
    Completing,
}

#[cfg(test)]
mod tests;
