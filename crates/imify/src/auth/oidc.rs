//! OIDC provider calls: discovery, PKCE, the authorization URL, the token
//! endpoint and token revocation.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::header::HeaderMap;
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::Revocation;
use crate::client::network_error;
use crate::error::from_response;
use crate::retry::RetryMode;
use crate::{Client, Error, LoginFailure, OidcSettings};

/// Scopes every login requests.
pub(crate) const SCOPES: &str = "openid email profile";

/// Path of the revocation endpoint on the token endpoint's origin, used when discovery
/// advertises none.
const DERIVED_REVOCATION_PATH: &str = "/oauth2/revoke";

/// The endpoints of an identity provider, read from its discovery document.
#[derive(Debug, Clone)]
pub(crate) struct Provider {
    pub(crate) issuer: String,
    pub(crate) authorization_endpoint: Url,
    pub(crate) token_endpoint: Url,
    /// The advertised revocation endpoint, or [`DERIVED_REVOCATION_PATH`] on the token
    /// endpoint's origin.
    pub(crate) revocation_endpoint: Url,
}

#[derive(Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    revocation_endpoint: Option<String>,
}

/// Reads the discovery document of the issuer in `settings`.
///
/// Fails with [`Error::Config`] when the issuer or an endpoint is not `https` on a
/// non-loopback host, or when the document names another issuer.
pub(crate) async fn discover(client: &Client, settings: &OidcSettings) -> Result<Provider, Error> {
    let issuer = parse_secure_url(&settings.issuer, "issuer")?;
    let document_url = parse_secure_url(
        &format!(
            "{}/.well-known/openid-configuration",
            issuer.as_str().trim_end_matches('/')
        ),
        "issuer",
    )?;
    let document: DiscoveryDocument = client
        .execute_unauthenticated(RetryMode::Idempotent, Method::GET, document_url)
        .await?;
    if document.issuer.trim_end_matches('/') != settings.issuer.trim_end_matches('/') {
        return Err(Error::Config {
            message: format!(
                "the discovery document of {} names the issuer {}",
                settings.issuer, document.issuer
            ),
        });
    }
    let authorization_endpoint =
        parse_secure_url(&document.authorization_endpoint, "authorization endpoint")?;
    let token_endpoint = parse_secure_url(&document.token_endpoint, "token endpoint")?;
    let revocation_endpoint = match &document.revocation_endpoint {
        Some(endpoint) => parse_secure_url(endpoint, "revocation endpoint")?,
        None => derived_revocation_endpoint(&token_endpoint),
    };
    Ok(Provider {
        issuer: document.issuer,
        authorization_endpoint,
        token_endpoint,
        revocation_endpoint,
    })
}

fn derived_revocation_endpoint(token_endpoint: &Url) -> Url {
    let mut endpoint = token_endpoint.clone();
    endpoint.set_path(DERIVED_REVOCATION_PATH);
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    endpoint
}

/// Parses `raw` and requires `https`, or `http` on a loopback host.
pub(crate) fn parse_secure_url(raw: &str, name: &str) -> Result<Url, Error> {
    let url = Url::parse(raw).map_err(|error| Error::Config {
        message: format!("invalid {name} URL {raw:?}: {error}"),
    })?;
    match url.scheme() {
        "https" => Ok(url),
        "http" if is_loopback(&url) => Ok(url),
        _ => Err(Error::Config {
            message: format!(
                "the {name} URL {raw} must use https unless its host is localhost, 127.0.0.1 or ::1"
            ),
        }),
    }
}

/// Returns `true` for the hosts `localhost`, `127.0.0.1` and `::1`.
pub(crate) fn is_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "[::1]"
    })
}

/// A random value with 256 bits of entropy, base64url-encoded: a PKCE verifier, `state`
/// or `nonce`.
pub(crate) fn random_secret() -> Result<String, Error> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| Error::Config {
        message: format!("cannot read random bytes from the operating system: {error}"),
    })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// The PKCE `S256` challenge of `verifier`.
pub(crate) fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The parameters of one authorization request.
#[derive(Debug)]
pub(crate) struct AuthorizationRequest<'a> {
    pub(crate) client_id: &'a str,
    pub(crate) redirect_uri: &'a str,
    pub(crate) state: &'a str,
    pub(crate) nonce: &'a str,
    pub(crate) code_challenge: &'a str,
}

/// The authorization endpoint URL the browser opens.
pub(crate) fn authorization_url(provider: &Provider, request: &AuthorizationRequest<'_>) -> Url {
    let mut url = provider.authorization_endpoint.clone();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", request.client_id)
        .append_pair("redirect_uri", request.redirect_uri)
        .append_pair("scope", SCOPES)
        .append_pair("state", request.state)
        .append_pair("nonce", request.nonce)
        .append_pair("code_challenge", request.code_challenge)
        .append_pair("code_challenge_method", "S256");
    url
}

/// The tokens of a successful token response. `Debug` redacts them.
#[derive(Deserialize)]
pub(crate) struct TokenResponse {
    pub(crate) id_token: Option<String>,
    pub(crate) refresh_token: Option<String>,
}

impl fmt::Debug for TokenResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenResponse")
            .field("id_token", &self.id_token.as_ref().map(|_| "<redacted>"))
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// A failed token request.
#[derive(Debug)]
pub(crate) enum TokenError {
    /// The token endpoint answered with an OAuth error, such as `invalid_grant`.
    Rejected(OAuthError),
    /// The request failed for another reason.
    Failed(Error),
}

/// An OAuth error response. The description has every secret the request sent replaced.
#[derive(Debug, Deserialize)]
pub(crate) struct OAuthError {
    pub(crate) error: String,
    pub(crate) error_description: Option<String>,
}

impl fmt::Display for OAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.error_description {
            Some(description) => write!(formatter, "{} ({description})", self.error),
            None => formatter.write_str(&self.error),
        }
    }
}

impl From<TokenError> for Error {
    fn from(error: TokenError) -> Self {
        match error {
            TokenError::Rejected(rejection) => Error::Login {
                reason: LoginFailure::Provider,
                message: format!("the token endpoint rejected the request: {rejection}"),
            },
            TokenError::Failed(error) => error,
        }
    }
}

/// Exchanges an authorization code for tokens.
pub(crate) async fn exchange_code(
    client: &Client,
    token_endpoint: &Url,
    client_id: &str,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<TokenResponse, TokenError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", code_verifier),
    ];
    request_tokens(client, token_endpoint, &form, &[code, code_verifier]).await
}

/// Exchanges a refresh token for a new ID token.
pub(crate) async fn refresh(
    client: &Client,
    token_endpoint: &Url,
    client_id: &str,
    refresh_token: &str,
) -> Result<TokenResponse, TokenError> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    request_tokens(client, token_endpoint, &form, &[refresh_token]).await
}

/// Revokes `refresh_token` (RFC 7009). A 404 yields [`Revocation::Unsupported`].
pub(crate) async fn revoke(
    client: &Client,
    revocation_endpoint: &Url,
    client_id: &str,
    refresh_token: &str,
) -> Result<Revocation, Error> {
    let form = [
        ("token", refresh_token),
        ("token_type_hint", "refresh_token"),
        ("client_id", client_id),
    ];
    let response = post_form(client, revocation_endpoint, &form).await?;
    match response.status {
        status if status.is_success() => Ok(Revocation::Revoked),
        StatusCode::NOT_FOUND => Ok(Revocation::Unsupported),
        _ => Err(response.into_error(&[refresh_token]).into()),
    }
}

async fn request_tokens(
    client: &Client,
    token_endpoint: &Url,
    form: &[(&str, &str)],
    secrets: &[&str],
) -> Result<TokenResponse, TokenError> {
    let response = post_form(client, token_endpoint, form)
        .await
        .map_err(TokenError::Failed)?;
    if !response.status.is_success() {
        return Err(response.into_error(secrets));
    }
    serde_json::from_slice(&response.body).map_err(|_| {
        TokenError::Failed(Error::UnexpectedResponse {
            status: response.status.as_u16(),
            message: "the token response is not a JSON object with string tokens".to_owned(),
        })
    })
}

struct FormResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl FormResponse {
    fn into_error(self, secrets: &[&str]) -> TokenError {
        let rejection = matches!(
            self.status,
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED
        )
        .then(|| serde_json::from_slice::<OAuthError>(&self.body).ok())
        .flatten();
        match rejection {
            Some(mut rejection) => {
                rejection.error = redact(&rejection.error, secrets);
                rejection.error_description = rejection
                    .error_description
                    .map(|description| redact(&description, secrets));
                TokenError::Rejected(rejection)
            }
            None => TokenError::Failed(from_response(
                self.status.as_u16(),
                &self.headers,
                &self.body,
            )),
        }
    }
}

fn redact(text: &str, secrets: &[&str]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(text.to_owned(), |text, secret| {
            text.replace(secret, "<redacted>")
        })
}

/// Posts `form` without an `Authorization` header. Retries connection failures and rate
/// limits.
async fn post_form(
    client: &Client,
    url: &Url,
    form: &[(&str, &str)],
) -> Result<FormResponse, Error> {
    client
        .retry(
            RetryMode::NotIdempotent,
            |_, _, _| {},
            || async {
                let response = client
                    .http()
                    .post(url.clone())
                    .form(form)
                    .send()
                    .await
                    .map_err(network_error)?;
                let status = response.status();
                let headers = response.headers().clone();
                let body = response.bytes().await.map_err(network_error)?.to_vec();
                if status == StatusCode::TOO_MANY_REQUESTS {
                    return Err(from_response(status.as_u16(), &headers, &body));
                }
                Ok(FormResponse {
                    status,
                    headers,
                    body,
                })
            },
        )
        .await
}

#[cfg(test)]
mod tests;
