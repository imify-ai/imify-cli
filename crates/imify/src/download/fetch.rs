//! One storage `GET` of one file, streamed into its `.part` file.

use std::path::PathBuf;

use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, RANGE};

use super::manifest::{Entry, StorageUrl};
use super::target::{PartFile, Target};
use crate::client::network_error;
use crate::error::retry_after;
use crate::{DownloadError, Error};

/// The source of the [`Error::Network`] returned when a body without `Content-Length`
/// ends before the manifest size.
#[derive(Debug, thiserror::Error)]
#[error("the connection closed after {received} of {expected} bytes")]
struct Truncated {
    received: u64,
    expected: u64,
}

/// Fetches `url` into the `.part` file of `entry` below `target` and returns the file,
/// which holds exactly the manifest size on success. `progress` receives the bytes in
/// the file after each write, and first the bytes continued from an earlier transfer.
///
/// A `.part` file that [`Target::reusable_part`] accepts is continued with a `Range`
/// request. A 206 with the matching `Content-Range` appends to it, a 200 replaces its
/// content. Another `Content-Range` or a 416 removes it, and the file is requested
/// again from its first byte. A new `.part` file is created only after the status and
/// `Content-Length` are accepted.
///
/// Storage statuses map as follows: 403 to [`DownloadError::UrlExpired`], 429 and 503 to
/// [`Error::RateLimited`], other 5xx to [`Error::Server`], any other non-success status to
/// [`DownloadError::Storage`]. No error carries the URL.
pub(crate) async fn fetch(
    http: &reqwest::Client,
    url: &StorageUrl,
    entry: &Entry,
    target: &Target,
    progress: &(dyn Fn(u64) + Send + Sync),
) -> Result<PartFile, Error> {
    let (path, size) = (target.path_of(&entry.path), entry.size);
    let mut reused = target.reusable_part(&entry.path, size).await?;
    let mut response = loop {
        let offset = reused.as_ref().map_or(0, PartFile::len);
        let response = send(http, url, offset).await?;
        match response.status() {
            StatusCode::PARTIAL_CONTENT | StatusCode::RANGE_NOT_SATISFIABLE
                if offset > 0 && !continues(&response, offset, size) =>
            {
                if let Some(part) = reused.take() {
                    part.discard().await?;
                }
            }
            _ => break response,
        }
    };
    check_status(response.status(), response.headers(), &path)?;
    let start = match &reused {
        Some(part) if response.status() == StatusCode::PARTIAL_CONTENT => part.len(),
        _ => 0,
    };
    if let Some(announced) = response.content_length()
        && start + announced != size
    {
        return Err(size_mismatch(path, size, start + announced));
    }
    let mut part = match reused {
        Some(mut part) => {
            if start == 0 {
                part.truncate().await?;
            }
            part
        }
        None => target.create_part(&entry.path).await?,
    };
    if start > 0 {
        progress(start);
    }
    let mut received = start;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| network_error(error.without_url()))?
    {
        received += chunk.len() as u64;
        if received > size {
            return Err(size_mismatch(path, size, received));
        }
        part.write(&chunk).await?;
        progress(received);
    }
    if received < size {
        return Err(Error::Network {
            source: Box::new(Truncated {
                received,
                expected: size,
            }),
        });
    }
    Ok(part)
}

/// Sends the `GET`, with `Range: bytes=<offset>-` when `offset` is not 0.
async fn send(
    http: &reqwest::Client,
    url: &StorageUrl,
    offset: u64,
) -> Result<reqwest::Response, Error> {
    let mut request = http.get(url.as_url().clone());
    if offset > 0 {
        request = request.header(RANGE, format!("bytes={offset}-"));
    }
    request
        .send()
        .await
        .map_err(|error| network_error(error.without_url()))
}

/// Returns `true` when `response` carries `Content-Range: bytes <offset>-<size - 1>/<size>`.
fn continues(response: &reqwest::Response, offset: u64, size: u64) -> bool {
    let expected = format!("bytes {offset}-{}/{size}", size - 1);
    response
        .headers()
        .get(CONTENT_RANGE)
        .is_some_and(|value| value.as_bytes() == expected.as_bytes())
}

fn check_status(
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
    path: &std::path::Path,
) -> Result<(), Error> {
    let failed = |reason| Error::Download {
        path: path.to_owned(),
        reason,
    };
    match status.as_u16() {
        _ if status.is_success() => Ok(()),
        403 => Err(failed(DownloadError::UrlExpired)),
        429 | 503 => Err(Error::RateLimited {
            retry_after: retry_after(headers),
        }),
        code @ 500..=599 => Err(Error::Server {
            status: code,
            message: status
                .canonical_reason()
                .unwrap_or("server error")
                .to_owned(),
        }),
        code => Err(failed(DownloadError::Storage { status: code })),
    }
}

fn size_mismatch(path: PathBuf, expected: u64, received: u64) -> Error {
    Error::Download {
        path,
        reason: DownloadError::SizeMismatch { expected, received },
    }
}
