//! Browser login: the authorization code flow with PKCE, received on `localhost` or
//! pasted by the user.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Url;
use serde::{Deserialize, Deserializer, Serialize};

use super::id_token::{self, Checked, Expected};
use super::loopback::{CALLBACK_PATH, Callback, RedirectListener, parse_callback};
use super::oidc::{self, AuthorizationRequest, Provider};
use crate::{Client, Error, LoginFailure, OidcSettings};

/// Browser login over a redirect to `http://localhost:<port>/callback`.
///
/// `Debug` prints neither the PKCE verifier, `state` nor `nonce`.
///
/// # Examples
///
/// ```no_run
/// # async fn run() -> Result<(), imify::Error> {
/// use std::time::Duration;
/// use imify::auth::LoopbackLogin;
///
/// let client = imify::Client::builder("https://api.example.com").build()?;
/// let settings = client.oidc_settings().await?;
/// let login = LoopbackLogin::start(&client, &settings).await?;
/// eprintln!("Signing in at {}.", login.provider_host());
/// eprintln!("{}", login.authorization_url());
/// let credentials = login.finish(Duration::from_secs(300)).await?;
/// assert!(credentials.has_refresh_token());
/// # Ok(()) }
/// ```
#[derive(Debug)]
pub struct LoopbackLogin {
    session: Session,
    listener: RedirectListener,
}

impl LoopbackLogin {
    /// The redirect ports, tried in order.
    pub const PORTS: [u16; 3] = [28753, 28754, 28755];

    /// Reads the provider's discovery document and listens on the first free port of
    /// [`LoopbackLogin::PORTS`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] when the issuer or a discovered endpoint is not `https`
    /// on a host other than `localhost`, `127.0.0.1` or `::1`, or when the discovery
    /// document names another issuer. Returns [`Error::Login`] with
    /// [`LoginFailure::PortsInUse`] when no port is free. Returns the other [`Error`]
    /// variants as for every request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
    /// let settings = imify::OidcSettings::new("https://login.example.com", "client-1");
    /// let login = imify::auth::LoopbackLogin::start(&client, &settings).await?;
    /// # Ok(()) }
    /// ```
    pub async fn start(client: &Client, settings: &OidcSettings) -> Result<Self, Error> {
        Self::start_on(client, settings, &Self::PORTS).await
    }

    pub(crate) async fn start_on(
        client: &Client,
        settings: &OidcSettings,
        ports: &[u16],
    ) -> Result<Self, Error> {
        let provider = oidc::discover(client, settings).await?;
        let listener = RedirectListener::bind(ports).await?;
        let session = Session::new(client, settings, provider, listener.port())?;
        Ok(Self { session, listener })
    }

    /// The URL to open in the browser.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn run(login: imify::auth::LoopbackLogin) {
    /// eprintln!("Open {}", login.authorization_url());
    /// # }
    /// ```
    #[must_use]
    pub fn authorization_url(&self) -> &str {
        &self.session.authorization_url
    }

    /// Host of the authorization endpoint, for display before the browser opens.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn run(login: imify::auth::LoopbackLogin) {
    /// eprintln!("Signing in at {}.", login.provider_host());
    /// # }
    /// ```
    #[must_use]
    pub fn provider_host(&self) -> &str {
        &self.session.provider_host
    }

    /// Waits up to `timeout` for the redirect that carries this login's `state`, then
    /// exchanges the authorization code for tokens. Redirects with another or no `state`
    /// get status 400 and are ignored, including error redirects.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Login`] with [`LoginFailure::TimedOut`] when no matching redirect
    /// arrives, [`LoginFailure::Denied`] when the user denies access,
    /// [`LoginFailure::InvalidIdToken`] when the ID token fails a claim check, and
    /// [`LoginFailure::Provider`] when the provider answers with an error or without an
    /// ID token. Returns the other [`Error`] variants as for every request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(login: imify::auth::LoopbackLogin) -> Result<(), imify::Error> {
    /// use std::time::Duration;
    ///
    /// let credentials = login.finish(Duration::from_secs(300)).await?;
    /// println!("signed in until {:?}", credentials.expires_at());
    /// # Ok(()) }
    /// ```
    pub async fn finish(self, timeout: Duration) -> Result<Credentials, Error> {
        let callback = self.listener.wait(timeout, &self.session.state).await?;
        self.session.complete(callback).await
    }
}

/// Login by pasting the redirect URL from the browser's address bar. The redirect URI
/// uses the first port of [`LoopbackLogin::PORTS`]; nothing listens on it.
///
/// `Debug` prints neither the PKCE verifier, `state` nor `nonce`.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client, pasted: String) -> Result<(), imify::Error> {
/// use imify::auth::PasteLogin;
///
/// let settings = client.oidc_settings().await?;
/// let login = PasteLogin::start(&client, &settings).await?;
/// eprintln!("Open {} and paste the address you are redirected to.", login.authorization_url());
/// let credentials = login.finish(&pasted).await?;
/// # Ok(()) }
/// ```
#[derive(Debug)]
pub struct PasteLogin {
    session: Session,
}

impl PasteLogin {
    /// Reads the provider's discovery document and prepares the authorization request.
    ///
    /// # Errors
    ///
    /// As [`LoopbackLogin::start`], without [`LoginFailure::PortsInUse`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
    /// let settings = imify::OidcSettings::new("https://login.example.com", "client-1");
    /// let login = imify::auth::PasteLogin::start(&client, &settings).await?;
    /// # Ok(()) }
    /// ```
    pub async fn start(client: &Client, settings: &OidcSettings) -> Result<Self, Error> {
        let provider = oidc::discover(client, settings).await?;
        let session = Session::new(client, settings, provider, LoopbackLogin::PORTS[0])?;
        Ok(Self { session })
    }

    /// The URL to open in the browser.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn run(login: imify::auth::PasteLogin) {
    /// eprintln!("Open {}", login.authorization_url());
    /// # }
    /// ```
    #[must_use]
    pub fn authorization_url(&self) -> &str {
        &self.session.authorization_url
    }

    /// Host of the authorization endpoint, for display before the browser opens.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn run(login: imify::auth::PasteLogin) {
    /// eprintln!("Signing in at {}.", login.provider_host());
    /// # }
    /// ```
    #[must_use]
    pub fn provider_host(&self) -> &str {
        &self.session.provider_host
    }

    /// Reads the code and `state` from the pasted redirect URL, then exchanges the code
    /// for tokens. Surrounding whitespace is ignored.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Login`] with [`LoginFailure::InvalidRedirectUrl`] when the input
    /// is not a URL or has neither `code` and `state` nor `error`, and
    /// [`LoginFailure::StateMismatch`] when its `state`, also on an error redirect, is not
    /// this login's. Otherwise as [`LoopbackLogin::finish`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(login: imify::auth::PasteLogin) -> Result<(), imify::Error> {
    /// let credentials = login
    ///     .finish("http://localhost:28753/callback?code=abc&state=xyz")
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub async fn finish(self, redirect_url: &str) -> Result<Credentials, Error> {
        let invalid = |message: &str| Error::Login {
            reason: LoginFailure::InvalidRedirectUrl,
            message: message.to_owned(),
        };
        let url =
            Url::parse(redirect_url.trim()).map_err(|_| invalid("the pasted text is not a URL"))?;
        let callback =
            parse_callback(&url).ok_or_else(|| invalid("the pasted URL has no code and state"))?;
        self.session.complete(callback).await
    }
}

/// One authorization request and the secrets that belong to it.
struct Session {
    client: Client,
    client_id: String,
    provider: Provider,
    redirect_uri: String,
    authorization_url: String,
    provider_host: String,
    code_verifier: String,
    state: String,
    nonce: String,
}

impl fmt::Debug for Session {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Session")
            .field("client_id", &self.client_id)
            .field("provider_host", &self.provider_host)
            .field("redirect_uri", &self.redirect_uri)
            .finish_non_exhaustive()
    }
}

impl Session {
    fn new(
        client: &Client,
        settings: &OidcSettings,
        provider: Provider,
        port: u16,
    ) -> Result<Self, Error> {
        let code_verifier = oidc::random_secret()?;
        let state = oidc::random_secret()?;
        let nonce = oidc::random_secret()?;
        let redirect_uri = format!("http://localhost:{port}{CALLBACK_PATH}");
        let authorization_url = oidc::authorization_url(
            &provider,
            &AuthorizationRequest {
                client_id: &settings.client_id,
                redirect_uri: &redirect_uri,
                state: &state,
                nonce: &nonce,
                code_challenge: &oidc::pkce_challenge(&code_verifier),
            },
        );
        let provider_host = provider
            .authorization_endpoint
            .host_str()
            .unwrap_or_default()
            .to_owned();
        Ok(Self {
            client: client.clone(),
            client_id: settings.client_id.clone(),
            provider,
            redirect_uri,
            authorization_url: authorization_url.into(),
            provider_host,
            code_verifier,
            state,
            nonce,
        })
    }

    async fn complete(self, callback: Callback) -> Result<Credentials, Error> {
        if !callback.has_state(&self.state) {
            return Err(Error::Login {
                reason: LoginFailure::StateMismatch,
                message: "the redirect belongs to another login attempt".to_owned(),
            });
        }
        let code = match callback {
            Callback::Code { code, .. } => code,
            Callback::Error {
                error, description, ..
            } => return Err(provider_error(&error, description)),
        };
        let tokens = oidc::exchange_code(
            &self.client,
            &self.provider.token_endpoint,
            &self.client_id,
            &code,
            &self.code_verifier,
            &self.redirect_uri,
        )
        .await?;
        let id_token = tokens.id_token.ok_or_else(missing_id_token)?;
        let checked = id_token::check(
            &id_token,
            &Expected {
                issuer: &self.provider.issuer,
                client_id: &self.client_id,
                nonce: Some(&self.nonce),
                subject: None,
                now: SystemTime::now(),
            },
        )?;
        Ok(Credentials::new(
            &self.provider,
            self.client_id,
            id_token,
            checked,
            tokens.refresh_token,
        ))
    }
}

fn provider_error(error: &str, description: Option<String>) -> Error {
    let reason = if error == "access_denied" {
        LoginFailure::Denied
    } else {
        LoginFailure::Provider
    };
    let message = match description {
        Some(description) => format!("the identity provider answered {error}: {description}"),
        None => format!("the identity provider answered {error}"),
    };
    Error::Login { reason, message }
}

pub(crate) fn missing_id_token() -> Error {
    Error::Login {
        reason: LoginFailure::Provider,
        message: "the token response has no ID token".to_owned(),
    }
}

/// The version [`Credentials`] serialize with.
const CREDENTIALS_VERSION: u32 = 2;

/// The version before `subject` was recorded. It still deserializes.
const CREDENTIALS_VERSION_WITHOUT_SUBJECT: u32 = 1;

/// ID token, refresh token, user and provider endpoints of one login. `Debug` redacts the
/// tokens.
///
/// The serialized form is a JSON object with `"version": 2`. Version 1, which lacks
/// `subject`, also deserializes; the next refresh records the subject. Deserializing
/// another version fails.
///
/// # Examples
///
/// ```no_run
/// # async fn run(login: imify::auth::LoopbackLogin) -> Result<(), imify::Error> {
/// use std::time::Duration;
///
/// let credentials = login.finish(Duration::from_secs(300)).await?;
/// let json = serde_json::to_string(&credentials).expect("credentials serialize");
/// println!("issued by {}, expires at {:?}", credentials.issuer(), credentials.expires_at());
/// # Ok(()) }
/// ```
#[derive(Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Credentials {
    #[serde(deserialize_with = "supported_version")]
    version: u32,
    issuer: String,
    client_id: String,
    token_endpoint: String,
    revocation_endpoint: String,
    /// The `sub` claim of the ID token. `None` for a login stored as version 1.
    #[serde(default)]
    subject: Option<String>,
    id_token: String,
    /// Unix seconds, from the ID token's `exp` claim.
    id_token_expires_at: u64,
    refresh_token: Option<String>,
}

impl Credentials {
    pub(crate) fn new(
        provider: &Provider,
        client_id: String,
        id_token: String,
        checked: Checked,
        refresh_token: Option<String>,
    ) -> Self {
        Self {
            version: CREDENTIALS_VERSION,
            issuer: provider.issuer.clone(),
            client_id,
            token_endpoint: provider.token_endpoint.to_string(),
            revocation_endpoint: provider.revocation_endpoint.to_string(),
            subject: Some(checked.subject),
            id_token,
            id_token_expires_at: checked.expires_at,
            refresh_token,
        }
    }

    /// The issuer of the ID token.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn run(credentials: imify::auth::Credentials) {
    /// println!("signed in at {}", credentials.issuer());
    /// # }
    /// ```
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// When the ID token expires. A stored expiry that `SystemTime` cannot represent
    /// yields `UNIX_EPOCH`, so the login counts as expired.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn run(credentials: imify::auth::Credentials) {
    /// let expired = credentials.expires_at() <= std::time::SystemTime::now();
    /// # }
    /// ```
    #[must_use]
    pub fn expires_at(&self) -> SystemTime {
        self.expiry().unwrap_or(UNIX_EPOCH)
    }

    /// The expiry as a `SystemTime`; `None` when it overflows.
    fn expiry(&self) -> Option<SystemTime> {
        UNIX_EPOCH.checked_add(Duration::from_secs(self.id_token_expires_at))
    }

    /// Returns `true` when the login can be refreshed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn run(credentials: imify::auth::Credentials) {
    /// if !credentials.has_refresh_token() {
    ///     eprintln!("the login ends when the ID token expires");
    /// }
    /// # }
    /// ```
    #[must_use]
    pub fn has_refresh_token(&self) -> bool {
        self.refresh_token.is_some()
    }

    pub(crate) fn id_token(&self) -> &str {
        &self.id_token
    }

    pub(crate) fn client_id(&self) -> &str {
        &self.client_id
    }

    pub(crate) fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }

    pub(crate) fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    pub(crate) fn token_endpoint(&self) -> Result<Url, Error> {
        oidc::parse_secure_url(&self.token_endpoint, "token endpoint")
    }

    pub(crate) fn revocation_endpoint(&self) -> Result<Url, Error> {
        oidc::parse_secure_url(&self.revocation_endpoint, "revocation endpoint")
    }

    /// Returns `true` when the ID token expires within `margin` of `now`, or when either
    /// point in time overflows.
    pub(crate) fn expires_within(&self, margin: Duration, now: SystemTime) -> bool {
        match (self.expiry(), now.checked_add(margin)) {
            (Some(expiry), Some(limit)) => expiry <= limit,
            _ => true,
        }
    }

    /// Replaces the ID token, its expiry and subject and, when `refresh_token` is `Some`,
    /// the refresh token.
    pub(crate) fn renew(
        &mut self,
        id_token: String,
        checked: Checked,
        refresh_token: Option<String>,
    ) {
        self.version = CREDENTIALS_VERSION;
        self.subject = Some(checked.subject);
        self.id_token = id_token;
        self.id_token_expires_at = checked.expires_at;
        if refresh_token.is_some() {
            self.refresh_token = refresh_token;
        }
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("version", &self.version)
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("token_endpoint", &self.token_endpoint)
            .field("revocation_endpoint", &self.revocation_endpoint)
            .field("subject", &self.subject)
            .field("id_token", &"<redacted>")
            .field("id_token_expires_at", &self.id_token_expires_at)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

fn supported_version<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version == CREDENTIALS_VERSION || version == CREDENTIALS_VERSION_WITHOUT_SUBJECT {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(format!(
            "unsupported credentials version {version}"
        )))
    }
}

#[cfg(test)]
mod tests;
