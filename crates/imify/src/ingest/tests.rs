//! The upload flow against a mock API, through the public interface only.

use std::fs;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{header, method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::auth::StaticToken;
use crate::ingest::{
    NewSession, SessionId, SessionStatus, UploadEvent, UploadOptions, UploadPlan, Uploader,
};
use crate::{Client, Error, RetryPolicy};

const TOKEN: &str = "tok_7f3a9c-secret";
const SESSION: &str = "3f2c9a";
const SESSIONS: &str = "/api/v1/ingest/sessions";
const FILES: &str = "/api/v1/ingest/sessions/3f2c9a/files/";
const COMPLETE: &str = "/api/v1/ingest/sessions/3f2c9a/complete";
const INFO: &[u8] = br#"{"codebase_version": "v3.0", "fps": 30}"#;

fn retries(max_retries: u32, max_backoff: Duration) -> RetryPolicy {
    RetryPolicy {
        max_retries,
        initial_backoff: Duration::from_millis(1),
        max_backoff,
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

fn options(concurrency: usize, request_interval: Duration) -> UploadOptions {
    UploadOptions {
        concurrency: NonZeroUsize::new(concurrency).unwrap(),
        request_interval,
    }
}

fn uploader(client: Client) -> Uploader {
    Uploader::new(client).options(options(4, Duration::ZERO))
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

fn session_body(status: &str) -> serde_json::Value {
    json!({
        "id": SESSION,
        "name": "pick-place",
        "dataset_format": null,
        "robot_type": null,
        "fps": null,
        "total_episodes": null,
        "status": status,
        "task_description": "Pick up the red cube",
        "created_at": "2026-10-16T09:30:00Z"
    })
}

fn new_session() -> NewSession {
    NewSession::new("pick-place").task_description("Pick up the red cube")
}

fn session_id() -> SessionId {
    SessionId::new(SESSION)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap();
            decoded.push(u8::from_str_radix(hex, 16).unwrap());
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).unwrap()
}

/// Answers a file `PUT` the way the API does: with the stored path and size.
struct Store;

impl Respond for Store {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let encoded = request.url.path().strip_prefix(FILES).unwrap_or_default();
        ResponseTemplate::new(200).set_body_json(json!({
            "path": percent_decode(encoded),
            "size": request.body.len(),
        }))
    }
}

/// Records when each request arrives, then answers with `inner`.
struct Timed<R> {
    arrivals: Arc<Mutex<Vec<(String, Instant)>>>,
    inner: R,
}

impl<R: Respond> Respond for Timed<R> {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        self.arrivals
            .lock()
            .unwrap()
            .push((request.url.path().to_owned(), Instant::now()));
        self.inner.respond(request)
    }
}

async fn mount_store(server: &MockServer) {
    Mock::given(method("PUT"))
        .and(path_regex("^/api/v1/ingest/sessions/3f2c9a/files/"))
        .respond_with(Store)
        .mount(server)
        .await;
}

async fn mount_sessions(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path(SESSIONS))
        .respond_with(ResponseTemplate::new(201).set_body_json(session_body("uploading")))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(COMPLETE))
        .respond_with(ResponseTemplate::new(200).set_body_json(session_body("ready")))
        .mount(server)
        .await;
}

async fn received(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.unwrap()
}

async fn received_at(server: &MockServer, wanted: &str) -> Vec<Request> {
    received(server)
        .await
        .into_iter()
        .filter(|request| request.url.path() == wanted)
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

fn validation_detail(detail: &str) -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(json!({ "detail": detail }))
}

#[tokio::test]
async fn create_session_sends_name_and_task_description() {
    let server = MockServer::start().await;
    mount_sessions(&server).await;

    let session = uploader(client(&server))
        .create_session(&new_session())
        .await
        .unwrap();

    assert_eq!(
        (
            received(&server).await[0]
                .body_json::<serde_json::Value>()
                .unwrap(),
            session.id,
            session.status
        ),
        (
            json!({"name": "pick-place", "task_description": "Pick up the red cube"}),
            session_id(),
            SessionStatus::Uploading
        )
    );
}

#[tokio::test]
async fn create_session_follows_a_same_origin_307_as_post_with_body_and_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SESSIONS))
        .respond_with(ResponseTemplate::new(307).insert_header("location", "/v2/sessions"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/sessions"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(201).set_body_json(session_body("uploading")))
        .mount(&server)
        .await;

    uploader(client(&server))
        .create_session(&new_session())
        .await
        .unwrap();

    assert_eq!(
        received_at(&server, "/v2/sessions").await[0]
            .body_json::<serde_json::Value>()
            .unwrap(),
        json!({"name": "pick-place", "task_description": "Pick up the red cube"})
    );
}

#[tokio::test]
async fn create_session_307_to_another_port_arrives_without_token() {
    let server = MockServer::start().await;
    let other = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SESSIONS))
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("location", format!("{}/sessions", other.uri()).as_str()),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201).set_body_json(session_body("uploading")))
        .mount(&other)
        .await;

    uploader(client(&server))
        .create_session(&new_session())
        .await
        .unwrap();

    assert!(
        !received(&other).await[0]
            .headers
            .contains_key("authorization")
    );
}

#[tokio::test]
async fn create_session_302_is_not_followed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SESSIONS))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/elsewhere"))
        .mount(&server)
        .await;

    let error = uploader(client(&server))
        .create_session(&new_session())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        Error::UnexpectedResponse { status: 302, ref message } if message.contains("/elsewhere")
    ));
}

#[tokio::test]
async fn file_put_answered_307_is_sent_again_with_the_full_body() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!("{FILES}data/a")))
        .respond_with(ResponseTemplate::new(307).insert_header("location", "/moved/data/a"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/moved/data/a"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"path": "data/a", "size": 5})),
        )
        .mount(&server)
        .await;
    let root = dataset(&[("data/a", b"abcde")]);

    uploader(client(&server))
        .send_files(&session_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap();

    assert_eq!(
        received_at(&server, "/moved/data/a").await[0].body,
        b"abcde"
    );
}

#[tokio::test]
async fn file_put_to_another_origin_after_307_carries_no_token() {
    let server = MockServer::start().await;
    let other = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("location", format!("{}/data/a", other.uri()).as_str()),
        )
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"path": "data/a", "size": 5})),
        )
        .mount(&other)
        .await;
    let root = dataset(&[("data/a", b"abcde")]);

    uploader(client(&server))
        .send_files(&session_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap();

    assert!(
        !received(&other).await[0]
            .headers
            .contains_key("authorization")
    );
}

#[tokio::test]
async fn file_put_goes_to_the_percent_encoded_file_path() {
    let server = MockServer::start().await;
    mount_store(&server).await;
    let root = dataset(&[("data/chunk 000/file-000.parquet", b"rows")]);

    uploader(client(&server))
        .send_files(
            &session_id(),
            &plan_of(&root, &["data/chunk 000/file-000.parquet"]),
        )
        .await
        .unwrap();

    assert_eq!(
        received(&server).await[0].url.path(),
        "/api/v1/ingest/sessions/3f2c9a/files/data/chunk%20000/file-000.parquet"
    );
}

#[tokio::test]
async fn file_put_carries_content_type_and_content_length() {
    let server = MockServer::start().await;
    mount_store(&server).await;
    let root = dataset(&[("data/a", b"abcde")]);

    uploader(client(&server))
        .send_files(&session_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap();

    let request = &received(&server).await[0];
    assert_eq!(
        (
            request.headers["content-type"].to_str().unwrap(),
            request.headers["content-length"].to_str().unwrap(),
            request.headers["authorization"].to_str().unwrap()
        ),
        (
            "application/octet-stream",
            "5",
            format!("Bearer {TOKEN}").as_str()
        )
    );
}

#[tokio::test]
async fn large_file_arrives_byte_identical() {
    let server = MockServer::start().await;
    mount_store(&server).await;
    let content: Vec<u8> = (0..3 * 1024 * 1024_u32)
        .map(|index| u8::try_from(index % 251).unwrap())
        .collect();
    let root = dataset(&[("videos/front.mp4", &content)]);

    uploader(client(&server))
        .send_files(&session_id(), &plan_of(&root, &["videos/front.mp4"]))
        .await
        .unwrap();

    assert_eq!(received(&server).await[0].body, content);
}

#[tokio::test]
async fn put_response_with_a_different_size_returns_unexpected_response() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"path": "data/a", "size": 4})),
        )
        .mount(&server)
        .await;
    let root = dataset(&[("data/a", b"abcde")]);

    let error = uploader(client(&server))
        .send_files(&session_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        Error::UnexpectedResponse { status: 200, .. }
    ));
}

#[tokio::test]
async fn put_answered_503_then_200_is_sent_twice_with_the_full_body() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    mount_store(&server).await;
    let root = dataset(&[("data/a", b"abcde")]);
    let (events, handler) = recorder();

    uploader(client(&server))
        .on_event(handler)
        .send_files(&session_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap();

    let bodies: Vec<Vec<u8>> = received(&server)
        .await
        .into_iter()
        .map(|request| request.body)
        .collect();
    let retries = events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| matches!(event, UploadEvent::FileRetrying { .. }))
        .count();
    assert_eq!(
        (bodies, retries),
        (vec![b"abcde".to_vec(), b"abcde".to_vec()], 1)
    );
}

#[tokio::test]
async fn put_answered_503_on_every_attempt_returns_server_error_after_all_retries() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let root = dataset(&[("data/a", b"abcde")]);

    let error = uploader(client(&server))
        .send_files(&session_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap_err();

    assert_eq!(
        (
            matches!(error, Error::Server { status: 503, .. }),
            received(&server).await.len()
        ),
        (true, 4)
    );
}

#[tokio::test]
async fn put_answered_400_returns_validation_and_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(validation_detail("Session is not accepting uploads"))
        .mount(&server)
        .await;
    let root = dataset(&[("data/a", b"abcde")]);

    let error = uploader(client(&server))
        .send_files(&session_id(), &plan_of(&root, &["data/a"]))
        .await
        .unwrap_err();

    assert_eq!(
        (
            matches!(error, Error::Validation { ref message, .. } if message == "Session is not accepting uploads"),
            received(&server).await.len()
        ),
        (true, 1)
    );
}

#[tokio::test]
async fn info_file_answered_400_sends_no_other_file() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!("{FILES}meta/info.json")))
        .respond_with(validation_detail("Unsupported dataset format"))
        .mount(&server)
        .await;
    mount_store(&server).await;
    let root = dataset(&[("data/a", b"abcde"), ("data/b", b"fghij")]);

    let result = uploader(client(&server))
        .send_files(&session_id(), &UploadPlan::from_dir(root.path()).unwrap())
        .await;

    assert_eq!((result.is_err(), received(&server).await.len()), (true, 1));
}

#[tokio::test]
async fn rate_limit_on_one_file_delays_the_start_of_other_files() {
    let server = MockServer::start().await;
    let arrivals = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("PUT"))
        .and(path(format!("{FILES}data/a")))
        .respond_with(Timed {
            arrivals: Arc::clone(&arrivals),
            inner: ResponseTemplate::new(429).insert_header("retry-after", "1"),
        })
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(Timed {
            arrivals: Arc::clone(&arrivals),
            inner: Store,
        })
        .mount(&server)
        .await;
    let root = dataset(&[("data/a", b"abcde"), ("data/b", b"fghij")]);
    let (events, handler) = recorder();

    Uploader::new(client_with(
        &server.uri(),
        retries(3, Duration::from_secs(2)),
    ))
    .options(options(2, Duration::from_millis(200)))
    .on_event(handler)
    .send_files(&session_id(), &UploadPlan::from_dir(root.path()).unwrap())
    .await
    .unwrap();

    let arrivals = arrivals.lock().unwrap().clone();
    let first_arrival = |wanted: &str| {
        arrivals
            .iter()
            .find(|(path, _)| path.ends_with(wanted))
            .unwrap()
            .1
    };
    let gap = first_arrival("data/b").duration_since(first_arrival("data/a"));
    assert!(
        events.lock().unwrap().contains(&UploadEvent::RateLimited {
            pause: Duration::from_secs(1)
        }) && gap >= Duration::from_millis(900),
        "{gap:?}"
    );
}

#[tokio::test]
async fn file_removed_after_planning_returns_io_without_a_request() {
    let server = MockServer::start().await;
    mount_store(&server).await;
    let root = dataset(&[("data/a", b"abcde")]);
    let plan = plan_of(&root, &["data/a"]);
    fs::remove_file(root.path().join("data/a")).unwrap();

    let error = uploader(client(&server))
        .send_files(&session_id(), &plan)
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
    let server = MockServer::start().await;
    mount_store(&server).await;
    let root = dataset(&[("data/a", b"abcde")]);
    let plan = plan_of(&root, &["data/a"]);
    write(root.path(), "data/a", b"abcdefgh");

    let error = uploader(client(&server))
        .send_files(&session_id(), &plan)
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Io { .. }));
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
    mount_sessions(&server).await;

    let session = uploader(client(&server))
        .complete(&session_id())
        .await
        .unwrap();

    assert_eq!(
        (session.status, received(&server).await.len()),
        (SessionStatus::Ready, 2)
    );
}

#[tokio::test]
async fn complete_answered_400_returns_validation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(COMPLETE))
        .respond_with(validation_detail("meta/episodes is missing"))
        .mount(&server)
        .await;

    let error = uploader(client(&server))
        .complete(&session_id())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        Error::Validation { ref message, .. } if message == "meta/episodes is missing"
    ));
}

#[tokio::test]
async fn upload_creates_sends_info_then_files_then_completes() {
    let server = MockServer::start().await;
    mount_sessions(&server).await;
    mount_store(&server).await;
    let root = dataset(&[("data/a", b"abcde"), ("videos/b.mp4", b"fghij")]);

    let session = uploader(client(&server))
        .upload(&UploadPlan::from_dir(root.path()).unwrap(), &new_session())
        .await
        .unwrap();

    let mut order: Vec<String> = received(&server)
        .await
        .into_iter()
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect();
    order[2..4].sort();
    assert_eq!(
        (session.status, order),
        (
            SessionStatus::Ready,
            vec![
                format!("POST {SESSIONS}"),
                format!("PUT {FILES}meta/info.json"),
                format!("PUT {FILES}data/a"),
                format!("PUT {FILES}videos/b.mp4"),
                format!("POST {COMPLETE}"),
            ]
        )
    );
}
