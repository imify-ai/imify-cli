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

async fn requests_to(server: &MockServer, route: &str) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == route)
        .collect()
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

    let requests = requests_to(&server, "/config.json").await;
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
async fn oidc_settings_follows_a_redirect_within_the_api_origin() {
    let server = MockServer::start().await;
    mount_config(
        &server,
        "/config.json",
        ResponseTemplate::new(307).insert_header("location", "/v2/config.json"),
    )
    .await;
    mount_config(
        &server,
        "/v2/config.json",
        ResponseTemplate::new(200).set_body_json(settings_body()),
    )
    .await;

    let settings = client(&server.uri()).oidc_settings().await;

    assert!(settings.is_ok(), "{settings:?}");
}

#[tokio::test]
async fn oidc_settings_does_not_follow_a_redirect_to_another_origin() {
    let server = MockServer::start().await;
    let target = MockServer::start().await;
    mount_config(
        &target,
        "/config.json",
        ResponseTemplate::new(200).set_body_json(settings_body()),
    )
    .await;
    mount_config(
        &server,
        "/config.json",
        ResponseTemplate::new(307)
            .insert_header("location", format!("{}/config.json", target.uri())),
    )
    .await;

    let error = client(&server.uri()).oidc_settings().await.unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 307, .. }),
        "{error:?}"
    );
    assert!(target.received_requests().await.unwrap().is_empty());
}

/// A `config.json` body of more than 1 MiB.
fn oversized_settings() -> String {
    let mut body = settings_body();
    body["padding"] = json!("x".repeat(1024 * 1024));
    body.to_string()
}

#[tokio::test]
async fn oidc_settings_fails_with_unexpected_response_for_a_config_json_over_1_mib() {
    let server = MockServer::start().await;
    mount_config(
        &server,
        "/config.json",
        ResponseTemplate::new(200).set_body_string(oversized_settings()),
    )
    .await;

    let error = client(&server.uri()).oidc_settings().await.unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, ref message } if message.contains("1048576")),
        "{error:?}"
    );
}

#[tokio::test]
async fn oidc_settings_fails_for_a_config_json_over_1_mib_without_content_length() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        // The first connection asks for the protected resource metadata, the second for
        // `config.json`.
        for body in [None, Some(oversized_settings())] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = [0_u8; 4096];
            let _ = stream.read(&mut head).await;
            let response = body.map_or_else(
                || "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
                |body| format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}"
                ),
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });

    let error = client(&format!("http://{address}"))
        .oidc_settings()
        .await
        .unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
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

const METADATA_PATH: &str = "/.well-known/oauth-protected-resource/api/v1";

/// Protected resource metadata for the API at `base_url`.
fn metadata_body(base_url: &str) -> serde_json::Value {
    json!({
        "resource": format!("{}/api/v1", base_url.trim_end_matches('/')),
        "authorization_servers": ["https://login.example.com/cli"],
        "scopes_supported": ["openid", "email", "profile"],
        "bearer_methods_supported": ["header"],
        "imify_cli_client_id": "cli-client",
    })
}

/// A server that serves `config.json` and answers the metadata request with the
/// response `metadata` builds from the server's URL.
async fn server_with_metadata(metadata: impl FnOnce(&str) -> ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    mount_config(&server, METADATA_PATH, metadata(&server.uri())).await;
    mount_config(
        &server,
        "/config.json",
        ResponseTemplate::new(200).set_body_json(settings_body()),
    )
    .await;
    server
}

fn metadata_ok(base_url: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(metadata_body(base_url))
}

#[tokio::test]
async fn oidc_settings_reads_the_issuer_and_cli_client_id_from_protected_resource_metadata() {
    let server = server_with_metadata(metadata_ok).await;

    let settings = client(&server.uri()).oidc_settings().await.unwrap();

    assert_eq!(
        settings,
        OidcSettings::new("https://login.example.com/cli", "cli-client")
    );
}

#[tokio::test]
async fn oidc_settings_does_not_request_config_json_when_metadata_exists() {
    let server = server_with_metadata(metadata_ok).await;

    client(&server.uri()).oidc_settings().await.unwrap();

    assert!(requests_to(&server, "/config.json").await.is_empty());
}

#[tokio::test]
async fn oidc_settings_reads_config_json_when_the_metadata_request_answers_404() {
    let server = server_with_metadata(|_| ResponseTemplate::new(404)).await;

    let settings = client(&server.uri()).oidc_settings().await.unwrap();

    assert_eq!(
        settings,
        OidcSettings::new("https://login.example.com/pool-1", "client-1")
    );
}

#[tokio::test]
async fn oidc_settings_reads_config_json_without_retry_when_metadata_answers_403_without_json() {
    let server = server_with_metadata(|_| {
        ResponseTemplate::new(403)
            .insert_header("content-type", "application/xml")
            .set_body_string("<Error><Code>AccessDenied</Code></Error>")
    })
    .await;
    let client = Client::builder(&server.uri())
        .retry_policy(RetryPolicy {
            initial_backoff: std::time::Duration::from_millis(1),
            ..RetryPolicy::default()
        })
        .build()
        .unwrap();

    let settings = client.oidc_settings().await;

    assert!(settings.is_ok(), "{settings:?}");
    assert_eq!(
        (
            requests_to(&server, METADATA_PATH).await.len(),
            requests_to(&server, "/config.json").await.len()
        ),
        (1, 1)
    );
}

#[tokio::test]
async fn oidc_settings_fails_with_server_without_reading_config_json_when_metadata_answers_500() {
    let server = server_with_metadata(|_| ResponseTemplate::new(500)).await;

    let error = client(&server.uri()).oidc_settings().await.unwrap_err();

    assert!(
        matches!(error, Error::Server { status: 500, .. }),
        "{error:?}"
    );
    assert!(requests_to(&server, "/config.json").await.is_empty());
}

#[tokio::test]
async fn oidc_settings_fails_with_config_when_the_metadata_names_another_resource() {
    let server = server_with_metadata(|_| metadata_ok("https://other.example.com")).await;

    let error = client(&server.uri()).oidc_settings().await.unwrap_err();

    assert!(
        matches!(error, Error::Config { ref message } if message.contains("https://other.example.com/api/v1")),
        "{error:?}"
    );
}

#[tokio::test]
async fn oidc_settings_accepts_a_metadata_resource_with_a_trailing_slash() {
    let server = server_with_metadata(|uri| {
        let mut body = metadata_body(uri);
        body["resource"] = json!(format!("{uri}/api/v1/"));
        ResponseTemplate::new(200).set_body_json(body)
    })
    .await;

    let settings = client(&server.uri()).oidc_settings().await;

    assert!(settings.is_ok(), "{settings:?}");
}

#[tokio::test]
async fn oidc_settings_fails_with_unexpected_response_for_metadata_without_authorization_servers() {
    let server = server_with_metadata(|uri| {
        let mut body = metadata_body(uri);
        body["authorization_servers"] = json!([]);
        ResponseTemplate::new(200).set_body_json(body)
    })
    .await;

    let error = client(&server.uri()).oidc_settings().await.unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn oidc_settings_fails_with_unexpected_response_for_metadata_without_cli_client_id() {
    let server = server_with_metadata(|uri| {
        let mut body = metadata_body(uri);
        body.as_object_mut().unwrap().remove("imify_cli_client_id");
        ResponseTemplate::new(200).set_body_json(body)
    })
    .await;

    let error = client(&server.uri()).oidc_settings().await.unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn oidc_settings_puts_the_base_url_prefix_after_the_well_known_segment() {
    let server = MockServer::start().await;
    let base_url = format!("{}/platform", server.uri());
    mount_config(
        &server,
        "/.well-known/oauth-protected-resource/platform/api/v1",
        metadata_ok(&base_url),
    )
    .await;

    let settings = client(&base_url).oidc_settings().await.unwrap();

    assert_eq!(settings.client_id, "cli-client");
}

#[tokio::test]
async fn oidc_settings_sends_no_authorization_header_with_the_metadata_request() {
    let server = server_with_metadata(metadata_ok).await;

    client(&server.uri()).oidc_settings().await.unwrap();

    let requests = requests_to(&server, METADATA_PATH).await;
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn oidc_settings_does_not_follow_a_metadata_redirect_to_another_origin() {
    let target = MockServer::start().await;
    let target_uri = target.uri();
    mount_config(&target, METADATA_PATH, metadata_ok(&target_uri)).await;
    let server = server_with_metadata(|_| {
        ResponseTemplate::new(307).insert_header("location", format!("{target_uri}{METADATA_PATH}"))
    })
    .await;

    let error = client(&server.uri()).oidc_settings().await.unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 307, .. }),
        "{error:?}"
    );
    assert!(target.received_requests().await.unwrap().is_empty());
}
