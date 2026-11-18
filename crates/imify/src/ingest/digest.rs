//! The SHA-256 of the files of a plan: the API takes it with each file before the upload.

use std::collections::HashMap;
use std::path::Path;

use futures_util::stream::{self, StreamExt, TryStreamExt};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt;

use super::plan::{PlannedFile, UploadPlan};
use super::transport::{Digest, open_planned};
use crate::Error;

/// Bytes read at once while hashing a file.
const HASH_CHUNK_SIZE: usize = 256 * 1024;

/// Files hashed at once.
const PARALLEL_FILES: usize = 4;

/// Returns the SHA-256 of every file of `plan`. A file in `known` is not read again.
///
/// Fails with [`Error::Io`] when a file cannot be read or changed size since planning.
pub(crate) async fn all(
    plan: &UploadPlan,
    known: &HashMap<String, Digest>,
) -> Result<HashMap<String, Digest>, Error> {
    stream::iter(plan.files())
        .map(|file| async move {
            let digest = match known.get(file.path()) {
                Some(digest) => *digest,
                None => file_sha256(file, &plan.root().join(file.path())).await?,
            };
            Ok::<_, Error>((file.path().to_owned(), digest))
        })
        .buffer_unordered(PARALLEL_FILES)
        .try_collect()
        .await
}

/// Returns the SHA-256 of the planned `file` at `path`, read up to its planned size.
async fn file_sha256(planned: &PlannedFile, path: &Path) -> Result<Digest, Error> {
    let io_error = |source| Error::Io {
        path: path.to_owned(),
        source,
    };
    let mut file = open_planned(planned, path).await?.take(planned.size());
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; HASH_CHUNK_SIZE];
    let mut read_total = 0_u64;
    loop {
        let read = file.read(&mut buffer).await.map_err(io_error)?;
        if read == 0 {
            break;
        }
        read_total += read as u64;
        hasher.update(&buffer[..read]);
    }
    if read_total != planned.size() {
        return Err(io_error(planned.changed_size()));
    }
    Ok(hasher.finalize().into())
}

#[cfg(test)]
mod tests;
