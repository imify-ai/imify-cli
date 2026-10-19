//! One storage `GET` of one file, streamed into its `.part` file.

use std::path::PathBuf;

use reqwest::StatusCode;

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
/// which holds exactly the manifest size on success. The `.part` file is created only
/// after the status and `Content-Length` are accepted. `progress` receives the bytes
/// written so far after each write.
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
    let mut response = http
        .get(url.as_url().clone())
        .send()
        .await
        .map_err(|error| network_error(error.without_url()))?;
    check_status(response.status(), response.headers(), &path)?;
    if let Some(announced) = response.content_length()
        && announced != size
    {
        return Err(size_mismatch(path, size, announced));
    }
    let mut part = target.create_part(&entry.path).await?;
    let mut received: u64 = 0;
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
