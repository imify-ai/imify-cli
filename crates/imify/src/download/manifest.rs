//! The manifest of a download: its request, its wire format and the checks of its entries.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;

use reqwest::{Method, Url};
use serde::Deserialize;

use super::DownloadSource;
use crate::client::is_loopback;
use crate::retry::RetryMode;
use crate::{Client, DownloadError, Error};

const TRAINING_RUNS_PATH: &str = "api/v1/training-runs";

#[derive(Deserialize)]
struct WireManifest {
    files: Vec<WireEntry>,
    total_size_bytes: u64,
}

#[derive(Deserialize)]
struct WireEntry {
    relative_path: String,
    url: String,
    size: u64,
}

/// A manifest whose entries passed every check.
#[derive(Debug, Clone)]
pub(crate) struct Manifest {
    entries: Vec<Entry>,
}

/// One file of a [`Manifest`].
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub(crate) path: String,
    pub(crate) url: StorageUrl,
    pub(crate) size: u64,
}

/// A presigned storage URL. Its `Debug` output names scheme and host only.
#[derive(Clone)]
pub(crate) struct StorageUrl(Url);

impl StorageUrl {
    pub(crate) fn as_url(&self) -> &Url {
        &self.0
    }
}

impl fmt::Debug for StorageUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "StorageUrl({})", origin(&self.0))
    }
}

impl Manifest {
    pub(crate) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The presigned URL of every path.
    pub(crate) fn urls(&self) -> HashMap<String, StorageUrl> {
        self.entries
            .iter()
            .map(|entry| (entry.path.clone(), entry.url.clone()))
            .collect()
    }

    /// Returns a description of the first difference when `other` lists other paths or
    /// sizes.
    pub(crate) fn same_files(&self, other: &Manifest) -> Result<(), String> {
        let sizes = |manifest: &Manifest| -> HashMap<String, u64> {
            manifest
                .entries
                .iter()
                .map(|entry| (entry.path.clone(), entry.size))
                .collect()
        };
        let (before, after) = (sizes(self), sizes(other));
        if let Some((path, size)) = before
            .iter()
            .find(|(path, size)| after.get(*path) != Some(size))
        {
            return Err(match after.get(path) {
                Some(new_size) => format!("{path} has {new_size} bytes instead of {size}"),
                None => format!("{path} is no longer listed"),
            });
        }
        if let Some(path) = after.keys().find(|path| !before.contains_key(*path)) {
            return Err(format!("{path} is listed in addition"));
        }
        Ok(())
    }
}

/// Fetches the manifest of `source` and checks it.
///
/// Returns [`Error::Download`] with [`DownloadError::UnsafePath`] or
/// [`DownloadError::UnsafeUrl`] for a refused entry, and [`Error::UnexpectedResponse`]
/// when `total_size_bytes` is not the sum of the sizes.
pub(crate) async fn fetch(client: &Client, source: &DownloadSource) -> Result<Manifest, Error> {
    let wire: WireManifest = client
        .execute_at(
            RetryMode::Idempotent,
            Method::GET,
            manifest_url(client, source)?,
            None,
        )
        .await?;
    check(wire)
}

fn manifest_url(client: &Client, source: &DownloadSource) -> Result<Url, Error> {
    let DownloadSource::Checkpoint { policy, checkpoint } = source;
    let mut url = client.url(TRAINING_RUNS_PATH)?;
    url.path_segments_mut()
        .map_err(|()| Error::Config {
            message: "base URL cannot have a path".to_owned(),
        })?
        .pop_if_empty()
        .extend([policy.as_str(), "checkpoints", checkpoint.as_str()]);
    Ok(url)
}

fn check(wire: WireManifest) -> Result<Manifest, Error> {
    let total = wire
        .files
        .iter()
        .try_fold(0_u64, |sum, entry| sum.checked_add(entry.size));
    if total != Some(wire.total_size_bytes) {
        return Err(Error::UnexpectedResponse {
            status: 200,
            message: format!(
                "the manifest lists total_size_bytes {} for files of another total size",
                wire.total_size_bytes
            ),
        });
    }
    let entries = wire
        .files
        .into_iter()
        .map(|entry| {
            check_path(&entry.relative_path)
                .and_then(|()| check_url(&entry.url))
                .map(|url| Entry {
                    url,
                    size: entry.size,
                    path: entry.relative_path.clone(),
                })
                .map_err(|reason| refused(&entry.relative_path, reason))
        })
        .collect::<Result<Vec<_>, _>>()?;
    check_collisions(&entries)?;
    Ok(Manifest { entries })
}

fn refused(relative_path: &str, reason: DownloadError) -> Error {
    Error::Download {
        path: PathBuf::from(relative_path),
        reason,
    }
}

fn check_path(path: &str) -> Result<(), DownloadError> {
    let unsafe_path = |why: &str| Err(DownloadError::UnsafePath(format!("{path:?} {why}")));
    if path.is_empty() {
        return unsafe_path("is empty");
    }
    if path.starts_with('/') {
        return unsafe_path("is absolute");
    }
    if path.contains(['\\', '\0']) {
        return unsafe_path("contains a backslash or NUL");
    }
    if path
        .split('/')
        .any(|component| matches!(component, "" | "." | ".."))
    {
        return unsafe_path("has an empty, . or .. component");
    }
    Ok(())
}

fn check_url(raw: &str) -> Result<StorageUrl, DownloadError> {
    let url =
        Url::parse(raw).map_err(|error| DownloadError::UnsafeUrl(format!("not a URL: {error}")))?;
    match url.scheme() {
        "https" => Ok(StorageUrl(url)),
        "http" if is_loopback(&url) => Ok(StorageUrl(url)),
        _ => Err(DownloadError::UnsafeUrl(format!(
            "{} must use https unless its host is localhost, 127.0.0.1 or ::1",
            origin(&url)
        ))),
    }
}

/// Scheme and host of `url`, without path or query.
fn origin(url: &Url) -> String {
    format!("{}://{}", url.scheme(), url.host_str().unwrap_or(""))
}

/// Refuses entries that would write to the same file on a case-insensitive file system,
/// a file that is also a directory of another entry, and a file that is the `.part` file
/// of another entry.
fn check_collisions(entries: &[Entry]) -> Result<(), Error> {
    let mut files = HashSet::with_capacity(entries.len());
    let mut directories = HashSet::new();
    for entry in entries {
        let folded = entry.path.to_lowercase();
        for (end, _) in folded.match_indices('/') {
            directories.insert(folded[..end].to_owned());
        }
        if !files.insert(folded) {
            return Err(collision(&entry.path, "is listed twice, ignoring case"));
        }
    }
    for entry in entries {
        let folded = entry.path.to_lowercase();
        if directories.contains(&folded) {
            return Err(collision(
                &entry.path,
                "is also a directory of another entry",
            ));
        }
        let part = format!("{folded}.part");
        if files.contains(&part) || directories.contains(&part) {
            return Err(collision(
                &entry.path,
                "collides with the .part file of another entry",
            ));
        }
    }
    Ok(())
}

fn collision(path: &str, why: &str) -> Error {
    refused(path, DownloadError::UnsafePath(format!("{path:?} {why}")))
}

#[cfg(test)]
mod tests;
