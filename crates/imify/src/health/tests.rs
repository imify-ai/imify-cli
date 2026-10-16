use std::time::{Duration, UNIX_EPOCH};

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::RetryPolicy;
use crate::auth::StaticToken;

const TOKEN: &str = "tok_7f3a9c-secret";

fn client(base_url: &str) -> Client {
    Client::builder(base_url)
        .token_source(StaticToken::new(TOKEN))
        .retry_policy(RetryPolicy::none())
        .build()
        .unwrap()
}

async fn api_answering(route: &str, response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

fn ok_health() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"status": "ok"}))
}

fn readiness_body(status: &str, storage: &str) -> serde_json::Value {
    json!({"status": status, "db": "ok", "storage": storage})
}

#[tokio::test]
async fn health_sends_no_authorization_header_when_the_client_has_a_token_source() {
    let server = api_answering("/api/health", ok_health()).await;

    client(&server.uri()).health().await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn health_returns_the_status_and_the_server_time_from_the_date_header() {
    let response = ok_health().insert_header("date", "Fri, 16 Oct 2026 12:00:00 GMT");
    let server = api_answering("/api/health", response).await;

    let health = client(&server.uri()).health().await.unwrap();

    assert_eq!(health.status, "ok");
    assert_eq!(
        health.server_time,
        Some(UNIX_EPOCH + Duration::from_secs(1_792_152_000))
    );
}

#[tokio::test]
async fn health_returns_no_server_time_for_an_unparsable_date_header() {
    let response = ok_health().insert_header("date", "yesterday");
    let server = api_answering("/api/health", response).await;

    let health = client(&server.uri()).health().await.unwrap();

    assert_eq!(health.server_time, None);
}

#[test]
fn server_time_is_none_without_a_date_header() {
    assert_eq!(server_time(&HeaderMap::new()), None);
}

#[tokio::test]
async fn health_on_404_fails_with_not_found() {
    let server = api_answering("/api/health", ResponseTemplate::new(404)).await;

    let error = client(&server.uri()).health().await.unwrap_err();

    assert!(matches!(error, Error::NotFound { .. }), "{error:?}");
}

#[tokio::test]
async fn readiness_returns_the_states_and_ignores_unknown_fields() {
    let mut body = readiness_body("ok", "ok");
    body["version"] = json!("1.4.0");
    let server = api_answering(
        "/api/health/ready",
        ResponseTemplate::new(200).set_body_json(body),
    )
    .await;

    let readiness = client(&server.uri()).readiness().await.unwrap();

    assert_eq!(
        (readiness.status, readiness.db, readiness.storage),
        ("ok".to_owned(), "ok".to_owned(), "ok".to_owned())
    );
}

#[tokio::test]
async fn readiness_sends_no_authorization_header() {
    let server = api_answering(
        "/api/health/ready",
        ResponseTemplate::new(200).set_body_json(readiness_body("ok", "ok")),
    )
    .await;

    client(&server.uri()).readiness().await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert!(!requests[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn readiness_returns_the_body_of_a_503_with_a_readiness_document() {
    let server = api_answering(
        "/api/health/ready",
        ResponseTemplate::new(503).set_body_json(readiness_body("degraded", "unreachable")),
    )
    .await;

    let readiness = client(&server.uri()).readiness().await.unwrap();

    assert_eq!(
        (readiness.status, readiness.storage),
        ("degraded".to_owned(), "unreachable".to_owned())
    );
}

#[tokio::test]
async fn readiness_on_a_503_without_a_readiness_document_fails_with_server() {
    let server = api_answering(
        "/api/health/ready",
        ResponseTemplate::new(503).set_body_json(json!({"detail": "Service Unavailable"})),
    )
    .await;

    let error = client(&server.uri()).readiness().await.unwrap_err();

    assert!(
        matches!(error, Error::Server { status: 503, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn readiness_on_500_fails_with_server() {
    let server = api_answering("/api/health/ready", ResponseTemplate::new(500)).await;

    let error = client(&server.uri()).readiness().await.unwrap_err();

    assert!(
        matches!(error, Error::Server { status: 500, .. }),
        "{error:?}"
    );
}
