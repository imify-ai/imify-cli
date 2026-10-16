use std::collections::HashMap;
use std::net::TcpListener as StdListener;

use serde_json::{Value, json};
use wiremock::ResponseTemplate;

use super::*;
use crate::auth::testing::{
    CLIENT_ID, ID_TOKEN_SUBJECT, MockProvider, REFRESH_TOKEN, claims, client, free_port, jwt,
};

const CODE: &str = "code-8d2f-secret";

fn query(url: &str) -> HashMap<String, String> {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

fn form(body: &str) -> HashMap<String, String> {
    query(&format!("http://localhost/?{body}"))
}

/// A started loopback login and the parameters of its authorization URL.
struct Started {
    login: LoopbackLogin,
    port: u16,
    params: HashMap<String, String>,
}

impl Started {
    fn state(&self) -> String {
        self.params["state"].clone()
    }

    fn nonce(&self) -> String {
        self.params["nonce"].clone()
    }

    fn redirect_query(&self) -> String {
        format!("code={CODE}&state={}", self.state())
    }

    /// Sends a redirect with `query` and returns the result of `finish`.
    async fn finish_after_redirect(self, query: &str) -> Result<Credentials, Error> {
        let finishing = tokio::spawn(self.login.finish(Duration::from_secs(10)));
        reqwest::get(format!("http://127.0.0.1:{}/callback?{query}", self.port))
            .await
            .unwrap();
        finishing.await.unwrap()
    }
}

async fn start_loopback(provider: &MockProvider) -> Started {
    let port = free_port();
    let login = LoopbackLogin::start_on(&client(&provider.issuer()), &provider.settings(), &[port])
        .await
        .unwrap();
    let params = query(login.authorization_url());
    Started {
        login,
        port,
        params,
    }
}

async fn start_paste(provider: &MockProvider) -> (PasteLogin, String) {
    let login = PasteLogin::start(&client(&provider.issuer()), &provider.settings())
        .await
        .unwrap();
    let state = query(login.authorization_url())["state"].clone();
    (login, state)
}

fn token_body(id_token: &str) -> Value {
    json!({
        "id_token": id_token,
        "access_token": "access-token-unused",
        "refresh_token": REFRESH_TOKEN,
        "token_type": "Bearer",
        "expires_in": 3600,
    })
}

fn reason(result: Result<Credentials, Error>) -> LoginFailure {
    match result {
        Err(Error::Login { reason, .. }) => reason,
        other => panic!("expected a login error, got {other:?}"),
    }
}

#[tokio::test]
async fn the_authorization_url_carries_the_pkce_and_oidc_parameters() {
    let provider = MockProvider::start().await;
    let started = start_loopback(&provider).await;

    let url = started.login.authorization_url();
    let params = &started.params;

    assert!(url.starts_with(&format!("{}/oauth2/authorize?", provider.issuer())));
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["client_id"], CLIENT_ID);
    assert_eq!(
        params["redirect_uri"],
        format!("http://localhost:{}/callback", started.port)
    );
    assert_eq!(params["scope"], "openid email profile");
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["code_challenge"].len(), 43);
    assert_eq!(params["state"].len(), 43);
    assert_eq!(params["nonce"].len(), 43);
}

#[tokio::test]
async fn provider_host_is_the_host_of_the_authorization_endpoint() {
    let provider = MockProvider::start().await;

    let started = start_loopback(&provider).await;

    assert_eq!(started.login.provider_host(), "127.0.0.1");
}

#[tokio::test]
async fn start_fails_with_ports_in_use_when_every_port_is_taken() {
    let provider = MockProvider::start().await;
    let taken = StdListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = taken.local_addr().unwrap().port();

    let result =
        LoopbackLogin::start_on(&client(&provider.issuer()), &provider.settings(), &[port]).await;

    assert!(
        matches!(
            result,
            Err(Error::Login {
                reason: LoginFailure::PortsInUse,
                ..
            })
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_redirect_with_the_right_state_yields_credentials_that_expire_with_the_id_token() {
    let provider = MockProvider::start().await;
    let started = start_loopback(&provider).await;
    let claims = claims(&provider.issuer(), Some(&started.nonce()), 3600);
    provider
        .mount_token_response(token_body(&jwt(&claims)))
        .await;
    let query = started.redirect_query();

    let credentials = started.finish_after_redirect(&query).await.unwrap();

    let exp = claims["exp"].as_u64().unwrap();
    assert_eq!(
        credentials.expires_at(),
        UNIX_EPOCH + Duration::from_secs(exp)
    );
    assert_eq!(credentials.issuer(), provider.issuer());
    assert_eq!(credentials.subject(), Some(ID_TOKEN_SUBJECT));
    assert!(credentials.has_refresh_token());
}

#[tokio::test]
async fn the_code_exchange_sends_the_code_verifier_and_redirect_uri() {
    let provider = MockProvider::start().await;
    let started = start_loopback(&provider).await;
    provider
        .mount_token_response(token_body(&provider.id_token(Some(&started.nonce()), 3600)))
        .await;
    let challenge = started.params["code_challenge"].clone();
    let redirect_uri = started.params["redirect_uri"].clone();
    let query = started.redirect_query();

    started.finish_after_redirect(&query).await.unwrap();

    let requests = provider.token_requests().await;
    assert_eq!(requests.len(), 1);
    let form = form(&requests[0]);
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["code"], CODE);
    assert_eq!(form["client_id"], CLIENT_ID);
    assert_eq!(form["redirect_uri"], redirect_uri);
    assert_eq!(oidc::pkce_challenge(&form["code_verifier"]), challenge);
}

#[tokio::test]
async fn a_redirect_with_a_wrong_state_fails_without_a_token_request() {
    let provider = MockProvider::start().await;
    let started = start_loopback(&provider).await;

    let result = started
        .finish_after_redirect(&format!("code={CODE}&state=other-state"))
        .await;

    assert_eq!(reason(result), LoginFailure::StateMismatch);
    assert_eq!(provider.token_requests().await, Vec::<String>::new());
}

#[tokio::test]
async fn an_id_token_with_a_wrong_nonce_fails_with_invalid_id_token() {
    let provider = MockProvider::start().await;
    let started = start_loopback(&provider).await;
    provider
        .mount_token_response(token_body(&provider.id_token(Some("other-nonce"), 3600)))
        .await;
    let query = started.redirect_query();

    let result = started.finish_after_redirect(&query).await;

    assert_eq!(reason(result), LoginFailure::InvalidIdToken);
}

#[tokio::test]
async fn an_id_token_with_a_wrong_audience_fails_with_invalid_id_token() {
    let provider = MockProvider::start().await;
    let started = start_loopback(&provider).await;
    let mut claims = claims(&provider.issuer(), Some(&started.nonce()), 3600);
    claims["aud"] = json!("other-client");
    provider
        .mount_token_response(token_body(&jwt(&claims)))
        .await;
    let query = started.redirect_query();

    let result = started.finish_after_redirect(&query).await;

    assert_eq!(reason(result), LoginFailure::InvalidIdToken);
}

#[tokio::test]
async fn an_id_token_from_another_issuer_fails_with_invalid_id_token() {
    let provider = MockProvider::start().await;
    let started = start_loopback(&provider).await;
    let claims = claims(
        "https://login.example.com/other",
        Some(&started.nonce()),
        3600,
    );
    provider
        .mount_token_response(token_body(&jwt(&claims)))
        .await;
    let query = started.redirect_query();

    let result = started.finish_after_redirect(&query).await;

    assert_eq!(reason(result), LoginFailure::InvalidIdToken);
}

#[tokio::test]
async fn an_expired_id_token_fails_with_invalid_id_token() {
    let provider = MockProvider::start().await;
    let started = start_loopback(&provider).await;
    provider
        .mount_token_response(token_body(&provider.id_token(Some(&started.nonce()), -120)))
        .await;
    let query = started.redirect_query();

    let result = started.finish_after_redirect(&query).await;

    assert_eq!(reason(result), LoginFailure::InvalidIdToken);
}

#[tokio::test]
async fn a_token_response_without_an_id_token_fails_with_provider() {
    let provider = MockProvider::start().await;
    let started = start_loopback(&provider).await;
    provider
        .mount_token_response(json!({"access_token": "a", "refresh_token": REFRESH_TOKEN}))
        .await;
    let query = started.redirect_query();

    let result = started.finish_after_redirect(&query).await;

    assert_eq!(reason(result), LoginFailure::Provider);
}

#[tokio::test]
async fn a_rejected_code_exchange_fails_with_provider_without_the_code() {
    let provider = MockProvider::start().await;
    let started = start_loopback(&provider).await;
    provider
        .mount_token(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": format!("code {CODE} has expired"),
        })))
        .await;
    let query = started.redirect_query();

    let error = started.finish_after_redirect(&query).await.unwrap_err();

    assert!(
        matches!(
            error,
            Error::Login {
                reason: LoginFailure::Provider,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(!error.to_string().contains(CODE), "{error}");
}

#[tokio::test]
async fn access_denied_fails_with_denied() {
    let provider = MockProvider::start().await;
    let started = start_loopback(&provider).await;
    let query = format!("error=access_denied&state={}", started.state());

    let result = started.finish_after_redirect(&query).await;

    assert_eq!(reason(result), LoginFailure::Denied);
}

#[tokio::test]
async fn no_redirect_within_the_timeout_fails_with_timed_out() {
    let provider = MockProvider::start().await;
    let started = start_loopback(&provider).await;

    let result = started.login.finish(Duration::from_millis(50)).await;

    assert_eq!(reason(result), LoginFailure::TimedOut);
}

#[tokio::test]
async fn paste_login_redirects_to_the_first_loopback_port() {
    let provider = MockProvider::start().await;

    let (login, _) = start_paste(&provider).await;

    assert_eq!(
        query(login.authorization_url())["redirect_uri"],
        "http://localhost:28753/callback"
    );
}

#[tokio::test]
async fn paste_login_accepts_the_redirect_url_with_surrounding_whitespace() {
    let provider = MockProvider::start().await;
    let (login, state) = start_paste(&provider).await;
    let nonce = query(login.authorization_url())["nonce"].clone();
    provider
        .mount_token_response(token_body(&provider.id_token(Some(&nonce), 3600)))
        .await;

    let result = login
        .finish(&format!(
            "  http://localhost:28753/callback?code={CODE}&state={state}\r\n"
        ))
        .await;

    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test]
async fn paste_login_accepts_a_redirect_url_longer_than_1024_bytes() {
    let provider = MockProvider::start().await;
    let (login, state) = start_paste(&provider).await;
    let nonce = query(login.authorization_url())["nonce"].clone();
    provider
        .mount_token_response(token_body(&provider.id_token(Some(&nonce), 3600)))
        .await;
    let long_code = "c".repeat(1500);

    let result = login
        .finish(&format!(
            "http://localhost:28753/callback?code={long_code}&state={state}"
        ))
        .await;

    assert!(result.is_ok(), "{result:?}");
    assert_eq!(form(&provider.token_requests().await[0])["code"], long_code);
}

#[tokio::test]
async fn paste_login_rejects_a_url_without_code_and_state() {
    let provider = MockProvider::start().await;
    let (login, _) = start_paste(&provider).await;

    let result = login
        .finish("http://localhost:28753/callback?foo=bar")
        .await;

    assert_eq!(reason(result), LoginFailure::InvalidRedirectUrl);
}

#[tokio::test]
async fn paste_login_rejects_text_that_is_not_a_url() {
    let provider = MockProvider::start().await;
    let (login, _) = start_paste(&provider).await;

    let result = login.finish("code=abc&state=xyz").await;

    assert_eq!(reason(result), LoginFailure::InvalidRedirectUrl);
}

#[tokio::test]
async fn login_debug_contains_no_verifier_state_or_nonce() {
    let provider = MockProvider::start().await;
    let started = start_loopback(&provider).await;
    let (paste, _) = start_paste(&provider).await;

    for debug in [format!("{:?}", started.login), format!("{paste:?}")] {
        for secret in ["state", "nonce", "code_challenge"] {
            assert!(
                !debug.contains(&started.params[secret]),
                "{secret} in {debug}"
            );
        }
        assert!(!debug.contains("code_verifier"), "{debug}");
    }
}

#[test]
fn credentials_debug_redacts_the_tokens() {
    let credentials = sample_credentials();

    let debug = format!("{credentials:?}");

    assert!(!debug.contains("id-token-secret"), "{debug}");
    assert!(!debug.contains(REFRESH_TOKEN), "{debug}");
}

#[test]
fn credentials_serialize_with_version_2_and_round_trip() {
    let credentials = sample_credentials();

    let json = serde_json::to_value(&credentials).unwrap();
    let parsed: Credentials = serde_json::from_value(json.clone()).unwrap();

    assert_eq!(json["version"], 2);
    assert_eq!(json["subject"], ID_TOKEN_SUBJECT);
    assert_eq!(parsed.id_token(), "id-token-secret");
    assert_eq!(parsed.refresh_token(), Some(REFRESH_TOKEN));
    assert_eq!(parsed.subject(), Some(ID_TOKEN_SUBJECT));
}

#[test]
fn credentials_of_version_1_without_a_subject_deserialize() {
    let mut json = serde_json::to_value(sample_credentials()).unwrap();
    json["version"] = json!(1);
    json.as_object_mut().unwrap().remove("subject");

    let parsed: Credentials = serde_json::from_value(json).unwrap();

    assert_eq!(parsed.subject(), None);
    assert_eq!(parsed.id_token(), "id-token-secret");
}

#[test]
fn credentials_of_another_version_do_not_deserialize() {
    let mut json = serde_json::to_value(sample_credentials()).unwrap();
    json["version"] = json!(3);

    let result = serde_json::from_value::<Credentials>(json);

    assert!(result.is_err());
}

fn sample_credentials() -> Credentials {
    Credentials {
        version: 2,
        issuer: "https://login.example.com".into(),
        client_id: CLIENT_ID.into(),
        token_endpoint: "https://login.example.com/oauth2/token".into(),
        revocation_endpoint: "https://login.example.com/oauth2/revoke".into(),
        subject: Some(ID_TOKEN_SUBJECT.into()),
        id_token: "id-token-secret".into(),
        id_token_expires_at: 1_900_000_000,
        refresh_token: Some(REFRESH_TOKEN.into()),
    }
}
