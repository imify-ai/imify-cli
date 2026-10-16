use std::collections::HashMap;

use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, ResponseTemplate};

use super::*;
use crate::auth::testing::{MemoryStore, MockProvider, REFRESH_TOKEN, client, unix_now};

const NEW_REFRESH_TOKEN: &str = "rt_9c4d-rotated-secret";

fn form(body: &str) -> HashMap<String, String> {
    reqwest::Url::parse(&format!("http://localhost/?{body}"))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

async fn stored_login(store: &Arc<MemoryStore>) -> StoredLogin {
    let store: Arc<dyn CredentialStore> = store.clone();
    StoredLogin::load(store).await.unwrap().unwrap()
}

async fn token_of(login: &StoredLogin) -> Result<String, Error> {
    Ok(login.token().await?.secret().to_owned())
}

fn reason(result: Result<String, Error>) -> LoginFailure {
    match result {
        Err(Error::Login { reason, .. }) => reason,
        other => panic!("expected a login error, got {other:?}"),
    }
}

#[tokio::test]
async fn load_returns_none_when_the_store_is_empty() {
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());

    let login = StoredLogin::load(store).await.unwrap();

    assert!(login.is_none());
}

#[tokio::test]
async fn the_stored_id_token_is_returned_without_a_request_while_more_than_60_s_remain() {
    let provider = MockProvider::start().await;
    let credentials = provider.credentials(120, Some(REFRESH_TOKEN));
    let store = MemoryStore::holding(credentials.clone());

    let token = token_of(&stored_login(&store).await).await.unwrap();

    assert_eq!(token, credentials.id_token());
    assert_eq!(provider.token_requests().await, Vec::<String>::new());
}

#[tokio::test]
async fn the_id_token_is_refreshed_and_saved_when_less_than_60_s_remain() {
    let provider = MockProvider::start().await;
    let store = MemoryStore::holding(provider.credentials(30, Some(REFRESH_TOKEN)));
    let new_id_token = provider.id_token(None, 3600);
    provider
        .mount_token_response(json!({"id_token": new_id_token, "refresh_token": NEW_REFRESH_TOKEN}))
        .await;

    let token = token_of(&stored_login(&store).await).await.unwrap();

    assert_eq!(token, new_id_token);
    let saved = store.stored().unwrap();
    assert_eq!(saved.id_token(), new_id_token);
    assert_eq!(saved.refresh_token(), Some(NEW_REFRESH_TOKEN));
    let expected_expiry = unix_now() + 3600;
    let expiry = saved
        .expires_at()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(
        expiry.abs_diff(expected_expiry) <= 2,
        "{expiry} vs {expected_expiry}"
    );
    assert_eq!(store.save_count(), 1);
}

#[tokio::test]
async fn the_refresh_request_sends_the_refresh_token_and_client_id() {
    let provider = MockProvider::start().await;
    let store = MemoryStore::holding(provider.credentials(30, Some(REFRESH_TOKEN)));
    provider
        .mount_token_response(json!({"id_token": provider.id_token(None, 3600)}))
        .await;

    token_of(&stored_login(&store).await).await.unwrap();

    let requests = provider.token_requests().await;
    assert_eq!(requests.len(), 1);
    let form = form(&requests[0]);
    assert_eq!(form["grant_type"], "refresh_token");
    assert_eq!(form["refresh_token"], REFRESH_TOKEN);
    assert_eq!(form["client_id"], "client-1");
}

#[tokio::test]
async fn the_old_refresh_token_is_kept_when_the_response_omits_one() {
    let provider = MockProvider::start().await;
    let store = MemoryStore::holding(provider.credentials(30, Some(REFRESH_TOKEN)));
    provider
        .mount_token_response(json!({"id_token": provider.id_token(None, 3600)}))
        .await;

    token_of(&stored_login(&store).await).await.unwrap();

    assert_eq!(store.stored().unwrap().refresh_token(), Some(REFRESH_TOKEN));
}

#[tokio::test]
async fn concurrent_requests_share_one_refresh() {
    let provider = MockProvider::start().await;
    let store = MemoryStore::holding(provider.credentials(30, Some(REFRESH_TOKEN)));
    provider
        .mount_token_response(json!({"id_token": provider.id_token(None, 3600)}))
        .await;
    let login = stored_login(&store).await;

    let (first, second) = tokio::join!(token_of(&login), token_of(&login));

    assert_eq!(first.unwrap(), second.unwrap());
    assert_eq!(provider.token_requests().await.len(), 1);
}

#[tokio::test]
async fn invalid_grant_fails_with_expired() {
    let provider = MockProvider::start().await;
    let store = MemoryStore::holding(provider.credentials(30, Some(REFRESH_TOKEN)));
    provider
        .mount_token(ResponseTemplate::new(400).set_body_json(json!({"error": "invalid_grant"})))
        .await;

    let result = token_of(&stored_login(&store).await).await;

    assert_eq!(reason(result), LoginFailure::Expired);
}

#[tokio::test]
async fn invalid_grant_uses_a_fresh_login_another_writer_saved() {
    let provider = MockProvider::start().await;
    let store = MemoryStore::holding(provider.credentials(30, Some(REFRESH_TOKEN)));
    provider
        .mount_token(ResponseTemplate::new(400).set_body_json(json!({"error": "invalid_grant"})))
        .await;
    let login = stored_login(&store).await;
    let newer = provider.credentials(3600, Some(NEW_REFRESH_TOKEN));
    store.replace(newer.clone());

    let token = token_of(&login).await.unwrap();

    assert_eq!(token, newer.id_token());
}

#[tokio::test]
async fn an_expiring_login_without_a_refresh_token_fails_with_expired() {
    let provider = MockProvider::start().await;
    let store = MemoryStore::holding(provider.credentials(30, None));

    let result = token_of(&stored_login(&store).await).await;

    assert_eq!(reason(result), LoginFailure::Expired);
}

#[tokio::test]
async fn a_refresh_response_without_an_id_token_fails_with_provider() {
    let provider = MockProvider::start().await;
    let store = MemoryStore::holding(provider.credentials(30, Some(REFRESH_TOKEN)));
    provider
        .mount_token_response(json!({"access_token": "a", "refresh_token": NEW_REFRESH_TOKEN}))
        .await;

    let result = token_of(&stored_login(&store).await).await;

    assert_eq!(reason(result), LoginFailure::Provider);
    assert_eq!(store.save_count(), 0);
}

#[tokio::test]
async fn the_client_sends_the_stored_id_token_as_bearer_token() {
    let provider = MockProvider::start().await;
    let credentials = provider.credentials(3600, Some(REFRESH_TOKEN));
    let store = MemoryStore::holding(credentials.clone());
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .and(header(
            "authorization",
            format!("Bearer {}", credentials.id_token()).as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "subject": "user-1234", "name": "Ada Lovelace", "role": "admin"
        })))
        .expect(1)
        .mount(&provider.server)
        .await;
    let api = Client::builder(&provider.issuer())
        .token_source(stored_login(&store).await)
        .build()
        .unwrap();

    api.me().await.unwrap();
}

#[tokio::test]
async fn stored_login_debug_contains_no_token() {
    let provider = MockProvider::start().await;
    let credentials = provider.credentials(3600, Some(REFRESH_TOKEN));
    let store = MemoryStore::holding(credentials.clone());

    let debug = format!("{:?}", stored_login(&store).await);

    assert!(!debug.contains(credentials.id_token()), "{debug}");
    assert!(!debug.contains(REFRESH_TOKEN), "{debug}");
}

async fn mount_revocation(provider: &MockProvider, status: u16) {
    Mock::given(method("POST"))
        .and(path("/oauth2/revocation"))
        .respond_with(ResponseTemplate::new(status))
        .mount(&provider.server)
        .await;
}

#[tokio::test]
async fn revoke_posts_the_refresh_token_and_returns_revoked_on_200() {
    let provider = MockProvider::start().await;
    mount_revocation(&provider, 200).await;
    let credentials = provider.credentials(3600, Some(REFRESH_TOKEN));

    let revocation = revoke(&client(&provider.issuer()), &credentials)
        .await
        .unwrap();

    assert_eq!(revocation, Revocation::Revoked);
    let requests = provider.server.received_requests().await.unwrap();
    let form = form(std::str::from_utf8(&requests[0].body).unwrap());
    assert_eq!(form["token"], REFRESH_TOKEN);
    assert_eq!(form["token_type_hint"], "refresh_token");
    assert_eq!(form["client_id"], "client-1");
    assert!(!requests[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn revoke_returns_unsupported_on_404() {
    let provider = MockProvider::start().await;
    mount_revocation(&provider, 404).await;
    let credentials = provider.credentials(3600, Some(REFRESH_TOKEN));

    let revocation = revoke(&client(&provider.issuer()), &credentials)
        .await
        .unwrap();

    assert_eq!(revocation, Revocation::Unsupported);
}

#[tokio::test]
async fn revoke_returns_unsupported_without_a_refresh_token() {
    let provider = MockProvider::start().await;
    let credentials = provider.credentials(3600, None);

    let revocation = revoke(&client(&provider.issuer()), &credentials)
        .await
        .unwrap();

    assert_eq!(revocation, Revocation::Unsupported);
    assert!(
        provider
            .server
            .received_requests()
            .await
            .unwrap()
            .is_empty()
    );
}
