//! Mock API, mock storage and target directories for download tests.

use std::fs;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::{DownloadEvent, DownloadOptions, DownloadSource, Downloader};
use crate::auth::StaticToken;
use crate::{Client, Error, RetryPolicy};

pub(super) const TOKEN: &str = "tok_7f3a9c-secret";
pub(super) const POLICY: &str = "8d41c7";
pub(super) const CHECKPOINT: &str = "step-20000";
pub(super) const MANIFEST: &str = "/api/v1/training-runs/8d41c7/checkpoints/step-20000";
/// The query of every presigned URL in the fixtures.
pub(super) const SIGNATURE: &str = "X-Amz-Signature=sig-0b7e4f-secret";

pub(super) fn source() -> DownloadSource {
    DownloadSource::checkpoint(POLICY, CHECKPOINT)
}

pub(super) fn client_with(server: &MockServer, max_retries: u32) -> Client {
    Client::builder(&server.uri())
        .token_source(StaticToken::new(TOKEN))
        .retry_policy(RetryPolicy {
            max_retries,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(1),
        })
        .build()
        .unwrap()
}

pub(super) fn downloader(server: &MockServer) -> Downloader {
    Downloader::new(client_with(server, 3))
}

pub(super) fn concurrency(value: usize) -> DownloadOptions {
    DownloadOptions {
        concurrency: NonZeroUsize::new(value).unwrap(),
    }
}

/// The presigned URL of `name` below `/storage/` on `base`.
pub(super) fn storage_url(base: &str, name: &str) -> String {
    format!("{base}/storage/{name}?{SIGNATURE}")
}

/// A manifest entry for `relative_path` with `size`, served from `url`.
pub(super) fn entry(relative_path: &str, url: &str, size: u64) -> Value {
    json!({"relative_path": relative_path, "url": url, "size": size})
}

/// A manifest whose `total_size_bytes` is the sum of the entry sizes.
pub(super) fn manifest(entries: &[Value]) -> Value {
    let total: u64 = entries
        .iter()
        .map(|entry| entry["size"].as_u64().unwrap())
        .sum();
    json!({"files": entries, "total_size_bytes": total})
}

/// A manifest of `files`, each served from `/storage/<path>` on `server`.
pub(super) fn manifest_of(server: &MockServer, files: &[(&str, &[u8])]) -> Value {
    let entries: Vec<Value> = files
        .iter()
        .map(|(name, bytes)| entry(name, &storage_url(&server.uri(), name), bytes.len() as u64))
        .collect();
    manifest(&entries)
}

pub(super) async fn mount_manifest(server: &MockServer, body: &Value) {
    Mock::given(method("GET"))
        .and(path(MANIFEST))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// Serves `bytes` at `/storage/<name>`.
pub(super) async fn mount_file(server: &MockServer, name: &str, bytes: &[u8]) {
    mount_storage(
        server,
        name,
        ResponseTemplate::new(200).set_body_bytes(bytes),
    )
    .await;
}

pub(super) async fn mount_storage(server: &MockServer, name: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(format!("/storage/{name}")))
        .respond_with(response)
        .mount(server)
        .await;
}

/// Mounts a manifest of `files` and serves each file.
pub(super) async fn mount_files(server: &MockServer, files: &[(&str, &[u8])]) {
    mount_manifest(server, &manifest_of(server, files)).await;
    for (name, bytes) in files {
        mount_file(server, name, bytes).await;
    }
}

pub(super) async fn requests_to(server: &MockServer, path: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == path)
        .collect()
}

pub(super) async fn storage_requests(server: &MockServer) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path().starts_with("/storage/"))
        .collect()
}

/// Collects every event a downloader reports.
#[derive(Debug, Clone, Default)]
pub(super) struct Events(Arc<Mutex<Vec<DownloadEvent>>>);

impl Events {
    pub(super) fn attach(&self, downloader: Downloader) -> Downloader {
        let events = Arc::clone(&self.0);
        downloader.on_event(move |event| events.lock().unwrap().push(event.clone()))
    }

    pub(super) fn all(&self) -> Vec<DownloadEvent> {
        self.0.lock().unwrap().clone()
    }
}

/// Lists every file below `root`, relative to it, sorted.
pub(super) fn files_below(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect(root, root, &mut files);
    files.sort();
    files
}

fn collect(root: &Path, dir: &Path, files: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.symlink_metadata().unwrap().is_dir() {
            collect(root, &path, files);
        } else {
            let relative = path.strip_prefix(root).unwrap();
            files.push(relative.to_string_lossy().into_owned());
        }
    }
}

pub(super) fn write(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// The `DownloadError` of `error`, failing the test for another error.
pub(super) fn download_reason(error: &Error) -> &crate::DownloadError {
    match error {
        Error::Download { reason, .. } => reason,
        other => panic!("expected a download error, got {other:?}"),
    }
}

/// A storage server that answers the n-th connection with the n-th raw response and
/// closes it. Returns the base URL.
pub(super) async fn raw_storage(responses: Vec<Vec<u8>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        for response in responses {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => request.extend_from_slice(&buffer[..read]),
                }
            }
            let _ = stream.write_all(&response).await;
            let _ = stream.shutdown().await;
        }
    });
    base
}

/// A raw response that announces `bytes` in `Content-Length` and sends the first `sent`.
pub(super) fn raw_response(bytes: &[u8], sent: usize) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    )
    .into_bytes();
    response.extend_from_slice(&bytes[..sent]);
    response
}
