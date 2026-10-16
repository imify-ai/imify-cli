//! Test doubles for the identity provider: unsigned ID tokens and a mock provider.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Url;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::id_token::Checked;
use super::oidc::Provider;
use super::{CredentialStore, Credentials};
use crate::{Client, Error, OidcSettings, RetryPolicy};

pub(crate) const CLIENT_ID: &str = "client-1";
pub(crate) const ID_TOKEN_SUBJECT: &str = "user-1234";
pub(crate) const REFRESH_TOKEN: &str = "rt_5b1e-secret";

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A port that was free on `127.0.0.1` when this function returned.
pub(crate) fn free_port() -> u16 {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// An unsigned JWT with the given payload.
pub(crate) fn jwt(payload: &Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
    format!("{header}.{payload}.c2lnbmF0dXJl")
}

/// The claims of a valid ID token from `issuer`, expiring `lifetime` seconds from now.
pub(crate) fn claims(issuer: &str, nonce: Option<&str>, lifetime: i64) -> Value {
    let exp = unix_now().saturating_add_signed(lifetime);
    let mut claims = json!({
        "iss": issuer,
        "sub": ID_TOKEN_SUBJECT,
        "aud": CLIENT_ID,
        "exp": exp,
        "iat": unix_now(),
    });
    if let Some(nonce) = nonce {
        claims["nonce"] = json!(nonce);
    }
    claims
}

/// A client without retries and without a token source.
pub(crate) fn client(base_url: &str) -> Client {
    Client::builder(base_url)
        .retry_policy(RetryPolicy::none())
        .build()
        .unwrap()
}

/// A mock identity provider whose issuer is the server's URI.
#[derive(Debug)]
pub(crate) struct MockProvider {
    pub(crate) server: MockServer,
}

impl MockProvider {
    /// Starts a provider whose discovery document advertises a revocation endpoint.
    pub(crate) async fn start() -> Self {
        let provider = Self::start_without_discovery().await;
        let document = provider.discovery_document();
        provider.mount_discovery(&document).await;
        provider
    }

    pub(crate) async fn start_without_discovery() -> Self {
        Self {
            server: MockServer::start().await,
        }
    }

    pub(crate) fn issuer(&self) -> String {
        self.server.uri()
    }

    pub(crate) fn settings(&self) -> OidcSettings {
        OidcSettings::new(self.issuer(), CLIENT_ID)
    }

    pub(crate) fn discovery_document(&self) -> Value {
        let issuer = self.issuer();
        json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/oauth2/authorize"),
            "token_endpoint": format!("{issuer}/oauth2/token"),
            "revocation_endpoint": format!("{issuer}/oauth2/revocation"),
            "jwks_uri": format!("{issuer}/.well-known/jwks.json"),
        })
    }

    pub(crate) async fn mount_discovery(&self, document: &Value) {
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(document))
            .mount(&self.server)
            .await;
    }

    /// A valid ID token from this provider, expiring `lifetime` seconds from now.
    pub(crate) fn id_token(&self, nonce: Option<&str>, lifetime: i64) -> String {
        jwt(&claims(&self.issuer(), nonce, lifetime))
    }

    /// Answers every token request with `body` and status 200.
    pub(crate) async fn mount_token_response(&self, body: Value) {
        self.mount_token(ResponseTemplate::new(200).set_body_json(body))
            .await;
    }

    pub(crate) async fn mount_token(&self, response: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(response)
            .mount(&self.server)
            .await;
    }

    /// Bodies of the requests the token endpoint received.
    pub(crate) async fn token_requests(&self) -> Vec<String> {
        self.server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|request| request.url.path() == "/oauth2/token")
            .map(|request| String::from_utf8(request.body).unwrap())
            .collect()
    }

    /// The endpoints of this provider as discovery reports them.
    pub(crate) fn endpoints(&self) -> Provider {
        let url = |path: &str| Url::parse(&format!("{}{path}", self.issuer())).unwrap();
        Provider {
            issuer: self.issuer(),
            authorization_endpoint: url("/oauth2/authorize"),
            token_endpoint: url("/oauth2/token"),
            revocation_endpoint: url("/oauth2/revocation"),
        }
    }

    /// Credentials of a login at this provider whose ID token expires in `lifetime`
    /// seconds.
    pub(crate) fn credentials(&self, lifetime: i64, refresh_token: Option<&str>) -> Credentials {
        let id_token = self.id_token(None, lifetime);
        let checked = Checked {
            expires_at: unix_now().saturating_add_signed(lifetime),
            subject: ID_TOKEN_SUBJECT.to_owned(),
        };
        Credentials::new(
            &self.endpoints(),
            CLIENT_ID.to_owned(),
            id_token,
            checked,
            refresh_token.map(str::to_owned),
        )
    }
}

/// A credential store in memory that counts its saves.
#[derive(Debug, Default)]
pub(crate) struct MemoryStore {
    credentials: Mutex<Option<Credentials>>,
    saves: AtomicUsize,
}

impl MemoryStore {
    pub(crate) fn holding(credentials: Credentials) -> Arc<Self> {
        Arc::new(Self {
            credentials: Mutex::new(Some(credentials)),
            saves: AtomicUsize::new(0),
        })
    }

    pub(crate) fn stored(&self) -> Option<Credentials> {
        self.credentials.lock().unwrap().clone()
    }

    /// Replaces the stored credentials without counting a save, as another writer would.
    pub(crate) fn replace(&self, credentials: Credentials) {
        *self.credentials.lock().unwrap() = Some(credentials);
    }

    pub(crate) fn save_count(&self) -> usize {
        self.saves.load(Ordering::SeqCst)
    }
}

impl CredentialStore for MemoryStore {
    fn load(&self) -> Result<Option<Credentials>, Error> {
        Ok(self.stored())
    }

    fn save(&self, credentials: &Credentials) -> Result<(), Error> {
        self.saves.fetch_add(1, Ordering::SeqCst);
        *self.credentials.lock().unwrap() = Some(credentials.clone());
        Ok(())
    }

    fn delete(&self) -> Result<bool, Error> {
        Ok(self.credentials.lock().unwrap().take().is_some())
    }
}
