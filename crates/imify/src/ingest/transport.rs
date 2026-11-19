//! How one file reaches storage: one `PUT` to its presigned URL.

use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use futures_util::future::{self, Either};
use futures_util::stream::{self, Stream, StreamExt};
use reqwest::header::{CONTENT_LENGTH, HeaderName, HeaderValue};
use reqwest::{Body, Response, StatusCode, Url};
use tokio::io::AsyncReadExt;
use tokio::time::{Instant, sleep_until};
use tokio_util::bytes::Bytes;
use tokio_util::io::ReaderStream;

use super::plan::PlannedFile;
use crate::client::{network_error, read_small_body};
use crate::{Client, Error, UploadError};

/// Bytes read from disk and handed to the HTTP stack at once, per file in flight.
const CHUNK_SIZE: usize = 256 * 1024;

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The SHA-256 of the bytes of a file.
pub(crate) type Digest = [u8; 32];

/// Receives the bytes of the current request taken by the HTTP stack so far.
pub(crate) type ProgressFn = Arc<dyn Fn(u64) + Send + Sync>;

/// Where one file goes: the `uploadUri` and `uploadHeaders` of `files:batchCreate`.
/// `Debug` names the origin of the URL only, since the URL carries a credential.
#[derive(Clone)]
pub(crate) struct UploadTarget {
    pub(crate) uri: Url,
    pub(crate) headers: Vec<(String, String)>,
}

impl fmt::Debug for UploadTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UploadTarget")
            .field("origin", &self.uri.origin().ascii_serialization())
            .field("headers", &self.headers)
            .finish()
    }
}

/// One attempt to upload one file of a plan.
pub(crate) struct FileUpload<'a> {
    pub(crate) target: &'a UploadTarget,
    pub(crate) file: &'a PlannedFile,
    pub(crate) local_path: PathBuf,
    pub(crate) progress: ProgressFn,
}

/// Sends one file to storage.
pub(crate) trait Transport: Send + Sync + fmt::Debug {
    /// Sends the file once, without retries.
    ///
    /// Fails with [`Error::Upload`] and [`UploadError::UrlExpired`] when storage answers
    /// 403, [`UploadError::ChecksumMismatch`] for `BadDigest`, and
    /// [`UploadError::Storage`] for another client error. A 429 or 5xx of storage fails
    /// with [`Error::Server`], so the caller retries it.
    fn put<'a>(&'a self, upload: &'a FileUpload<'a>) -> BoxFuture<'a, Result<(), Error>>;
}

/// Sends each file as one streamed `PUT` request to its presigned storage URL. The
/// request carries `Content-Length` and the headers of the target, never an
/// `Authorization` header.
#[derive(Debug, Clone)]
pub(crate) struct StreamingPut {
    client: Client,
}

impl StreamingPut {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    async fn put_to_storage(&self, upload: &FileUpload<'_>) -> Result<(), Error> {
        let response = self.put_once(upload).await?;
        if response.status().is_success() {
            return Ok(());
        }
        let limit = self.client.read_timeout();
        tokio::time::timeout(limit, storage_error(upload.file, response))
            .await
            .map_err(|_| stalled(limit))?
    }

    async fn put_once(&self, upload: &FileUpload<'_>) -> Result<Response, Error> {
        let tracker = Arc::new(Tracker::new());
        let body = file_body(upload, Arc::clone(&tracker)).await?;
        let mut request = self
            .client
            .upload_http()
            .put(upload.target.uri.clone())
            .header(CONTENT_LENGTH, upload.file.size())
            .body(body);
        for (name, value) in &upload.target.headers {
            let (Ok(name), Ok(value)) = (
                HeaderName::try_from(name.as_str()),
                HeaderValue::try_from(value.as_str()),
            ) else {
                return Err(Error::UnexpectedResponse {
                    status: 200,
                    message: format!(
                        "the upload header {name:?} of {} is not a valid HTTP header",
                        upload.file.path()
                    ),
                });
            };
            request = request.header(name, value);
        }
        tracker.activity.touch();
        let sent = watch(self.client.read_timeout(), &tracker.activity, async {
            // The presigned URL carries a credential: the error must not name it.
            request
                .send()
                .await
                .map_err(|error| network_error(error.without_url()))
        })
        .await;
        if tracker.truncated.load(Ordering::Relaxed) {
            return Err(changed_size(upload));
        }
        sent
    }
}

impl Transport for StreamingPut {
    fn put<'a>(&'a self, upload: &'a FileUpload<'a>) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(self.put_to_storage(upload))
    }
}

/// The error for a `PUT` that storage refused. Storage answers in the S3 XML shape; its
/// `Code` tells a mismatching checksum from other client errors.
async fn storage_error(file: &PlannedFile, response: Response) -> Result<(), Error> {
    let status = response.status();
    let body = read_small_body(response).await.unwrap_or_default();
    let code = s3_code(&String::from_utf8_lossy(&body));
    let refused = |reason| Error::Upload {
        path: file.path().to_owned(),
        reason,
    };
    Err(match status {
        StatusCode::FORBIDDEN => refused(UploadError::UrlExpired),
        StatusCode::BAD_REQUEST if code.as_deref() == Some("BadDigest") => {
            refused(UploadError::ChecksumMismatch)
        }
        StatusCode::TOO_MANY_REQUESTS => storage_busy(status, code),
        _ if status.is_server_error() => storage_busy(status, code),
        _ => refused(UploadError::Storage {
            status: status.as_u16(),
            code,
        }),
    })
}

/// The retryable error for a 429 or 5xx of storage, such as `503 SlowDown`.
fn storage_busy(status: StatusCode, code: Option<String>) -> Error {
    Error::Server {
        status: status.as_u16(),
        message: format!(
            "storage answered {}{}",
            status.as_u16(),
            code.map(|code| format!(" {code}")).unwrap_or_default()
        ),
        retry_after: None,
    }
}

/// The text of the first `<Code>` element of an S3 error body.
fn s3_code(body: &str) -> Option<String> {
    let start = body.find("<Code>")? + "<Code>".len();
    let end = start + body[start..].find("</Code>")?;
    let code = body[start..end].trim();
    (!code.is_empty() && code.len() <= 64).then(|| code.to_owned())
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

/// Upload state shared between the request body and the request.
struct Tracker {
    sent: AtomicU64,
    truncated: AtomicBool,
    activity: Activity,
}

impl Tracker {
    fn new() -> Self {
        Self {
            sent: AtomicU64::new(0),
            truncated: AtomicBool::new(false),
            activity: Activity::new(),
        }
    }

    /// Records `bytes` as sent and returns the number of bytes sent in total.
    fn add(&self, bytes: &[u8]) -> u64 {
        self.activity.touch();
        let count = bytes.len() as u64;
        self.sent.fetch_add(count, Ordering::Relaxed) + count
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
