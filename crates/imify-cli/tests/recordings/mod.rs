//! A mock recordings API with its storage, shared by the upload tests. It creates the
//! recording `3f2c9a`, answers `files:batchCreate` from the files storage holds, takes file
//! `PUT` requests below `/storage/` and answers `:complete` as the test asks.
#![allow(dead_code, reason = "each test crate uses a part of the mock")]

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

pub const RECORDING_ID: &str = "3f2c9a";
pub const RECORDINGS: &str = "/api/v1/recordings";
pub const RECORDING: &str = "/api/v1/recordings/3f2c9a";
pub const BATCH_CREATE: &str = "/api/v1/recordings/3f2c9a/files:batchCreate";
pub const COMPLETE: &str = "/api/v1/recordings/3f2c9a:complete";
pub const STORAGE: &str = "/storage/";

/// The paths storage holds. Clones share the set.
#[derive(Debug, Clone, Default)]
pub struct Stored(Arc<Mutex<BTreeSet<String>>>);

impl Stored {
    fn insert(&self, path: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_owned());
    }

    fn holds(&self, path: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(path)
    }
}

/// A recording in `state` as the API sends it, before the upload completes.
pub fn recording_body(state: &str) -> Value {
    json!({
        "name": format!("recordings/{RECORDING_ID}"),
        "displayName": "pick-cube",
        "robot": "",
        "operator": "user-1234",
        "startTime": null,
        "site": "",
        "task": "Pick up the red cube",
        "labels": {},
        "state": state,
        "format": "",
        "robotType": "",
        "fps": 0,
        "episodeCount": 0,
        "dataset": null,
        "createTime": "2026-11-18T09:30:00Z",
        "updateTime": "2026-11-18T09:30:00Z",
        "etag": "W/\"1\""
    })
}

/// The recording as `:complete` answers: `READY`, with format, robot type, fps, episodes
/// and dataset.
pub fn completed_recording_body() -> Value {
    let mut body = recording_body("READY");
    body["format"] = json!("v3");
    body["robotType"] = json!("so100");
    body["fps"] = json!(30);
    body["episodeCount"] = json!(2);
    body["dataset"] = json!("datasets/d-41");
    body
}

pub fn completed() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(completed_recording_body())
}

/// An AIP-193 error with the `reason` of an `ErrorInfo` detail.
pub fn api_error(code: u16, status: &str, reason: &str, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(code).set_body_json(json!({"error": {
        "code": code,
        "status": status,
        "message": message,
        "details": [{
            "@type": "type.googleapis.com/google.rpc.ErrorInfo",
            "reason": reason,
            "domain": "imify-cloud"
        }]
    }}))
}

/// Answers `files:batchCreate`: `UPLOADED` for a file storage holds, `PENDING` with an upload
/// URL below `/storage/` of `base` otherwise.
struct Announce {
    stored: Stored,
    base: String,
}

impl Respond for Announce {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let files: Vec<Value> = body["requests"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|entry| {
                let file = &entry["file"];
                let file_path = file["path"].as_str().unwrap_or_default();
                let mut answer = json!({
                    "name": format!("recordings/{RECORDING_ID}/files/f-{file_path}"),
                    "path": file_path,
                    "sizeBytes": file["sizeBytes"],
                    "sha256": file["sha256"],
                    "createTime": "2026-11-18T09:30:00Z",
                });
                if self.stored.holds(file_path) {
                    answer["state"] = json!("UPLOADED");
                } else {
                    answer["state"] = json!("PENDING");
                    answer["uploadUri"] = json!(format!(
                        "{}{STORAGE}{file_path}?X-Amz-Credential=AKIA",
                        self.base
                    ));
                    answer["uploadHeaders"] = json!({"x-amz-checksum-sha256": "c2hh"});
                    answer["uploadUriExpireTime"] = json!("2026-11-18T10:30:00Z");
                }
                answer
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "files": files }))
    }
}

/// Stores the file of a storage `PUT`, after `delay(path)`.
struct Store {
    stored: Stored,
    delay: fn(&str) -> Duration,
}

impl Respond for Store {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let file_path = request.url.path().strip_prefix(STORAGE).unwrap_or_default();
        let delay = (self.delay)(file_path);
        if delay.is_zero() {
            self.stored.insert(file_path);
        }
        ResponseTemplate::new(200).set_delay(delay)
    }
}

fn no_delay(_: &str) -> Duration {
    Duration::ZERO
}

/// Mounts the recordings API and its storage on `server`. Storage answers a `PUT` after
/// `delay(path)`; a file with a delay is not stored. `:complete` answers `complete`.
pub async fn mount_slow(
    server: &MockServer,
    stored: &Stored,
    complete: ResponseTemplate,
    delay: fn(&str) -> Duration,
) {
    Mock::given(method("POST"))
        .and(path(RECORDINGS))
        .respond_with(ResponseTemplate::new(201).set_body_json(recording_body("UPLOADING")))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(RECORDING))
        .respond_with(ResponseTemplate::new(200).set_body_json(recording_body("UPLOADING")))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(BATCH_CREATE))
        .respond_with(Announce {
            stored: stored.clone(),
            base: server.uri(),
        })
        .mount(server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(format!("^{STORAGE}")))
        .respond_with(Store {
            stored: stored.clone(),
            delay,
        })
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(COMPLETE))
        .respond_with(complete)
        .mount(server)
        .await;
}

/// Mounts the recordings API whose storage stores every file at once.
pub async fn mount(server: &MockServer, stored: &Stored, complete: ResponseTemplate) {
    mount_slow(server, stored, complete, no_delay).await;
}

/// A recordings API whose `:complete` answers `complete`.
pub async fn api_completing_with(complete: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    mount(&server, &Stored::default(), complete).await;
    server
}

/// A recordings API that completes the recording.
pub async fn api() -> MockServer {
    api_completing_with(completed()).await
}

/// The paths of the storage `PUT` requests among `requests`, in request order.
pub fn stored_paths(requests: &[Request]) -> Vec<String> {
    requests
        .iter()
        .filter(|request| request.method.as_str() == "PUT")
        .filter_map(|request| request.url.path().strip_prefix(STORAGE))
        .map(str::to_owned)
        .collect()
}

/// The number of creates among `requests`.
pub fn creations(requests: &[Request]) -> usize {
    requests
        .iter()
        .filter(|request| request.method.as_str() == "POST" && request.url.path() == RECORDINGS)
        .count()
}
