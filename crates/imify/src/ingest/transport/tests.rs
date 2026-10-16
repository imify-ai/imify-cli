use std::sync::Mutex;

use serde_json::json;
use tokio::time::sleep;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::*;
use crate::ingest::UploadPlan;
use crate::ingest::journal::hex;

const LIMIT: Duration = Duration::from_millis(100);

/// Takes `chunks` chunks with `gap` between them, then waits `response_after` for the
/// response, like an HTTP stack sending a body.
async fn send_body(
    activity: &Activity,
    chunks: u32,
    gap: Duration,
    response_after: Duration,
) -> Result<(), Error> {
    for _ in 0..chunks {
        sleep(gap).await;
        activity.touch();
    }
    sleep(response_after).await;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn body_that_keeps_moving_is_not_cut() {
    let activity = Activity::new();

    let result = watch(
        LIMIT,
        &activity,
        send_body(&activity, 25, Duration::from_millis(40), Duration::ZERO),
    )
    .await;

    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test(start_paused = true)]
async fn body_that_stalls_fails_with_retryable_network_error() {
    let activity = Activity::new();

    let error = watch(
        LIMIT,
        &activity,
        send_body(&activity, 2, Duration::from_millis(150), Duration::ZERO),
    )
    .await
    .unwrap_err();

    assert!(crate::retry::should_retry(
        &crate::RetryPolicy::default(),
        crate::retry::RetryMode::Idempotent,
        &error,
        0
    ));
}

#[tokio::test(start_paused = true)]
async fn missing_response_after_the_last_chunk_fails_with_network_error() {
    let activity = Activity::new();

    let error = watch(
        LIMIT,
        &activity,
        send_body(
            &activity,
            3,
            Duration::from_millis(10),
            Duration::from_millis(150),
        ),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, Error::Network { .. }));
}

const STORED_PATH: &str = "data/a";

/// A dataset directory whose only file besides `meta/info.json` is `data/a`.
fn dataset(content: &[u8]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (path, content) in [("meta/info.json", &b"{}"[..]), (STORED_PATH, content)] {
        let path = root.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    root
}

async fn put_data_file(server: &MockServer, root: &std::path::Path) -> Result<Digest, Error> {
    let plan = UploadPlan::from_dir(root).unwrap();
    let file = plan
        .files()
        .iter()
        .find(|file| file.path() == STORED_PATH)
        .unwrap();
    let client = Client::builder(&server.uri()).build().unwrap();
    let upload = FileUpload {
        session: &SessionId::new("3f2c9a"),
        file,
        local_path: root.join(STORED_PATH),
        progress: Arc::new(|_| {}),
    };
    StreamingPut::new(client).put(&upload).await
}

fn stored(size: usize) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "path": STORED_PATH, "size": size }))
}

#[tokio::test]
async fn put_returns_the_sha256_of_the_bytes_it_sent() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(stored(10))
        .mount(&server)
        .await;
    let root = dataset(b"0123456789");

    let digest = put_data_file(&server, root.path()).await.unwrap();

    assert_eq!(
        hex(&digest),
        "84d89877f0d4041efb6bf91a16f0248f2fd573e6af05c19f96bedb9f882f7882"
    );
}

/// Fails the first request and rewrites the file with other bytes of the same size
/// before the next attempt reads it.
struct FailOnceAndRewrite {
    file: std::path::PathBuf,
    calls: Mutex<u32>,
}

impl Respond for FailOnceAndRewrite {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        if *calls == 1 {
            std::fs::write(&self.file, b"abcdefghij").unwrap();
            ResponseTemplate::new(500)
        } else {
            stored(10)
        }
    }
}

#[tokio::test]
async fn put_after_a_failed_attempt_returns_the_hash_of_the_stored_attempt() {
    let server = MockServer::start().await;
    let root = dataset(b"0123456789");
    Mock::given(method("PUT"))
        .respond_with(FailOnceAndRewrite {
            file: root.path().join(STORED_PATH),
            calls: Mutex::new(0),
        })
        .mount(&server)
        .await;
    let failed = put_data_file(&server, root.path()).await;

    let digest = put_data_file(&server, root.path()).await.unwrap();

    assert_eq!(
        (failed.is_err(), hex(&digest)),
        (
            true,
            "72399361da6a7754fec986dca5b7cbaf1c810a28ded4abaf56b2106d06cb78b0".to_owned()
        )
    );
}
