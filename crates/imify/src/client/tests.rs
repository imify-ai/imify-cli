use std::future::Future;
use std::net::TcpListener;
use std::pin::Pin;
use std::time::Duration;

use reqwest::Method;
use serde::Deserialize;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;
use crate::auth::{StaticToken, Token, TokenSource};

const TOKEN: &str = "tok_7f3a9c-secret";

#[derive(Debug, Deserialize, PartialEq)]
struct Dataset {
    id: String,
    episodes: u32,
}

fn fast_retries(max_retries: u32) -> RetryPolicy {
    RetryPolicy {
        max_retries,
        initial_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(1),
    }
}

fn client(server: &MockServer) -> Client {
    Client::builder(&server.uri())
        .retry_policy(fast_retries(3))
        .build()
        .unwrap()
}

fn client_with_token(server: &MockServer) -> Client {
    Client::builder(&server.uri())
        .token_source(StaticToken::new(TOKEN))
        .retry_policy(fast_retries(3))
        .build()
        .unwrap()
}

async fn get_dataset(client: &Client) -> Result<Dataset, Error> {
    client.execute(Method::GET, "/v1/datasets/ds_1", None).await
}

fn dataset_body() -> serde_json::Value {
    json!({"id": "ds_1", "episodes": 12})
}

async fn mount_dataset(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/v1/datasets/ds_1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(dataset_body()))
        .mount(server)
        .await;
}

async fn received(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.unwrap()
}

#[derive(Debug)]
struct FailingTokenSource;

impl TokenSource for FailingTokenSource {
    fn token(&self) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + '_>> {
        Box::pin(async {
            Err(Error::Auth {
                status: None,
                message: "not signed in".into(),
            })
        })
    }
}

#[tokio::test]
async fn requests_carry_the_default_user_agent() {
    let server = MockServer::start().await;
    mount_dataset(&server).await;

    get_dataset(&client(&server)).await.unwrap();

    let user_agent = format!("imify-cli/{}", crate::VERSION);
    assert_eq!(
        received(&server).await[0].headers["user-agent"],
        user_agent.as_str()
    );
}

#[tokio::test]
async fn custom_user_agent_replaces_the_default() {
    let server = MockServer::start().await;
    mount_dataset(&server).await;
    let client = Client::builder(&server.uri())
        .user_agent("robot-fleet/2.1")
        .build()
        .unwrap();

    get_dataset(&client).await.unwrap();

    assert_eq!(
        received(&server).await[0].headers["user-agent"],
        "robot-fleet/2.1"
    );
}

#[tokio::test]
async fn token_source_sets_bearer_authorization() {
    let server = MockServer::start().await;
    mount_dataset(&server).await;

    get_dataset(&client_with_token(&server)).await.unwrap();

    assert_eq!(
        received(&server).await[0].headers["authorization"],
        format!("Bearer {TOKEN}").as_str()
    );
}

#[tokio::test]
async fn without_token_source_no_authorization_is_sent() {
    let server = MockServer::start().await;
    mount_dataset(&server).await;

    get_dataset(&client(&server)).await.unwrap();

    assert!(
        !received(&server).await[0]
            .headers
            .contains_key("authorization")
    );
}

#[tokio::test]
async fn failing_token_source_returns_auth_without_sending_a_request() {
    let server = MockServer::start().await;
    mount_dataset(&server).await;
    let client = Client::builder(&server.uri())
        .token_source(FailingTokenSource)
        .build()
        .unwrap();

    let error = get_dataset(&client).await.unwrap_err();

    assert!(matches!(error, Error::Auth { status: None, .. }));
    assert!(received(&server).await.is_empty());
}

#[tokio::test]
async fn get_succeeds_after_two_server_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .with_priority(1)
        .mount(&server)
        .await;
    mount_dataset(&server).await;

    let dataset = get_dataset(&client(&server)).await.unwrap();

    assert_eq!(
        (dataset.id.as_str(), received(&server).await.len()),
        ("ds_1", 3)
    );
}

#[tokio::test]
async fn get_returns_server_error_after_exhausting_retries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let error = get_dataset(&client(&server)).await.unwrap_err();

    assert!(matches!(error, Error::Server { status: 503, .. }));
    assert_eq!(received(&server).await.len(), 4);
}

#[tokio::test]
async fn an_error_body_over_1_mib_is_dropped_and_the_error_carries_the_reason_phrase() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"detail": "x".repeat(1024 * 1024)})),
        )
        .mount(&server)
        .await;

    let error = get_dataset(&client(&server)).await.unwrap_err();

    assert!(
        matches!(error, Error::NotFound { ref message } if message == "Not Found"),
        "{error:?}"
    );
}

#[tokio::test]
async fn get_not_found_is_sent_once() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"detail": "Not Found"})))
        .mount(&server)
        .await;

    let error = get_dataset(&client(&server)).await.unwrap_err();

    assert!(matches!(error, Error::NotFound { .. }));
    assert_eq!(received(&server).await.len(), 1);
}

#[tokio::test]
async fn post_server_error_is_sent_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let error = client(&server)
        .execute::<Dataset>(
            Method::POST,
            "/v1/ingest/sessions",
            Some(&json!({"name": "pick-place"})),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Server { status: 503, .. }));
    assert_eq!(received(&server).await.len(), 1);
}

#[tokio::test]
async fn post_sends_the_json_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/ingest/sessions"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(201).set_body_json(dataset_body()))
        .mount(&server)
        .await;

    client(&server)
        .execute::<Dataset>(
            Method::POST,
            "/v1/ingest/sessions",
            Some(&json!({"name": "pick-place"})),
        )
        .await
        .unwrap();

    assert_eq!(
        received(&server).await[0]
            .body_json::<serde_json::Value>()
            .unwrap(),
        json!({"name": "pick-place"})
    );
}

#[tokio::test]
async fn unreachable_host_returns_network_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let client = Client::builder(&format!("http://{address}"))
        .retry_policy(fast_retries(1))
        .build()
        .unwrap();

    let error = get_dataset(&client).await.unwrap_err();

    assert!(matches!(error, Error::Network { .. }));
}

#[tokio::test]
async fn response_with_unknown_fields_deserializes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "ds_1",
            "episodes": 12,
            "created_by": "usr_9",
            "storage": {"bytes": 1024}
        })))
        .mount(&server)
        .await;

    let dataset = get_dataset(&client(&server)).await.unwrap();

    assert_eq!(
        dataset,
        Dataset {
            id: "ds_1".into(),
            episodes: 12
        }
    );
}

#[tokio::test]
async fn success_body_of_the_wrong_shape_returns_unexpected_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 7})))
        .mount(&server)
        .await;

    let error = get_dataset(&client(&server)).await.unwrap_err();

    assert!(matches!(
        error,
        Error::UnexpectedResponse { status: 200, .. }
    ));
}

#[tokio::test]
async fn base_url_path_prefix_is_kept() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/datasets/ds_1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(dataset_body()))
        .mount(&server)
        .await;
    let client = Client::builder(&format!("{}/api", server.uri()))
        .build()
        .unwrap();

    let dataset = get_dataset(&client).await;

    assert!(dataset.is_ok(), "{dataset:?}");
}

#[test]
fn invalid_base_url_is_a_config_error() {
    let error = Client::builder("not a url").build().unwrap_err();

    assert!(matches!(error, Error::Config { .. }));
}

#[test]
fn non_http_base_url_is_a_config_error() {
    let error = Client::builder("ftp://api.example.com")
        .build()
        .unwrap_err();

    assert!(matches!(error, Error::Config { .. }));
}

#[test]
fn an_http_base_url_on_a_non_loopback_host_with_a_token_source_is_a_config_error() {
    let error = Client::builder("http://api.example.com")
        .token_source(StaticToken::new(TOKEN))
        .build()
        .unwrap_err();

    assert!(
        matches!(error, Error::Config { ref message } if message.contains("https")),
        "{error:?}"
    );
}

#[test]
fn an_http_base_url_on_a_loopback_host_with_a_token_source_is_accepted() {
    for base_url in [
        "http://localhost:8000",
        "http://127.0.0.1:8000",
        "http://[::1]:8000",
    ] {
        let client = Client::builder(base_url)
            .token_source(StaticToken::new(TOKEN))
            .build();

        assert!(client.is_ok(), "{base_url}: {client:?}");
    }
}

#[test]
fn an_http_base_url_without_a_token_source_is_accepted() {
    let client = Client::builder("http://api.example.com").build();

    assert!(client.is_ok(), "{client:?}");
}

#[test]
fn invalid_user_agent_is_a_config_error() {
    let error = Client::builder("https://api.example.com")
        .user_agent("line\nbreak")
        .build()
        .unwrap_err();

    assert!(matches!(error, Error::Config { .. }));
}

#[test]
fn client_debug_does_not_contain_the_token() {
    let client = Client::builder("https://api.example.com")
        .token_source(StaticToken::new(TOKEN))
        .build()
        .unwrap();

    assert!(!format!("{client:?}").contains(TOKEN));
}

#[test]
fn token_debug_does_not_contain_the_token() {
    assert!(!format!("{:?}", Token::new(TOKEN)).contains(TOKEN));
}

#[tokio::test]
async fn errors_do_not_contain_the_token() {
    let server = MockServer::start().await;
    for (status, body) in [
        (401, json!({"detail": "Not authenticated"})),
        (
            422,
            json!({"detail": [{"loc": ["body"], "msg": "Field required", "type": "missing"}]}),
        ),
        (503, json!({"detail": "Service Unavailable"})),
    ] {
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&server)
            .await;

        let error = get_dataset(&client_with_token(&server)).await.unwrap_err();

        assert!(!format!("{error:?} {error}").contains(TOKEN), "{status}");
    }
}

#[tokio::test]
async fn a_success_body_announced_over_32_mib_fails_with_unexpected_response() {
    let base = serve("content-length: 33554433", 0).await;
    let client = Client::builder(&base)
        .retry_policy(RetryPolicy::none())
        .build()
        .unwrap();

    let error = get_dataset(&client).await.unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, ref message }
            if message == "the response body exceeds 33554432 bytes"),
        "{error:?}"
    );
}

/// Serves one `200` response with the header line `header` and a chunked body of `chunks`
/// chunks of 1 MiB, and returns the base URL.
async fn serve(header: &'static str, chunks: usize) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request).await.unwrap();
        let head = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n{header}\r\n\r\n");
        // The client may close the connection once it rejects the response.
        if stream.write_all(head.as_bytes()).await.is_err() || chunks == 0 {
            return;
        }
        let chunk = vec![b' '; 1024 * 1024];
        for _ in 0..chunks {
            let header = format!("{:x}\r\n", chunk.len());
            // The client stops reading once the limit is exceeded.
            if stream.write_all(header.as_bytes()).await.is_err()
                || stream.write_all(&chunk).await.is_err()
                || stream.write_all(b"\r\n").await.is_err()
            {
                return;
            }
        }
        let _ = stream.write_all(b"0\r\n\r\n").await;
    });
    format!("http://{address}")
}

#[tokio::test]
async fn a_streamed_success_body_over_32_mib_fails_with_unexpected_response() {
    let base = serve("transfer-encoding: chunked", 33).await;
    let client = Client::builder(&base)
        .retry_policy(RetryPolicy::none())
        .build()
        .unwrap();

    let error = get_dataset(&client).await.unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
        "{error:?}"
    );
}

#[test]
fn upload_redirect_to_http_on_another_host_is_not_followed_from_a_loopback_base() {
    let previous = Url::parse("http://127.0.0.1:8080/v1/a").unwrap();
    let next = Url::parse("http://upload.example.com/v1/b").unwrap();

    assert!(!may_follow_upload_redirect(
        StatusCode::TEMPORARY_REDIRECT,
        &previous,
        &next
    ));
}

#[test]
fn upload_redirect_to_http_on_a_non_loopback_host_is_not_followed_from_any_base() {
    let next = Url::parse("http://upload.example.com/v1/b").unwrap();
    for previous in [
        "http://upload.example.com/v1/a",
        "http://localhost/v1/a",
        "https://api.example.com/v1/a",
    ] {
        let previous = Url::parse(previous).unwrap();

        assert!(
            !may_follow_upload_redirect(StatusCode::PERMANENT_REDIRECT, &previous, &next),
            "{previous}"
        );
    }
}

#[test]
fn upload_redirect_to_http_on_a_loopback_host_or_to_https_is_followed() {
    let previous = Url::parse("http://localhost:8080/v1/a").unwrap();
    for next in [
        "http://localhost:9000/v1/b",
        "http://127.0.0.1/v1/b",
        "https://upload.example.com/v1/b",
    ] {
        let next = Url::parse(next).unwrap();

        assert!(
            may_follow_upload_redirect(StatusCode::TEMPORARY_REDIRECT, &previous, &next),
            "{next}"
        );
    }
}

#[test]
fn redirect_from_https_to_http_is_not_followed() {
    let previous = Url::parse("https://api.example.com/v1/a").unwrap();
    let next = Url::parse("http://api.example.com/v1/b").unwrap();

    assert!(!may_follow_redirect(
        StatusCode::TEMPORARY_REDIRECT,
        &previous,
        &next
    ));
}

#[tokio::test]
async fn get_follows_at_most_five_redirects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(308).insert_header("location", "/v1/datasets/ds_1"))
        .mount(&server)
        .await;

    let error = get_dataset(&client(&server)).await.unwrap_err();

    assert_eq!(
        (
            matches!(error, Error::UnexpectedResponse { status: 308, .. }),
            received(&server).await.len()
        ),
        (true, 6)
    );
}
