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

async fn mount_config(server: &MockServer, route: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(response)
        .mount(server)
        .await;
}

fn settings_body() -> serde_json::Value {
    json!({"authority": "https://login.example.com/pool-1", "clientId": "client-1"})
}

#[tokio::test]
async fn oidc_settings_reads_authority_and_client_id_and_ignores_other_keys() {
    let server = MockServer::start().await;
    let mut body = settings_body();
    body["apiUrl"] = json!("https://api.example.com");
    mount_config(
        &server,
        "/config.json",
        ResponseTemplate::new(200).set_body_json(body),
    )
    .await;

    let settings = client(&server.uri()).oidc_settings().await.unwrap();

    assert_eq!(
        settings,
        OidcSettings::new("https://login.example.com/pool-1", "client-1")
    );
}

#[tokio::test]
async fn oidc_settings_sends_no_authorization_header() {
    let server = MockServer::start().await;
    mount_config(
        &server,
        "/config.json",
        ResponseTemplate::new(200).set_body_json(settings_body()),
    )
    .await;

    client(&server.uri()).oidc_settings().await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn oidc_settings_reads_config_json_below_a_base_url_prefix() {
    let server = MockServer::start().await;
    mount_config(
        &server,
        "/platform/config.json",
        ResponseTemplate::new(200).set_body_json(settings_body()),
    )
    .await;

    let settings = client(&format!("{}/platform", server.uri()))
        .oidc_settings()
        .await;

    assert!(settings.is_ok(), "{settings:?}");
}

#[tokio::test]
async fn oidc_settings_fails_with_config_for_an_http_base_url_on_a_non_loopback_host() {
    let client = Client::builder("http://api.example.com")
        .retry_policy(RetryPolicy::none())
        .build()
        .unwrap();

    let error = client.oidc_settings().await.unwrap_err();

    assert!(
        matches!(error, Error::Config { ref message } if message.contains("http://api.example.com")),
        "{error:?}"
    );
}

#[tokio::test]
async fn oidc_settings_fails_with_not_found_when_the_host_serves_no_config_json() {
    let server = MockServer::start().await;
    mount_config(&server, "/config.json", ResponseTemplate::new(404)).await;

    let error = client(&server.uri()).oidc_settings().await.unwrap_err();

    assert!(matches!(error, Error::NotFound { .. }), "{error:?}");
}

#[tokio::test]
async fn oidc_settings_fails_with_unexpected_response_without_client_id() {
    let server = MockServer::start().await;
    mount_config(
        &server,
        "/config.json",
        ResponseTemplate::new(200).set_body_json(json!({"authority": "https://login.example.com"})),
    )
    .await;

    let error = client(&server.uri()).oidc_settings().await.unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
        "{error:?}"
    );
}
