use serde_json::json;
use wiremock::ResponseTemplate;

use super::*;
use crate::auth::testing::{CLIENT_ID, MockProvider, REFRESH_TOKEN, client};

fn config_message(result: Result<Provider, Error>) -> String {
    match result {
        Err(Error::Config { message }) => message,
        other => panic!("expected a config error, got {other:?}"),
    }
}

#[tokio::test]
async fn discovery_reads_the_endpoints_of_the_issuer() {
    let provider = MockProvider::start().await;

    let discovered = discover(&client(&provider.issuer()), &provider.settings())
        .await
        .unwrap();

    assert_eq!(discovered.issuer, provider.issuer());
    assert_eq!(
        discovered.token_endpoint.as_str(),
        format!("{}/oauth2/token", provider.issuer())
    );
    assert_eq!(
        discovered.revocation_endpoint.as_str(),
        format!("{}/oauth2/revocation", provider.issuer())
    );
}

#[tokio::test]
async fn an_http_issuer_on_a_non_loopback_host_fails_with_config() {
    let settings = OidcSettings::new("http://idp.example.com", CLIENT_ID);

    let message = config_message(discover(&client("https://api.example.com"), &settings).await);

    assert!(message.contains("http://idp.example.com"), "{message}");
}

#[tokio::test]
async fn an_http_issuer_on_127_0_0_1_is_accepted() {
    let provider = MockProvider::start().await;
    assert!(provider.issuer().starts_with("http://127.0.0.1:"));

    let discovered = discover(&client(&provider.issuer()), &provider.settings()).await;

    assert!(discovered.is_ok(), "{discovered:?}");
}

#[tokio::test]
async fn an_http_endpoint_on_a_non_loopback_host_fails_with_config() {
    let provider = MockProvider::start_without_discovery().await;
    let mut document = provider.discovery_document();
    document["token_endpoint"] = json!("http://idp.example.com/oauth2/token");
    provider.mount_discovery(&document).await;

    let message = config_message(discover(&client(&provider.issuer()), &provider.settings()).await);

    assert!(message.contains("token endpoint"), "{message}");
}

#[tokio::test]
async fn a_discovery_document_that_names_another_issuer_fails_with_config() {
    let provider = MockProvider::start_without_discovery().await;
    let mut document = provider.discovery_document();
    document["issuer"] = json!("https://login.example.com/other-pool");
    provider.mount_discovery(&document).await;

    let message = config_message(discover(&client(&provider.issuer()), &provider.settings()).await);

    assert!(message.contains("other-pool"), "{message}");
}

#[tokio::test]
async fn without_an_advertised_revocation_endpoint_it_is_derived_from_the_token_endpoint_origin() {
    let provider = MockProvider::start_without_discovery().await;
    let mut document = provider.discovery_document();
    document
        .as_object_mut()
        .unwrap()
        .remove("revocation_endpoint");
    document["token_endpoint"] = json!(format!("{}/pool-1/oauth2/token?x=1", provider.issuer()));
    provider.mount_discovery(&document).await;

    let discovered = discover(&client(&provider.issuer()), &provider.settings())
        .await
        .unwrap();

    assert_eq!(
        discovered.revocation_endpoint.as_str(),
        format!("{}/oauth2/revoke", provider.issuer())
    );
}

#[test]
fn the_pkce_challenge_matches_the_rfc_7636_example() {
    let challenge = pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");

    assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
}

#[test]
fn random_secrets_are_43_base64url_characters_and_differ() {
    let first = random_secret().unwrap();
    let second = random_secret().unwrap();

    assert_eq!(first.len(), 43);
    assert!(
        first
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    );
    assert_ne!(first, second);
}

#[tokio::test]
async fn a_rejected_refresh_reports_the_oauth_error_without_the_refresh_token() {
    let provider = MockProvider::start().await;
    provider
        .mount_token(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": format!("refresh token {REFRESH_TOKEN} has been revoked"),
        })))
        .await;
    let endpoint = Url::parse(&format!("{}/oauth2/token", provider.issuer())).unwrap();

    let error = refresh(
        &client(&provider.issuer()),
        &endpoint,
        CLIENT_ID,
        REFRESH_TOKEN,
    )
    .await
    .unwrap_err();

    let TokenError::Rejected(rejection) = error else {
        panic!("expected a rejection, got {error:?}");
    };
    assert_eq!(rejection.error, "invalid_grant");
    assert!(
        !rejection.to_string().contains(REFRESH_TOKEN),
        "{rejection}"
    );
}

#[tokio::test]
async fn a_server_error_from_the_token_endpoint_is_a_server_error() {
    let provider = MockProvider::start().await;
    provider.mount_token(ResponseTemplate::new(503)).await;
    let endpoint = Url::parse(&format!("{}/oauth2/token", provider.issuer())).unwrap();

    let error = refresh(
        &client(&provider.issuer()),
        &endpoint,
        CLIENT_ID,
        REFRESH_TOKEN,
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, TokenError::Failed(Error::Server { status: 503, .. })),
        "{error:?}"
    );
}

#[test]
fn token_response_debug_redacts_the_tokens() {
    let response = TokenResponse {
        id_token: Some("id-token-secret".into()),
        refresh_token: Some(REFRESH_TOKEN.into()),
    };

    let debug = format!("{response:?}");

    assert!(!debug.contains("id-token-secret"), "{debug}");
    assert!(!debug.contains(REFRESH_TOKEN), "{debug}");
}

#[test]
fn http_is_accepted_only_for_localhost_127_0_0_1_and_ipv6_loopback() {
    for accepted in [
        "http://localhost:8080/pool",
        "http://127.0.0.1/pool",
        "http://[::1]:8080/pool",
        "https://login.example.com",
    ] {
        assert!(parse_secure_url(accepted, "issuer").is_ok(), "{accepted}");
    }
    for rejected in [
        "http://login.example.com",
        "http://127.0.0.2",
        "http://localhost.example.com",
        "ftp://localhost",
    ] {
        assert!(parse_secure_url(rejected, "issuer").is_err(), "{rejected}");
    }
}
