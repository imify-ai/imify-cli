//! A fake recordings API for upload tests. It creates the recording `3f2c9a`, answers
//! `files:batchCreate` from the files a [`Storage`] holds, and completes the recording.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

pub(crate) const RECORDING: &str = "3f2c9a";
pub(crate) const RECORDINGS: &str = "/api/v1/recordings";
pub(crate) const RECORDING_PATH: &str = "/api/v1/recordings/3f2c9a";
pub(crate) const BATCH_CREATE: &str = "/api/v1/recordings/3f2c9a/files:batchCreate";
pub(crate) const COMPLETE: &str = "/api/v1/recordings/3f2c9a:complete";

/// The query of every upload URL: a credential that no error or event may show.
pub(crate) const CREDENTIAL: &str = "X-Amz-Credential=AKIA-secret";

/// The files storage holds, by path, with the SHA-256 they were announced with.
#[derive(Debug, Clone, Default)]
pub(crate) struct Storage {
    files: Arc<Mutex<HashMap<String, String>>>,
    announced: Arc<Mutex<HashMap<String, String>>>,
}

impl Storage {
    /// Records that storage holds `path`.
    pub(crate) fn store(&self, path: &str) {
        let sha256 = Self::lock(&self.announced)
            .get(path)
            .cloned()
            .unwrap_or_default();
        Self::lock(&self.files).insert(path.to_owned(), sha256);
    }

    /// Drops `path`, as storage that lost the file.
    pub(crate) fn forget(&self, path: &str) {
        Self::lock(&self.files).remove(path);
    }

    pub(crate) fn holds(&self, path: &str) -> bool {
        Self::lock(&self.files).contains_key(path)
    }

    /// The SHA-256 each path was last announced with.
    pub(crate) fn announced(&self) -> HashMap<String, String> {
        Self::lock(&self.announced).clone()
    }

    fn lock(
        map: &Mutex<HashMap<String, String>>,
    ) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        map.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A recording in `state` as the API sends it.
pub(crate) fn recording(state: &str) -> Value {
    let complete = state != "UPLOADING";
    json!({
        "name": format!("recordings/{RECORDING}"),
        "displayName": "pick-place",
        "robot": "",
        "operator": "user-1234",
        "startTime": null,
        "site": "",
        "task": "Pick up the red cube",
        "labels": {},
        "state": state,
        "format": if complete { "v3" } else { "" },
        "robotType": if complete { "so100" } else { "" },
        "fps": if complete { 30 } else { 0 },
        "episodeCount": if complete { 2 } else { 0 },
        "dataset": if complete { json!("datasets/d-41") } else { Value::Null },
        "createTime": "2026-11-18T09:30:00Z",
        "updateTime": "2026-11-18T09:30:00Z",
        "etag": "W/\"1\""
    })
}

/// Answers `files:batchCreate`: `UPLOADED` for a file storage holds, `PENDING` with an
/// upload URL below `upload_base` otherwise. Remembers the announced SHA-256.
struct Announce {
    storage: Storage,
    upload_base: String,
}

impl Respond for Announce {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let files: Vec<Value> = body["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                let file = &entry["file"];
                let file_path = file["path"].as_str().unwrap();
                Storage::lock(&self.storage.announced).insert(
                    file_path.to_owned(),
                    file["sha256"].as_str().unwrap().to_owned(),
                );
                let mut answer = json!({
                    "name": format!("recordings/{RECORDING}/files/f-{file_path}"),
                    "path": file_path,
                    "sizeBytes": file["sizeBytes"],
                    "sha256": file["sha256"],
                    "createTime": "2026-11-18T09:30:00Z",
                });
                if self.storage.holds(file_path) {
                    answer["state"] = json!("UPLOADED");
                } else {
                    answer["state"] = json!("PENDING");
                    answer["uploadUri"] =
                        json!(format!("{}/{file_path}?{CREDENTIAL}", self.upload_base));
                    answer["uploadHeaders"] = json!({"x-amz-checksum-sha256": "c2hhMjU2"});
                    answer["uploadUriExpireTime"] = json!("2026-11-18T10:30:00Z");
                }
                answer
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "files": files }))
    }
}

/// Stores the file of a `PUT` below `/storage/`.
struct Store {
    storage: Storage,
}

impl Respond for Store {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let file_path = request.url.path().strip_prefix("/storage/").unwrap();
        self.storage.store(file_path);
        ResponseTemplate::new(200)
    }
}

/// Mounts the recordings API on `server`: create and get answer `UPLOADING` until the
/// recording is complete, `:complete` answers `READY`. Upload URLs point to
/// `upload_base`.
pub(crate) async fn mount(server: &MockServer, storage: &Storage, upload_base: &str) {
    Mock::given(method("POST"))
        .and(path(RECORDINGS))
        .respond_with(ResponseTemplate::new(201).set_body_json(recording("UPLOADING")))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(RECORDING_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(recording("UPLOADING")))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(BATCH_CREATE))
        .respond_with(Announce {
            storage: storage.clone(),
            upload_base: upload_base.to_owned(),
        })
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(COMPLETE))
        .respond_with(ResponseTemplate::new(200).set_body_json(recording("READY")))
        .mount(server)
        .await;
}

/// Mounts the recordings API with storage on the same `server`, below `/storage/`.
pub(crate) async fn mount_with_storage(server: &MockServer, storage: &Storage) {
    mount(server, storage, &format!("{}/storage", server.uri())).await;
    Mock::given(method("PUT"))
        .and(path_regex("^/storage/"))
        .respond_with(Store {
            storage: storage.clone(),
        })
        .mount(server)
        .await;
}

/// An AIP-193 error body with the `reason` of an `ErrorInfo` detail.
pub(crate) fn api_error(code: u16, status: &str, reason: &str) -> ResponseTemplate {
    ResponseTemplate::new(code).set_body_json(json!({"error": {
        "code": code,
        "status": status,
        "message": format!("{reason} answer"),
        "details": [{
            "@type": "type.googleapis.com/google.rpc.ErrorInfo",
            "reason": reason,
            "domain": "imify-cloud"
        }]
    }}))
}

/// The requests to `wanted` that `server` received.
pub(crate) async fn requests_to(server: &MockServer, wanted: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == wanted)
        .collect()
}
