use std::fmt::Write as _;

use tokio::time::sleep;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::auth::StaticToken;
use crate::ingest::UploadPlan;

const LIMIT: Duration = Duration::from_millis(100);

/// The query of a presigned URL: a temporary credential that no error may show.
const CREDENTIAL: &str = "X-Amz-Credential=AKIA-secret&X-Amz-Signature=5f3c";

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
const CHECKSUM: &str = "hNiYd/DUBB77a/kaFvAkjy/Vc+avBcGfltvbn4gveII=";

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

/// The upload target of `data/a` on `server`, with a credential in the query.
fn target(server: &MockServer) -> UploadTarget {
    UploadTarget {
        uri: Url::parse(&format!(
            "{}/bucket/{STORED_PATH}?{CREDENTIAL}",
            server.uri()
        ))
        .unwrap(),
        headers: vec![("x-amz-checksum-sha256".into(), CHECKSUM.into())],
    }
}

async fn put_data_file(server: &MockServer, root: &std::path::Path) -> Result<(), Error> {
    let plan = UploadPlan::from_dir(root).unwrap();
    put_planned(server, &plan).await
}

async fn put_planned(server: &MockServer, plan: &UploadPlan) -> Result<(), Error> {
    let root = plan.root();
    let file = plan
        .files()
        .iter()
        .find(|file| file.path() == STORED_PATH)
        .unwrap();
    let client = Client::builder(&server.uri())
        .token_source(StaticToken::new("tok_7f3a9c-secret"))
        .build()
        .unwrap();
    let target = target(server);
    let upload = FileUpload {
        target: &target,
        file,
        local_path: root.join(STORED_PATH),
        progress: Arc::new(|_| {}),
    };
    StreamingPut::new(client).put(&upload).await
}

/// A storage error body in the S3 XML shape.
fn s3_error(status: u16, code: &str) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .insert_header("content-type", "application/xml")
        .set_body_string(format!(
            "<?xml version=\"1.0\"?><Error><Code>{code}</Code><Message>m</Message></Error>"
        ))
}

async fn storage_answering(response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn put_sends_the_bytes_to_the_presigned_url_with_its_headers() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!("/bucket/{STORED_PATH}")))
        .and(header("x-amz-checksum-sha256", CHECKSUM))
        .and(header("content-length", "10"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let root = dataset(b"0123456789");

    put_data_file(&server, root.path()).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        (
            requests[0].body.as_slice(),
            requests[0].url.query(),
            requests[0].headers.get("transfer-encoding"),
        ),
        (&b"0123456789"[..], Some(CREDENTIAL), None)
    );
}

#[tokio::test]
async fn put_sends_no_authorization_header_and_no_content_type() {
    let server = storage_answering(ResponseTemplate::new(200)).await;
    let root = dataset(b"0123456789");

    put_data_file(&server, root.path()).await.unwrap();

    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        (
            request.headers.get("authorization"),
            request.headers.get("content-type")
        ),
        (None, None)
    );
}

#[tokio::test]
async fn storage_403_fails_with_url_expired() {
    let server = storage_answering(s3_error(403, "AccessDenied")).await;
    let root = dataset(b"0123456789");

    let error = put_data_file(&server, root.path()).await.unwrap_err();

    assert!(
        matches!(&error, Error::Upload { path, reason: UploadError::UrlExpired } if path == STORED_PATH),
        "{error:?}"
    );
}

#[tokio::test]
async fn storage_bad_digest_fails_with_checksum_mismatch() {
    let server = storage_answering(s3_error(400, "BadDigest")).await;
    let root = dataset(b"0123456789");

    let error = put_data_file(&server, root.path()).await.unwrap_err();

    assert!(
        matches!(
            error,
            Error::Upload {
                reason: UploadError::ChecksumMismatch,
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn another_storage_400_fails_with_its_code() {
    let server = storage_answering(s3_error(400, "MissingContentLength")).await;
    let root = dataset(b"0123456789");

    let error = put_data_file(&server, root.path()).await.unwrap_err();

    assert!(
        matches!(&error, Error::Upload { reason: UploadError::Storage { status: 400, code: Some(code) }, .. }
            if code == "MissingContentLength"),
        "{error:?}"
    );
}

#[tokio::test]
async fn storage_slow_down_and_429_fail_with_a_retryable_server_error() {
    for response in [s3_error(503, "SlowDown"), ResponseTemplate::new(429)] {
        let server = storage_answering(response).await;
        let root = dataset(b"0123456789");

        let error = put_data_file(&server, root.path()).await.unwrap_err();

        assert!(
            matches!(error, Error::Server { .. })
                && crate::retry::should_retry(
                    &crate::RetryPolicy::default(),
                    crate::retry::RetryMode::Idempotent,
                    &error,
                    0
                ),
            "{error:?}"
        );
    }
}

#[tokio::test]
async fn no_error_shows_the_credential_of_the_presigned_url() {
    let refused = storage_answering(s3_error(400, "InvalidRequest")).await;
    let unreachable = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        address
    };
    let root = dataset(b"0123456789");
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let file = plan
        .files()
        .iter()
        .find(|file| file.path() == STORED_PATH)
        .unwrap();
    let client = Client::builder(&refused.uri()).build().unwrap();
    let offline = UploadTarget {
        uri: Url::parse(&format!("http://{unreachable}/bucket/a?{CREDENTIAL}")).unwrap(),
        headers: Vec::new(),
    };
    let upload = FileUpload {
        target: &offline,
        file,
        local_path: root.path().join(file.path()),
        progress: Arc::new(|_| {}),
    };

    let errors = [
        put_data_file(&refused, root.path()).await.unwrap_err(),
        StreamingPut::new(client).put(&upload).await.unwrap_err(),
    ];

    for error in errors {
        let mut shown = format!("{error} {error:?}");
        let mut source = std::error::Error::source(&error);
        while let Some(cause) = source {
            let _ = write!(shown, " {cause} {cause:?}");
            source = cause.source();
        }
        assert!(!shown.contains("AKIA-secret"), "{shown}");
    }
}

#[test]
fn target_debug_names_the_origin_only() {
    let target = UploadTarget {
        uri: Url::parse(&format!(
            "https://storage.example.com/bucket/a?{CREDENTIAL}"
        ))
        .unwrap(),
        headers: Vec::new(),
    };

    let debug = format!("{target:?}");

    assert!(
        debug.contains("https://storage.example.com") && !debug.contains("AKIA-secret"),
        "{debug}"
    );
}

#[test]
fn s3_code_reads_the_first_code_element() {
    assert_eq!(
        (
            s3_code("<Error><Code>BadDigest</Code></Error>").as_deref(),
            s3_code("not xml"),
        ),
        (Some("BadDigest"), None)
    );
}

#[tokio::test]
async fn put_does_not_follow_a_redirect_to_plain_http_on_another_host() {
    let server = storage_answering(
        ResponseTemplate::new(307).insert_header("location", "http://upload.example.com/a"),
    )
    .await;
    let root = dataset(b"0123456789");

    let error = put_data_file(&server, root.path()).await.unwrap_err();

    assert!(
        matches!(
            error,
            Error::Upload {
                reason: UploadError::Storage { status: 307, .. },
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn put_of_a_file_replaced_by_a_symbolic_link_after_planning_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let root = dataset(b"0123456789");
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), b"abcdefghij").unwrap();
    std::fs::remove_file(root.path().join(STORED_PATH)).unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret"), root.path().join(STORED_PATH))
        .unwrap();

    let error = put_planned(&server, &plan).await.unwrap_err();

    assert!(matches!(error, Error::Io { .. }), "{error:?}");
}

#[tokio::test]
async fn put_of_a_file_replaced_by_a_fifo_after_planning_fails_without_blocking() {
    let server = MockServer::start().await;
    let root = dataset(b"0123456789");
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    std::fs::remove_file(root.path().join(STORED_PATH)).unwrap();
    crate::local_file::make_fifo(&root.path().join(STORED_PATH));

    let error = put_planned(&server, &plan).await.unwrap_err();

    assert!(matches!(error, Error::Io { .. }), "{error:?}");
}
