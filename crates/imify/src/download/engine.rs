//! The download flow: manifest, target scan, transfers.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::{self, StreamExt};
use tokio::sync::Mutex;

use super::fetch::fetch;
use super::manifest::{self, Entry, Manifest, StorageUrl};
use super::target::Target;
use super::{DownloadEvent, DownloadOptions, DownloadSource, DownloadSummary};
use crate::ingest::pace::Pacer;
use crate::retry::RetryMode;
use crate::{Client, DownloadError, Error};

type EventHandler = Arc<dyn Fn(&DownloadEvent) + Send + Sync>;

/// Downloads the files of a policy or a checkpoint into a directory.
///
/// Dropping the future returned by [`Downloader::download`] stops every transfer. The
/// `.part` files stay, and the next download into the same directory continues them.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
/// use imify::download::{DownloadEvent, DownloadSource, Downloader};
///
/// let summary = Downloader::new(client)
///     .on_event(|event| {
///         if let DownloadEvent::FileFinished { path, .. } = event {
///             println!("saved {path}");
///         }
///     })
///     .download(&DownloadSource::checkpoint("8d41c7", "step-20000"), "checkpoints/20000")
///     .await?;
/// println!("{} bytes in {}", summary.bytes, summary.target.display());
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct Downloader {
    client: Client,
    options: DownloadOptions,
    handler: EventHandler,
}

impl fmt::Debug for Downloader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Downloader")
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

/// The state shared by the transfers of one download.
struct Batch<'a> {
    source: &'a DownloadSource,
    manifest: &'a Manifest,
    target: Target,
    pacer: Pacer,
    urls: Mutex<Urls>,
}

/// The presigned URLs of the newest manifest.
struct Urls {
    by_path: HashMap<String, StorageUrl>,
    generation: u64,
}

impl Downloader {
    /// Creates a downloader that sends requests through `client`, with default
    /// [`DownloadOptions`] and no event handler.
    ///
    /// # Examples
    ///
    /// ```
    /// let client = imify::Client::builder("https://api.example.com").build()?;
    /// let downloader = imify::download::Downloader::new(client);
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self {
            client,
            options: DownloadOptions::default(),
            handler: Arc::new(|_| {}),
        }
    }

    /// Replaces the download options.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::num::NonZeroUsize;
    /// use imify::download::{DownloadOptions, Downloader};
    ///
    /// let mut options = DownloadOptions::default();
    /// options.concurrency = NonZeroUsize::new(8).unwrap();
    /// let client = imify::Client::builder("https://api.example.com").build()?;
    /// let downloader = Downloader::new(client).options(options);
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn options(mut self, options: DownloadOptions) -> Self {
        self.options = options;
        self
    }

    /// Sets the function that receives every [`DownloadEvent`]. It is called from the
    /// transfer tasks, possibly from several threads at once, and must return quickly.
    ///
    /// # Examples
    ///
    /// ```
    /// let client = imify::Client::builder("https://api.example.com").build()?;
    /// let downloader = imify::download::Downloader::new(client)
    ///     .on_event(|event| println!("{event:?}"));
    /// # Ok::<(), imify::Error>(())
    /// ```
    #[must_use]
    pub fn on_event(mut self, handler: impl Fn(&DownloadEvent) + Send + Sync + 'static) -> Self {
        self.handler = Arc::new(handler);
        self
    }

    /// Downloads every file of `source` into the directory `target`.
    ///
    /// The manifest and the target directory are checked before any file is written.
    /// Files that exist with the manifest size are skipped and reported in
    /// [`DownloadEvent::Resumed`]. The other files are fetched with
    /// [`DownloadOptions::concurrency`] in flight at once.
    ///
    /// # Errors
    ///
    /// Before any file is written:
    ///
    /// - [`Error::NotFound`] when the training run or checkpoint does not exist or has no
    ///   files yet, and [`Error::Validation`] when the API offers no download for it.
    /// - [`Error::UnexpectedResponse`] when the manifest's `total_size_bytes` is not the
    ///   sum of its file sizes.
    /// - [`Error::Download`] with [`DownloadError::UnsafePath`] or
    ///   [`DownloadError::UnsafeUrl`] for a refused manifest entry, with
    ///   [`DownloadError::Foreign`] for an entry of `target` the download does not own,
    ///   and with [`DownloadError::Io`] when `target` cannot be created or read.
    ///
    /// For the first file that fails, after the retries of the client's
    /// [`RetryPolicy`](crate::RetryPolicy):
    ///
    /// - [`Error::Download`] with [`DownloadError::SizeMismatch`],
    ///   [`DownloadError::UrlExpired`], [`DownloadError::ManifestChanged`],
    ///   [`DownloadError::Storage`], [`DownloadError::Foreign`] or [`DownloadError::Io`].
    /// - [`Error::Network`], [`Error::Server`] or [`Error::RateLimited`].
    ///
    /// No further file starts, the files in flight stop, and files already renamed into
    /// place stay.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(downloader: imify::download::Downloader) -> Result<(), imify::Error> {
    /// use imify::download::DownloadSource;
    ///
    /// let summary = downloader
    ///     .download(&DownloadSource::policy("8d41c7"), "policies/pick-place")
    ///     .await?;
    /// println!("{} files, {} skipped", summary.files, summary.files_skipped);
    /// # Ok(()) }
    /// ```
    pub async fn download(
        &self,
        source: &DownloadSource,
        target: impl AsRef<Path>,
    ) -> Result<DownloadSummary, Error> {
        let manifest = manifest::fetch(&self.client, source).await?;
        let target = Target::open(target.as_ref()).await?;
        let complete = target.scan(&manifest).await?;
        let (skipped, remaining): (Vec<&Entry>, Vec<&Entry>) = manifest
            .entries()
            .iter()
            .partition(|entry| complete.contains(&entry.path));
        let summary = DownloadSummary {
            target: target.root().to_owned(),
            files: manifest.entries().len(),
            bytes: total_size(manifest.entries().iter()),
            files_skipped: skipped.len(),
            bytes_skipped: total_size(skipped.iter().copied()),
        };
        if summary.files_skipped > 0 {
            self.emit(&DownloadEvent::Resumed {
                files_done: summary.files_skipped,
                bytes_done: summary.bytes_skipped,
            });
        }
        let batch = Batch {
            source,
            manifest: &manifest,
            target,
            pacer: Pacer::new(Duration::ZERO),
            urls: Mutex::new(Urls {
                by_path: manifest.urls(),
                generation: 0,
            }),
        };
        // Built before the stream, so the future of `download` is `Send` without a
        // higher-ranked closure in the stream.
        let files: Vec<_> = remaining
            .into_iter()
            .map(|entry| self.download_file(&batch, entry))
            .collect();
        let mut transfers = stream::iter(files).buffer_unordered(self.options.concurrency.get());
        while let Some(result) = transfers.next().await {
            result?;
        }
        Ok(summary)
    }

    /// Fetches one file. A 403 from storage fetches the manifest again once.
    async fn download_file(&self, batch: &Batch<'_>, entry: &Entry) -> Result<(), Error> {
        self.emit(&DownloadEvent::FileStarted {
            path: entry.path.clone(),
            size: entry.size,
        });
        let mut refreshed = false;
        loop {
            let (generation, url) = batch.url(entry).await;
            match self.fetch_with_retries(batch, entry, &url).await {
                Err(Error::Download {
                    reason: DownloadError::UrlExpired,
                    ..
                }) if !refreshed => {
                    self.refresh(batch, generation, entry).await?;
                    refreshed = true;
                }
                result => {
                    result?;
                    break;
                }
            }
        }
        self.emit(&DownloadEvent::FileFinished {
            path: entry.path.clone(),
            size: entry.size,
        });
        Ok(())
    }

    async fn fetch_with_retries(
        &self,
        batch: &Batch<'_>,
        entry: &Entry,
        url: &StorageUrl,
    ) -> Result<(), Error> {
        let http = self.client.download_http()?;
        let pacer = &batch.pacer;
        let on_retry = |attempt, error: &Error, delay| {
            if matches!(error, Error::RateLimited { .. }) {
                pacer.pause(delay);
                self.emit(&DownloadEvent::RateLimited { pause: delay });
            }
            self.emit(&DownloadEvent::FileRetrying {
                path: entry.path.clone(),
                attempt,
                reason: error.to_string(),
            });
        };
        let progress = |bytes_received| {
            self.emit(&DownloadEvent::FileProgress {
                path: entry.path.clone(),
                bytes_received,
            });
        };
        let progress = &progress;
        self.client
            .retry(RetryMode::Idempotent, on_retry, || async move {
                pacer.ready().await;
                fetch(http, url, entry, &batch.target, progress)
                    .await?
                    .finish()
                    .await
            })
            .await
    }

    /// Fetches the manifest again unless another file did so after `seen`, the
    /// generation of the URL that storage rejected.
    async fn refresh(&self, batch: &Batch<'_>, seen: u64, entry: &Entry) -> Result<(), Error> {
        let mut urls = batch.urls.lock().await;
        if urls.generation > seen {
            return Ok(());
        }
        let fresh = manifest::fetch(&self.client, batch.source).await?;
        batch
            .manifest
            .same_files(&fresh)
            .map_err(|difference| Error::Download {
                path: batch.target.path_of(&entry.path),
                reason: DownloadError::ManifestChanged(difference),
            })?;
        urls.by_path = fresh.urls();
        urls.generation += 1;
        self.emit(&DownloadEvent::ManifestRefreshed);
        Ok(())
    }

    fn emit(&self, event: &DownloadEvent) {
        (self.handler)(event);
    }
}

impl Batch<'_> {
    /// The newest presigned URL of `entry` and the generation of its manifest.
    async fn url(&self, entry: &Entry) -> (u64, StorageUrl) {
        let urls = self.urls.lock().await;
        let url = urls.by_path.get(&entry.path).unwrap_or(&entry.url).clone();
        (urls.generation, url)
    }
}

fn total_size<'a>(entries: impl Iterator<Item = &'a Entry>) -> u64 {
    entries.map(|entry| entry.size).sum()
}
