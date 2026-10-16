//! How one file reaches the API.

use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::future::{self, Either};
use futures_util::stream::{self, Stream, StreamExt};
use reqwest::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use reqwest::{Body, Response, Url};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt;
use tokio::time::{Instant, sleep_until};
use tokio_util::bytes::Bytes;
use tokio_util::io::ReaderStream;

use super::plan::PlannedFile;
use super::session::{SessionId, session_url};
use crate::client::{MAX_REDIRECTS, decode, may_follow_redirect, network_error};
use crate::{Client, Error};

/// Bytes read from disk and handed to the HTTP stack at once, per file in flight.
const CHUNK_SIZE: usize = 256 * 1024;

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The SHA-256 of the bytes of a file.
pub(crate) type Digest = [u8; 32];

/// Receives the bytes of the current request taken by the HTTP stack so far.
pub(crate) type ProgressFn = Arc<dyn Fn(u64) + Send + Sync>;

/// One attempt to upload one file of a plan.
pub(crate) struct FileUpload<'a> {
    pub(crate) session: &'a SessionId,
    pub(crate) file: &'a PlannedFile,
    pub(crate) local_path: PathBuf,
    pub(crate) progress: ProgressFn,
}

/// Sends one file to an ingest session.
pub(crate) trait Transport: Send + Sync + fmt::Debug {
    /// Sends the file once, without retries, and returns the SHA-256 of the bytes the
    /// API stored.
    fn put<'a>(&'a self, upload: &'a FileUpload<'a>) -> BoxFuture<'a, Result<Digest, Error>>;
}

/// Sends each file as one streamed `PUT` request to the API.
#[derive(Debug, Clone)]
pub(crate) struct StreamingPut {
    client: Client,
}

#[derive(Deserialize)]
struct StoredFile {
    path: String,
    size: u64,
}

impl StreamingPut {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    async fn put_following_redirects(&self, upload: &FileUpload<'_>) -> Result<Digest, Error> {
        let segments = std::iter::once("files").chain(upload.file.path().split('/'));
        let mut url = session_url(&self.client, upload.session, segments)?;
        let mut redirects = 0;
        loop {
            let (response, tracker) = self.put_once(upload, &url).await?;
            if redirects < MAX_REDIRECTS
                && let Some(next) = redirect_target(&url, &response)
            {
                url = next;
                redirects += 1;
                continue;
            }
            let limit = self.client.read_timeout();
            tokio::time::timeout(limit, check_stored(upload.file, response))
                .await
                .map_err(|_| stalled(limit))??;
            return Ok(tracker.digest());
        }
    }

    async fn put_once(
        &self,
        upload: &FileUpload<'_>,
        url: &Url,
    ) -> Result<(Response, Arc<Tracker>), Error> {
        let size = upload.file.size();
        let tracker = Arc::new(Tracker::new());
        let body = file_body(upload, Arc::clone(&tracker)).await?;
        let mut request = self
            .client
            .upload_http()
            .put(url.clone())
            .header(CONTENT_TYPE, "application/octet-stream")
            .header(CONTENT_LENGTH, size)
            .body(body);
        if self.client.is_same_origin(url)
            && let Some(authorization) = self.client.authorization().await?
        {
            request = request.header(AUTHORIZATION, authorization);
        }
        tracker.activity.touch();
        let sent = watch(self.client.read_timeout(), &tracker.activity, async {
            request.send().await.map_err(network_error)
        })
        .await;
        if tracker.truncated.load(Ordering::Relaxed) {
            return Err(changed_size(upload));
        }
        Ok((sent?, tracker))
    }
}

impl Transport for StreamingPut {
    fn put<'a>(&'a self, upload: &'a FileUpload<'a>) -> BoxFuture<'a, Result<Digest, Error>> {
        Box::pin(self.put_following_redirects(upload))
    }
}

async fn file_body(upload: &FileUpload<'_>, tracker: Arc<Tracker>) -> Result<Body, Error> {
    let file = open_planned(upload.file, &upload.local_path).await?;
    let size = upload.file.size();
    let progress = Arc::clone(&upload.progress);
    let chunks = ReaderStream::with_capacity(file.take(size), CHUNK_SIZE);
    Ok(Body::wrap_stream(tracked(chunks, size, tracker, progress)))
}

/// Reports each chunk the HTTP stack takes and fails the body when the file ends before
/// `size` bytes.
fn tracked(
    chunks: impl Stream<Item = io::Result<Bytes>> + Send + 'static,
    size: u64,
    tracker: Arc<Tracker>,
    progress: ProgressFn,
) -> impl Stream<Item = io::Result<Bytes>> + Send + 'static {
    let end_tracker = Arc::clone(&tracker);
    let chunks = chunks.inspect(move |chunk| {
        if let Ok(bytes) = chunk {
            let sent = tracker.add(bytes);
            progress(sent);
        }
    });
    let end = stream::once(async move {
        (end_tracker.sent.load(Ordering::Relaxed) < size).then(|| {
            end_tracker.truncated.store(true, Ordering::Relaxed);
            Err(io::Error::from(io::ErrorKind::UnexpectedEof))
        })
    })
    .filter_map(future::ready);
    chunks.chain(end)
}

/// Opens `file` at `local_path` on the blocking thread pool, as [`PlannedFile::open`]
/// does.
pub(crate) async fn open_planned(
    file: &PlannedFile,
    local_path: &Path,
) -> Result<tokio::fs::File, Error> {
    let (file, local_path) = (file.clone(), local_path.to_owned());
    let path = local_path.clone();
    match tokio::task::spawn_blocking(move || file.open(&local_path)).await {
        Ok(opened) => opened.map(tokio::fs::File::from_std),
        Err(join_error) => Err(Error::Io {
            path,
            source: io::Error::from(join_error),
        }),
    }
}

fn changed_size(upload: &FileUpload<'_>) -> Error {
    Error::Io {
        path: upload.local_path.clone(),
        source: upload.file.changed_size(),
    }
}

fn redirect_target(current: &Url, response: &Response) -> Option<Url> {
    let location = response.headers().get(LOCATION)?.to_str().ok()?;
    let next = current.join(location).ok()?;
    may_follow_redirect(response.status(), current, &next).then_some(next)
}

async fn check_stored(file: &PlannedFile, response: Response) -> Result<(), Error> {
    let status = response.status().as_u16();
    let stored: StoredFile = decode(response).await?;
    if stored.path == file.path() && stored.size == file.size() {
        Ok(())
    } else {
        Err(Error::UnexpectedResponse {
            status,
            message: format!(
                "API stored {} with {} bytes, expected {} with {} bytes",
                stored.path,
                stored.size,
                file.path(),
                file.size()
            ),
        })
    }
}

/// Upload state shared between the request body and the request.
struct Tracker {
    sent: AtomicU64,
    truncated: AtomicBool,
    activity: Activity,
    hasher: Mutex<Sha256>,
}

impl Tracker {
    fn new() -> Self {
        Self {
            sent: AtomicU64::new(0),
            truncated: AtomicBool::new(false),
            activity: Activity::new(),
            hasher: Mutex::new(Sha256::new()),
        }
    }

    /// Records `bytes` as sent and returns the number of bytes sent in total.
    fn add(&self, bytes: &[u8]) -> u64 {
        self.activity.touch();
        self.hasher
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .update(bytes);
        let count = bytes.len() as u64;
        self.sent.fetch_add(count, Ordering::Relaxed) + count
    }

    /// Returns the SHA-256 of the bytes sent so far.
    fn digest(&self) -> Digest {
        self.hasher
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .finalize()
            .into()
    }
}

/// The time of the last progress of a request.
#[derive(Debug)]
pub(crate) struct Activity {
    origin: Instant,
    last_nanos: AtomicU64,
}

impl Activity {
    pub(crate) fn new() -> Self {
        Self {
            origin: Instant::now(),
            last_nanos: AtomicU64::new(0),
        }
    }

    /// Records progress now.
    pub(crate) fn touch(&self) {
        let nanos = u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.last_nanos.store(nanos, Ordering::Relaxed);
    }

    fn last(&self) -> Instant {
        self.origin + Duration::from_nanos(self.last_nanos.load(Ordering::Relaxed))
    }
}

/// The source of the [`Error::Network`] returned when a request makes no progress.
#[derive(Debug, thiserror::Error)]
#[error("no upload progress for {} s", limit.as_secs_f64())]
pub(crate) struct Stalled {
    limit: Duration,
}

/// Runs `request` and fails it with [`Error::Network`] once `activity` records no
/// progress for `limit`.
pub(crate) async fn watch<T>(
    limit: Duration,
    activity: &Activity,
    request: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    let idle = async {
        loop {
            let deadline = activity.last() + limit;
            if Instant::now() >= deadline {
                return;
            }
            sleep_until(deadline).await;
        }
    };
    match future::select(pin!(request), pin!(idle)).await {
        Either::Left((result, _)) => result,
        Either::Right(((), _)) => Err(stalled(limit)),
    }
}

fn stalled(limit: Duration) -> Error {
    Error::Network {
        source: Box::new(Stalled { limit }),
    }
}

#[cfg(test)]
mod tests;
