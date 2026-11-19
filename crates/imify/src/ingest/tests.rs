//! The upload flow against a mock API and a mock storage, through the public interface
//! only.

use std::fs;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use crate::auth::StaticToken;
use crate::ingest::fake_api::{
    self, BATCH_CREATE, COMPLETE, CREDENTIAL, RECORDING, RECORDINGS, Storage,
};
use crate::ingest::{
    NewRecording, RecordingId, RecordingState, RequestId, UploadEvent, UploadOptions, UploadPlan,
    Uploader,
};
use crate::{Client, Error, RetryPolicy, UploadError};

const TOKEN: &str = "tok_7f3a9c-secret";
const INFO: &[u8] = br#"{"codebase_version": "v3.0", "fps": 30}"#;

fn retries(max_retries: u32, max_backoff: Duration) -> RetryPolicy {
    RetryPolicy {
        max_retries,
        initial_backoff: Duration::from_millis(1),
        max_backoff,
        rate_limit_pause: Duration::from_millis(1),
    }
}

fn client_with(base_url: &str, policy: RetryPolicy) -> Client {
    Client::builder(base_url)
        .token_source(StaticToken::new(TOKEN))
        .retry_policy(policy)
        .build()
        .unwrap()
}

fn client(server: &MockServer) -> Client {
    client_with(&server.uri(), retries(3, Duration::from_millis(1)))
}

fn options(concurrency: usize) -> UploadOptions {
    UploadOptions {
        concurrency: NonZeroUsize::new(concurrency).unwrap(),
        request_interval: Duration::ZERO,
    }
}

fn uploader(client: Client) -> Uploader {
    Uploader::new(client).options(options(4))
}

fn write(root: &Path, path: &str, content: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// A dataset directory with `meta/info.json` and `files`.
fn dataset(files: &[(&str, &[u8])]) -> TempDir {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "meta/info.json", INFO);
    for (path, content) in files {
        write(root.path(), path, content);
    }
    root
}

/// A plan that contains only `files`.
fn plan_of(root: &TempDir, files: &[&str]) -> UploadPlan {
    let mut plan = UploadPlan::from_dir(root.path()).unwrap();
    plan.retain(|file| files.contains(&file.path()));
    plan
}

fn new_recording() -> NewRecording {
    NewRecording::new("pick-place").task("Pick up the red cube")
}

fn recording_id() -> RecordingId {
    RecordingId::new(RECORDING)
}

/// A mock API with its storage below `/storage/`.
async fn api(storage: &Storage) -> MockServer {
    let server = MockServer::start().await;
    fake_api::mount_with_storage(&server, storage).await;
    server
}

async fn received(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.unwrap()
}

/// The storage `PUT` requests `server` received.
async fn storage_puts(server: &MockServer) -> Vec<Request> {
    received(server)
        .await
        .into_iter()
        .filter(|request| request.url.path().starts_with("/storage/"))
        .collect()
}

fn recorder() -> (
    Arc<Mutex<Vec<UploadEvent>>>,
    impl Fn(&UploadEvent) + Send + Sync,
) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    (events, move |event: &UploadEvent| {
        sink.lock().unwrap().push(event.clone());
    })
}

/// Mounts `response` for the first `times` storage `PUT` requests of `file_path`.
async fn storage_answers(
    server: &MockServer,
    file_path: &str,
    response: ResponseTemplate,
    times: u64,
) {
    Mock::given(method("PUT"))
        .and(path(format!("/storage/{file_path}")))
        .respond_with(response)
        .up_to_n_times(times)
        .with_priority(1)
        .mount(server)
        .await;
}

fn s3_error(status: u16, code: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_string(format!("<Error><Code>{code}</Code></Error>"))
}

#[tokio::test]
async fn create_recording_sends_the_settings_and_the_request_id() {
    let server = api(&Storage::default()).await;
    let request_id = RequestId::new("0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13");

    let recording = uploader(client(&server))
        .create_recording(&new_recording().robot("so100-0042"), &request_id)
        .await
        .unwrap();

    let body: Value = received(&server).await[0].body_json().unwrap();
    assert_eq!(
        (recording.id, recording.state, body),
        (
            recording_id(),
            RecordingState::Uploading,
            json!({
                "requestId": "0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13",
                "displayName": "pick-place",
                "task": "Pick up the red cube",
                "robot": "so100-0042"
            })
        )
    );
}

#[tokio::test]
async fn create_recording_answered_503_is_sent_again_with_the_same_request_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(RECORDINGS))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    fake_api::mount_with_storage(&server, &Storage::default()).await;

    uploader(client(&server))
        .create_recording(&new_recording(), &RequestId::new("r-1"))
        .await
        .unwrap();

    let ids: Vec<Value> = received(&server)
        .await
        .iter()
        .map(|request| request.body_json::<Value>().unwrap()["requestId"].clone())
        .collect();
    assert_eq!(ids, [json!("r-1"), json!("r-1")]);
}

#[tokio::test]
async fn create_recording_follows_a_same_origin_307_as_post_with_body_and_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(RECORDINGS))
        .respond_with(ResponseTemplate::new(307).insert_header("location", "/v2/recordings"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/recordings"))
        .respond_with(ResponseTemplate::new(201).set_body_json(fake_api::recording("UPLOADING")))
        .mount(&server)
        .await;

    uploader(client(&server))
        .create_recording(&new_recording(), &RequestId::new("r-1"))
        .await
        .unwrap();

    let redirected = &received(&server).await[1];
    assert_eq!(
        (
            redirected.headers["authorization"].to_str().unwrap(),
            redirected.body_json::<Value>().unwrap()["displayName"].clone()
        ),
        (format!("Bearer {TOKEN}").as_str(), json!("pick-place"))
    );
}

#[tokio::test]
async fn create_recording_302_is_not_followed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/login"))
        .mount(&server)
        .await;

    let error = uploader(client(&server))
        .create_recording(&new_recording(), &RequestId::new("r-1"))
        .await
        .unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 302, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn upload_creates_announces_puts_to_storage_and_completes() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let root = dataset(&[("data/a", b"abcde"), ("videos/b.mp4", b"fghij")]);

    let recording = uploader(client(&server))
        .upload(
            &UploadPlan::from_dir(root.path()).unwrap(),
            &new_recording(),
        )
        .await
        .unwrap();

    let mut order: Vec<String> = received(&server)
        .await
        .into_iter()
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect();
    order[2..5].sort();
    assert_eq!(
        (recording.state, order),
        (
            RecordingState::Ready,
            vec![
                format!("POST {RECORDINGS}"),
                format!("POST {BATCH_CREATE}"),
                "PUT /storage/data/a".to_owned(),
                "PUT /storage/meta/info.json".to_owned(),
                "PUT /storage/videos/b.mp4".to_owned(),
                format!("POST {COMPLETE}"),
            ]
        )
    );
}

#[tokio::test]
async fn announcement_names_the_recording_size_and_sha256_of_each_file() {
    let server = api(&Storage::default()).await;
    let root = dataset(&[("data/a", b"abcde")]);

    uploader(client(&server))
        .send_files(&recording_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap();

    let body: Value = fake_api::requests_to(&server, BATCH_CREATE).await[0]
        .body_json()
        .unwrap();
    assert_eq!(
        body,
        json!({"requests": [{
            "parent": "recordings/3f2c9a",
            "file": {
                "path": "data/a",
                "sizeBytes": 5,
                "sha256": "36bbe50ed96841d10443bcb670d6554f0a34b761be67ec9c4a8ad2c0c44ca42c"
            }
        }]})
    );
}

#[tokio::test]
async fn more_than_1000_files_are_announced_in_several_calls() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let files: Vec<(String, &[u8])> = (0..1001)
        .map(|index| (format!("data/f-{index:04}"), &b"x"[..]))
        .collect();
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, content)| (path.as_str(), *content))
        .collect();
    let root = dataset(&refs);

    uploader(client(&server))
        .send_files(&recording_id(), &UploadPlan::from_dir(root.path()).unwrap())
        .await
        .unwrap();

    let sizes: Vec<usize> = fake_api::requests_to(&server, BATCH_CREATE)
        .await
        .iter()
        .map(|request| {
            request.body_json::<Value>().unwrap()["requests"]
                .as_array()
                .unwrap()
                .len()
        })
        .collect();
    assert_eq!(sizes, [1000, 2]);
}

#[tokio::test]
async fn storage_puts_carry_the_upload_headers_and_no_token() {
    let server = api(&Storage::default()).await;
    let root = dataset(&[("data/a", b"abcde")]);

    uploader(client(&server))
        .send_files(&recording_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap();

    let put = &storage_puts(&server).await[0];
    assert_eq!(
        (
            put.headers.get("authorization"),
            put.headers["x-amz-checksum-sha256"].to_str().unwrap(),
            put.headers["content-length"].to_str().unwrap(),
            put.url.query(),
        ),
        (None, "c2hhMjU2", "5", Some(CREDENTIAL))
    );
}

#[tokio::test]
async fn large_file_arrives_byte_identical() {
    let server = api(&Storage::default()).await;
    let content: Vec<u8> = (0..3 * 1024 * 1024)
        .map(|index| u8::try_from(index % 251).unwrap())
        .collect();
    let root = dataset(&[("videos/front.mp4", &content)]);

    uploader(client(&server))
        .send_files(&recording_id(), &plan_of(&root, &["videos/front.mp4"]))
        .await
        .unwrap();

    assert_eq!(storage_puts(&server).await[0].body, content);
}

#[tokio::test]
async fn put_answered_503_then_200_is_sent_twice_with_the_full_body() {
    let server = api(&Storage::default()).await;
    storage_answers(&server, "data/a", s3_error(503, "SlowDown"), 1).await;
    let root = dataset(&[("data/a", b"abcde")]);

    uploader(client(&server))
        .send_files(&recording_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap();

    let bodies: Vec<Vec<u8>> = storage_puts(&server)
        .await
        .into_iter()
        .map(|request| request.body)
        .collect();
    assert_eq!(bodies, [b"abcde".to_vec(), b"abcde".to_vec()]);
}

#[tokio::test]
async fn put_answered_403_is_announced_again_and_sent_to_the_fresh_url() {
    let storage = Storage::default();
    let server = api(&storage).await;
    storage_answers(&server, "data/a", s3_error(403, "AccessDenied"), 1).await;
    let root = dataset(&[("data/a", b"abcde")]);

    uploader(client(&server))
        .send_files(&recording_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap();

    assert_eq!(
        (
            fake_api::requests_to(&server, BATCH_CREATE).await.len(),
            storage_puts(&server).await.len(),
            storage.holds("data/a")
        ),
        (2, 2, true)
    );
}

#[tokio::test]
async fn put_answered_403_twice_fails_with_url_expired() {
    let server = api(&Storage::default()).await;
    storage_answers(&server, "data/a", s3_error(403, "AccessDenied"), 2).await;
    let root = dataset(&[("data/a", b"abcde")]);

    let error = uploader(client(&server))
        .send_files(&recording_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            Error::Upload {
                reason: UploadError::UrlExpired,
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn put_answered_bad_digest_twice_fails_with_checksum_mismatch() {
    let server = api(&Storage::default()).await;
    storage_answers(&server, "data/a", s3_error(400, "BadDigest"), 2).await;
    let root = dataset(&[("data/a", b"abcde")]);

    let error = uploader(client(&server))
        .send_files(&recording_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap_err();

    assert_eq!(
        (
            matches!(
                error,
                Error::Upload {
                    reason: UploadError::ChecksumMismatch,
                    ..
                }
            ),
            storage_puts(&server).await.len()
        ),
        (true, 2)
    );
}

#[tokio::test]
async fn events_and_errors_never_show_the_credential_of_an_upload_url() {
    let server = api(&Storage::default()).await;
    storage_answers(&server, "data/a", s3_error(403, "AccessDenied"), 2).await;
    let root = dataset(&[("data/a", b"abcde")]);
    let (events, handler) = recorder();

    let error = uploader(client(&server))
        .on_event(handler)
        .send_files(&recording_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap_err();

    let shown = format!("{error} {error:?} {:?}", events.lock().unwrap());
    assert!(!shown.contains("AKIA-secret"), "{shown}");
}

#[tokio::test]
async fn an_upload_url_on_plain_http_of_another_host_is_refused_before_any_put() {
    let storage = Storage::default();
    let server = MockServer::start().await;
    fake_api::mount(&server, &storage, "http://storage.example.com").await;
    let root = dataset(&[("data/a", b"abcde")]);

    let error = uploader(client(&server))
        .send_files(&recording_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_file_conflict_of_the_announcement_fails_with_its_reason() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(BATCH_CREATE))
        .respond_with(fake_api::api_error(409, "ALREADY_EXISTS", "FILE_CONFLICT"))
        .mount(&server)
        .await;
    let root = dataset(&[("data/a", b"abcde")]);

    let error = uploader(client(&server))
        .send_files(&recording_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap_err();

    assert!(
        matches!(&error, Error::Conflict { reason: Some(reason), .. } if reason == "FILE_CONFLICT"),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_rate_limited_announcement_reports_the_pause_and_is_sent_again() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(BATCH_CREATE))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    fake_api::mount_with_storage(&server, &Storage::default()).await;
    let root = dataset(&[("data/a", b"abcde")]);
    let (events, handler) = recorder();

    Uploader::new(client_with(
        &server.uri(),
        retries(3, Duration::from_secs(2)),
    ))
    .options(options(2))
    .on_event(handler)
    .send_files(&recording_id(), &plan_of(&root, &["data/a"]))
    .await
    .unwrap();

    assert!(events.lock().unwrap().contains(&UploadEvent::RateLimited {
        pause: Duration::from_secs(1)
    }));
}

#[tokio::test]
async fn file_removed_after_planning_returns_io_without_a_request() {
    let server = api(&Storage::default()).await;
    let root = dataset(&[("data/a", b"abcde")]);
    let plan = plan_of(&root, &["data/a"]);
    fs::remove_file(root.path().join("data/a")).unwrap();

    let error = uploader(client(&server))
        .upload(&plan, &new_recording())
        .await
        .unwrap_err();

    assert_eq!(
        (
            matches!(error, Error::Io { .. }),
            received(&server).await.len()
        ),
        (true, 0)
    );
}

#[tokio::test]
async fn file_whose_size_changed_after_planning_returns_io() {
    let server = api(&Storage::default()).await;
    let root = dataset(&[("data/a", b"abcde")]);
    let plan = plan_of(&root, &["data/a"]);
    write(root.path(), "data/a", b"abcdefgh");

    let error = uploader(client(&server))
        .send_files(&recording_id(), &plan)
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Io { .. }), "{error:?}");
}

#[tokio::test]
async fn complete_answered_503_then_200_is_sent_twice_and_returns_ready() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(COMPLETE))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    fake_api::mount_with_storage(&server, &Storage::default()).await;

    let recording = uploader(client(&server))
        .complete(&recording_id())
        .await
        .unwrap();

    assert_eq!(
        (recording.state, received(&server).await.len()),
        (RecordingState::Ready, 2)
    );
}

#[tokio::test]
async fn complete_answered_412_names_the_files_that_are_not_uploaded() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(":complete$"))
        .respond_with(ResponseTemplate::new(412).set_body_json(json!({"error": {
            "code": 412,
            "status": "FAILED_PRECONDITION",
            "message": "1 file is not uploaded",
            "details": [
                {"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "FILES_NOT_UPLOADED"},
                {
                    "@type": "type.googleapis.com/google.rpc.PreconditionFailure",
                    "violations": [{"type": "FILE_NOT_UPLOADED", "subject": "data/a", "description": "not uploaded"}]
                }
            ]
        }})))
        .mount(&server)
        .await;

    let error = uploader(client(&server))
        .complete(&recording_id())
        .await
        .unwrap_err();

    assert!(
        matches!(&error, Error::FailedPrecondition { violations, .. } if violations[0].subject == "data/a"),
        "{error:?}"
    );
}

#[tokio::test]
async fn complete_answered_invalid_dataset_returns_validation_with_the_reason() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(COMPLETE))
        .respond_with(fake_api::api_error(
            400,
            "INVALID_ARGUMENT",
            "INVALID_DATASET",
        ))
        .mount(&server)
        .await;

    let error = uploader(client(&server))
        .complete(&recording_id())
        .await
        .unwrap_err();

    assert!(
        matches!(&error, Error::Validation { reason: Some(reason), .. } if reason == "INVALID_DATASET"),
        "{error:?}"
    );
}
