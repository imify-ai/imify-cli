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
