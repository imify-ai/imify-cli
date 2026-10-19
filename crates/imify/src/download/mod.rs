//! Downloads the files of a trained policy or a checkpoint into a directory.
//!
//! ```no_run
//! # async fn run() -> Result<(), imify::Error> {
//! use imify::Client;
//! use imify::auth::StaticToken;
//! use imify::download::{DownloadEvent, DownloadSource, Downloader};
//!
//! let client = Client::builder("https://api.example.com")
//!     .token_source(StaticToken::new("<access token>"))
//!     .build()?;
//! let summary = Downloader::new(client)
//!     .on_event(|event| {
//!         if let DownloadEvent::FileFinished { path, size } = event {
//!             println!("{path}: {size} bytes");
//!         }
//!     })
//!     .download(&DownloadSource::policy("8d41c7"), "policies/pick-place")
//!     .await?;
//! println!("{} files in {}", summary.files, summary.target.display());
//! # Ok(()) }
//! ```
//!
//! A download fetches the manifest of the source from the API: one presigned storage URL,
//! relative path and size per file. It checks every entry and the target directory before
//! it writes anything, then fetches the files with bounded concurrency. Each file is
//! written to `<path>.part`, checked against the manifest size, synced and renamed to
//! `<path>`. Storage requests carry no `Authorization` header.
//!
//! # Target directory
//!
//! The target directory is created with its parents when it does not exist. It may be
//! a symbolic link. Below it, the download follows no symbolic link and refuses to start
//! when the directory holds an entry it does not own: a file not in the manifest, a
//! manifest file with another size, a symbolic link, or anything other than a regular
//! file or directory. Files of the manifest that exist with the manifest size are not
//! fetched again, so running the same download again completes an interrupted one.
//! Existing `.part` files are replaced from their first byte.
//!
//! # Failures
//!
//! A file is fetched again from its first byte after a network error, a server error or
//! a rate limit, as the client's [`RetryPolicy`](crate::RetryPolicy) allows. When storage
//! rejects a presigned URL with 403, the manifest is fetched again once and the file
//! continues from its new URL. The first file that fails stops the download; files
//! already renamed into place stay.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

mod engine;
mod fetch;
mod manifest;
mod target;

pub use engine::Downloader;

const DEFAULT_CONCURRENCY: NonZeroUsize = match NonZeroUsize::new(4) {
    Some(value) => value,
    None => NonZeroUsize::MIN,
};

/// What a [`Downloader`] downloads.
///
/// # Examples
///
/// ```
/// use imify::download::DownloadSource;
///
/// let policy = DownloadSource::policy("8d41c7");
/// let checkpoint = DownloadSource::checkpoint("8d41c7", "step-20000");
/// assert_ne!(policy, checkpoint);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DownloadSource {
    /// The final policy of a training run: `GET /api/v1/training-runs/{policy}/artifact`.
    Policy {
        /// Id of the training run.
        policy: String,
    },
    /// One checkpoint of a training run:
    /// `GET /api/v1/training-runs/{policy}/checkpoints/{checkpoint}`.
    Checkpoint {
        /// Id of the training run.
        policy: String,
        /// Id of the checkpoint.
        checkpoint: String,
    },
}

impl DownloadSource {
    /// The final policy of the training run `policy`.
    ///
    /// # Examples
    ///
    /// ```
    /// let source = imify::download::DownloadSource::policy("8d41c7");
    /// ```
    pub fn policy(policy: impl Into<String>) -> Self {
        Self::Policy {
            policy: policy.into(),
        }
    }

    /// The checkpoint `checkpoint` of the training run `policy`.
    ///
    /// # Examples
    ///
    /// ```
    /// let source = imify::download::DownloadSource::checkpoint("8d41c7", "step-20000");
    /// ```
    pub fn checkpoint(policy: impl Into<String>, checkpoint: impl Into<String>) -> Self {
        Self::Checkpoint {
            policy: policy.into(),
            checkpoint: checkpoint.into(),
        }
    }
}

/// Settings of a [`Downloader`].
///
/// # Examples
///
/// ```
/// use std::num::NonZeroUsize;
/// use imify::download::DownloadOptions;
///
/// let mut options = DownloadOptions::default();
/// options.concurrency = NonZeroUsize::new(8).unwrap();
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DownloadOptions {
    /// Files in flight at once. Default: 4.
    pub concurrency: NonZeroUsize,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            concurrency: DEFAULT_CONCURRENCY,
        }
    }
}

/// Progress of a download, passed to the handler set with [`Downloader::on_event`].
///
/// Events of one file arrive in order: `FileStarted`, any number of `FileProgress` and
/// `FileRetrying`, then `FileFinished`.
///
/// # Examples
///
/// ```
/// use imify::download::DownloadEvent;
///
/// fn describe(event: &DownloadEvent) -> Option<String> {
///     match event {
///         DownloadEvent::FileProgress { path, bytes_received } => {
///             Some(format!("{path}: {bytes_received}"))
///         }
///         DownloadEvent::RateLimited { pause } => Some(format!("paused for {pause:?}")),
///         _ => None,
///     }
/// }
///
/// assert_eq!(
///     describe(&DownloadEvent::FileProgress {
///         path: "model.safetensors".into(),
///         bytes_received: 512,
///     }),
///     Some("model.safetensors: 512".to_owned())
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DownloadEvent {
    /// Files of the manifest that are complete in the target directory and not fetched
    /// again. Sent once, before the first `FileStarted`, when `files_done` is not 0.
    Resumed {
        /// Number of those files.
        files_done: usize,
        /// Total size of those files in bytes.
        bytes_done: u64,
    },
    /// A file transfer starts.
    FileStarted {
        /// Path relative to the target directory.
        path: String,
        /// Size in bytes.
        size: u64,
    },
    /// Part of a file was written to `<path>.part`.
    FileProgress {
        /// Path relative to the target directory.
        path: String,
        /// Bytes of the current request written so far.
        bytes_received: u64,
    },
    /// A file is fetched again from its first byte after a failure.
    FileRetrying {
        /// Path relative to the target directory.
        path: String,
        /// Number of the retry, starting at 1.
        attempt: u32,
        /// Description of the failure.
        reason: String,
    },
    /// A file has the manifest size and is renamed into place.
    FileFinished {
        /// Path relative to the target directory.
        path: String,
        /// Size in bytes.
        size: u64,
    },
    /// Storage rate-limited a request. No request starts during `pause`.
    RateLimited {
        /// How long request starts are held.
        pause: Duration,
    },
    /// Storage rejected a presigned URL with 403, and the manifest was fetched again.
    ManifestRefreshed,
}

/// The result of a completed download.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
/// use imify::download::{DownloadSource, Downloader};
///
/// let summary = Downloader::new(client)
///     .download(&DownloadSource::policy("8d41c7"), "policies/pick-place")
///     .await?;
/// println!(
///     "{} of {} files fetched",
///     summary.files - summary.files_skipped,
///     summary.files
/// );
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DownloadSummary {
    /// The target directory, canonical.
    pub target: PathBuf,
    /// Number of files in the manifest.
    pub files: usize,
    /// Total size of the files in the manifest in bytes.
    pub bytes: u64,
    /// Number of files that were complete before the download and not fetched.
    pub files_skipped: usize,
    /// Total size of the skipped files in bytes.
    pub bytes_skipped: u64,
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
