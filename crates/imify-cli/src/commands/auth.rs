//! Account commands and the resolution of credentials.

use std::fmt;
use std::future::Future;
use std::io::{self, Write};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use imify::auth::{
    CredentialStore, Credentials, EnvToken, LoopbackLogin, PasteLogin, Revocation, StoredLogin,
    Token, TokenSource,
};
use imify::{OidcSettings, User};
use url::Url;

use super::{api_url, client, write_result};
use crate::browser::{Browser, SystemBrowser};
use crate::credentials::{HostStore, Keychain, Saved, host_key};
use crate::output::renderer::{Render, Renderer};
use crate::output::{CliError, Environment, ErrorCode, LOGIN_HINT, PASTE_HINT};

/// How long `imify login` waits for the browser to return to the CLI.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

const ISSUER_VARIABLE: &str = "IMIFY_OIDC_ISSUER";
const CLIENT_ID_VARIABLE: &str = "IMIFY_OIDC_CLIENT_ID";

/// Where the token of an API client comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenOrigin {
    /// `IMIFY_TOKEN`.
    Env,
    /// The stored login of the host.
    Login,
}

impl TokenOrigin {
    /// Replaces the hint of an `auth_rejected` error when the token comes from a login.
    pub fn adjust(self, error: CliError) -> CliError {
        if self == Self::Login && error.code == ErrorCode::AuthRejected {
            error.with_hint(LOGIN_HINT)
        } else {
            error
        }
    }
}

/// The token source of an API client: `IMIFY_TOKEN` or the stored login.
#[derive(Debug)]
pub enum AuthSource {
    Env(EnvToken),
    Stored(Box<StoredLogin>),
}

impl TokenSource for AuthSource {
    fn token(&self) -> Pin<Box<dyn Future<Output = Result<Token, imify::Error>> + Send + '_>> {
        match self {
            Self::Env(source) => source.token(),
            Self::Stored(source) => source.token(),
        }
    }
}

/// Resolves where the token for `host` comes from: `IMIFY_TOKEN` when it is set and not
/// empty, otherwise the stored login of `host`. `None` when there is neither.
///
/// # Errors
///
/// Returns `invalid_argument` when `IMIFY_TOKEN` is not valid Unicode or
/// `IMIFY_CREDENTIAL_STORE` is unsupported, and the store's error when it cannot be read.
pub async fn token_source(
    env: &Environment,
    host: &str,
) -> Result<Option<(AuthSource, TokenOrigin)>, CliError> {
    if let Some(source) = env_token_source()? {
        return Ok(Some(source));
    }
    let store = HostStore::open(env, host, Keychain::system())?;
    let login = StoredLogin::load(Arc::new(store)).await?;
    Ok(login.map(|login| (AuthSource::Stored(Box::new(login)), TokenOrigin::Login)))
}

/// The token source of `IMIFY_TOKEN`, when it is set and not empty.
///
/// # Errors
///
/// Returns `invalid_argument` when `IMIFY_TOKEN` is not valid Unicode.
pub fn env_token_source() -> Result<Option<(AuthSource, TokenOrigin)>, CliError> {
    Ok(EnvToken::from_env()?.map(|token| (AuthSource::Env(token), TokenOrigin::Env)))
}

/// `imify whoami`: prints the user the token belongs to.
///
/// # Errors
///
/// Returns the error of building the client, of the API request, or of writing the result.
pub async fn whoami(env: &Environment, renderer: &mut dyn Renderer) -> Result<(), CliError> {
    let api = client(env).await?;
    let user = api.client.me().await.map_err(|error| api.error(error))?;
    write_result(renderer, &Whoami(user))
}

/// The result of `imify whoami`.
struct Whoami(User);

impl Render for Whoami {
    fn kind(&self) -> &'static str {
        "user"
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "subject": self.0.subject,
            "name": self.0.name,
            "role": self.0.role,
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "Name     {}", self.0.name)?;
        writeln!(w, "Subject  {}", self.0.subject)?;
        writeln!(w, "Role     {}", self.0.role)
    }
}

/// What `imify login` uses besides the API and the identity provider.
pub struct LoginContext<'a> {
    pub browser: &'a dyn Browser,
    pub keychain: Keychain,
    /// Receives the sign-in instructions and the paste prompt.
    pub prompt: &'a mut dyn Write,
    /// Reads the redirect URL the user pastes with `--no-browser`.
    pub read_pasted_url: &'a mut dyn FnMut() -> io::Result<String>,
    /// Whether `IMIFY_TOKEN` is set.
    pub env_token_set: bool,
}

impl fmt::Debug for LoginContext<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoginContext")
            .field("keychain", &self.keychain)
            .field("env_token_set", &self.env_token_set)
            .finish_non_exhaustive()
    }
}

/// `imify login`: signs in through the system browser, or by reading the pasted redirect URL
/// from stdin with `--no-browser`, and stores the login of the host.
///
/// # Errors
///
/// As [`login_with`].
pub async fn login(
    env: &Environment,
    no_browser: bool,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    let mut stderr = io::stderr();
    let mut read_pasted_url = || crate::output::paste::read_line(io::stdin());
    let context = LoginContext {
        browser: &SystemBrowser,
        keychain: Keychain::system(),
        prompt: &mut stderr,
        read_pasted_url: &mut read_pasted_url,
        env_token_set: EnvToken::from_env()?.is_some(),
    };
    login_with(env, no_browser, context, renderer).await
}

/// Signs in to the host in `IMIFY_API_URL`, confirms the account with the API and stores the
/// login. Writes the identity provider's host and the authorization URL to `prompt` before
/// it opens the browser.
///
/// # Errors
///
/// Returns `invalid_argument` for an invalid `IMIFY_API_URL` or `IMIFY_CREDENTIAL_STORE`,
/// for `--no-browser` without a terminal on stdin, when the host publishes no identity
/// provider settings and the overrides are unset, and for an invalid pasted URL. Returns
/// `login_failed` when the login does not complete, and the mapped SDK and store errors
/// otherwise.
pub async fn login_with(
    env: &Environment,
    no_browser: bool,
    context: LoginContext<'_>,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    let api_url = api_url(env)?;
    let host = host_key(&api_url);
    let store = HostStore::open(env, &host, context.keychain)?;
    if no_browser && !env.stdin_is_tty {
        return Err(no_terminal_error());
    }
    let client = unauthenticated_client(&api_url)?;
    let settings = oidc_settings(env, &client, &host).await?;
    let credentials = if no_browser {
        paste_login(&client, &settings, context.prompt, context.read_pasted_url).await?
    } else {
        browser_login(
            &client,
            &settings,
            context.browser,
            context.prompt,
            renderer,
        )
        .await?
    };
    let user = confirm_account(&api_url, &credentials).await?;
    let saved = save(store, credentials).await?;
    warn_about_the_stored_login(renderer, &saved, context.env_token_set)?;
    write_result(renderer, &LoginResult { host, user, saved })
}

fn no_terminal_error() -> CliError {
    CliError::new(
        ErrorCode::InvalidArgument,
        "`imify login --no-browser` reads the redirect URL from a terminal, and stdin is not one.",
    )
    .with_hint("Run `imify login --no-browser` in a terminal, or set `IMIFY_TOKEN` to a token.")
}

/// Warns when the login went to the file because no keychain is available, and when
/// `IMIFY_TOKEN` takes precedence over it.
fn warn_about_the_stored_login(
    renderer: &mut dyn Renderer,
    saved: &Saved,
    env_token_set: bool,
) -> Result<(), CliError> {
    if let Saved::FileFallback { path, reason } = saved {
        warn(
            renderer,
            &format!(
                "The login is stored in {}, because {reason}.",
                path.display()
            ),
        )?;
    }
    if env_token_set {
        warn(
            renderer,
            "IMIFY_TOKEN is set and takes precedence over the stored login.",
        )?;
    }
    Ok(())
}

fn unauthenticated_client(api_url: &Url) -> Result<imify::Client, CliError> {
    Ok(imify::Client::builder(api_url.as_str()).build()?)
}

/// The identity provider settings: the overrides, completed from the host's `config.json`
/// when one of them is unset.
async fn oidc_settings(
    env: &Environment,
    client: &imify::Client,
    host: &str,
) -> Result<OidcSettings, CliError> {
    let non_empty = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let issuer = non_empty(&env.oidc_issuer);
    let client_id = non_empty(&env.oidc_client_id);
    if let (Some(issuer), Some(client_id)) = (&issuer, &client_id) {
        return Ok(OidcSettings::new(issuer, client_id));
    }
    let published = client.oidc_settings().await.map_err(|error| match error {
        imify::Error::NotFound { .. } | imify::Error::UnexpectedResponse { .. } => CliError::new(
            ErrorCode::InvalidArgument,
            format!("{host} publishes no identity provider settings in config.json."),
        )
        .with_hint(format!(
            "Set `{ISSUER_VARIABLE}` and `{CLIENT_ID_VARIABLE}`."
        )),
        other => other.into(),
    })?;
    Ok(OidcSettings::new(
        issuer.unwrap_or(published.issuer),
        client_id.unwrap_or(published.client_id),
    ))
}

async fn browser_login(
    client: &imify::Client,
    settings: &OidcSettings,
    browser: &dyn Browser,
    prompt: &mut dyn Write,
    renderer: &mut dyn Renderer,
) -> Result<Credentials, CliError> {
    let login = LoopbackLogin::start(client, settings).await?;
    writeln!(
        prompt,
        "Signing in at {}. Opening the browser.",
        login.provider_host()
    )
    .and_then(|()| writeln!(prompt, "{}", login.authorization_url()))
    .and_then(|()| prompt.flush())
    .map_err(|error| prompt_error(&error))?;
    if let Err(error) = browser.open(login.authorization_url()) {
        warn(
            renderer,
            &format!("Cannot open the browser: {error}. Open the URL above."),
        )?;
    }
    Ok(login.finish(LOGIN_TIMEOUT).await?)
}

async fn paste_login(
    client: &imify::Client,
    settings: &OidcSettings,
    prompt: &mut dyn Write,
    read_pasted_url: &mut dyn FnMut() -> io::Result<String>,
) -> Result<Credentials, CliError> {
    let login = PasteLogin::start(client, settings).await?;
    writeln!(
        prompt,
        "Signing in at {}. Open this URL in a browser:",
        login.provider_host()
    )
    .and_then(|()| writeln!(prompt, "{}", login.authorization_url()))
    .and_then(|()| write!(prompt, "Paste the URL from the browser's address bar: "))
    .and_then(|()| prompt.flush())
    .map_err(|error| prompt_error(&error))?;
    let pasted = read_pasted_url().map_err(|error| pasted_url_error(&error))?;
    Ok(login.finish(&pasted).await?)
}

fn prompt_error(error: &io::Error) -> CliError {
    CliError::new(
        ErrorCode::Internal,
        format!("Cannot write the sign-in instructions: {error}"),
    )
}

fn pasted_url_error(error: &io::Error) -> CliError {
    let code = match error.kind() {
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof => {
            ErrorCode::InvalidArgument
        }
        _ => ErrorCode::Internal,
    };
    CliError::new(code, format!("Cannot read the pasted URL: {error}.")).with_hint(PASTE_HINT)
}

/// Asks the API for the user the new login belongs to.
async fn confirm_account(api_url: &Url, credentials: &Credentials) -> Result<User, CliError> {
    let pending = Arc::new(PendingLogin(Mutex::new(Some(credentials.clone()))));
    let source = StoredLogin::load(pending).await?.ok_or_else(|| {
        CliError::new(ErrorCode::Internal, "The new login could not be read back.")
    })?;
    let client = imify::Client::builder(api_url.as_str())
        .token_source(source)
        .build()?;
    Ok(client.me().await?)
}

/// Holds a login that is not stored yet.
#[derive(Debug)]
struct PendingLogin(Mutex<Option<Credentials>>);

impl PendingLogin {
    fn credentials(&self) -> Result<std::sync::MutexGuard<'_, Option<Credentials>>, imify::Error> {
        self.0.lock().map_err(|_| imify::Error::CredentialStore {
            source: "the pending login is poisoned".into(),
        })
    }
}

impl CredentialStore for PendingLogin {
    fn load(&self) -> Result<Option<Credentials>, imify::Error> {
        Ok(self.credentials()?.clone())
    }

    fn save(&self, credentials: &Credentials) -> Result<(), imify::Error> {
        *self.credentials()? = Some(credentials.clone());
        Ok(())
    }

    fn delete(&self) -> Result<bool, imify::Error> {
        Ok(self.credentials()?.take().is_some())
    }
}

/// Saves `credentials` on a blocking thread: a keychain may block, and the Secret Service
/// client runs its own event loop.
async fn save(store: HostStore, credentials: Credentials) -> Result<Saved, CliError> {
    on_blocking_thread(move || store.save_reporting(&credentials)).await
}

async fn on_blocking_thread<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, imify::Error> + Send + 'static,
) -> Result<T, CliError> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| {
            CliError::new(
                ErrorCode::Internal,
                format!("The credential store stopped: {error}"),
            )
        })?
        .map_err(CliError::from)
}

fn warn(renderer: &mut dyn Renderer, message: &str) -> Result<(), CliError> {
    renderer.warning(message).map_err(|error| {
        CliError::new(
            ErrorCode::Internal,
            format!("Cannot write a warning: {error}"),
        )
    })
}

/// The result of `imify login`.
struct LoginResult {
    host: String,
    user: User,
    saved: Saved,
}

impl Render for LoginResult {
    fn kind(&self) -> &'static str {
        "login"
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "host": self.host,
            "subject": self.user.subject,
            "name": self.user.name,
            "role": self.user.role,
            "storage": self.saved.storage(),
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(
            w,
            "Logged in to {} as {} ({}).",
            self.host, self.user.name, self.user.role
        )?;
        match &self.saved {
            Saved::Keychain => writeln!(w, "Credentials stored in the keychain."),
            Saved::File(path) | Saved::FileFallback { path, .. } => {
                writeln!(w, "Credentials stored in {}.", path.display())
            }
        }
    }
}

/// `imify logout`: revokes the refresh token of the stored login and removes the login.
///
/// # Errors
///
/// As [`logout_with`].
pub async fn logout(env: &Environment, renderer: &mut dyn Renderer) -> Result<(), CliError> {
    logout_with(env, Keychain::system(), renderer).await
}

/// Revokes the refresh token of the host's stored login and removes the login from every
/// store. A failed revocation is a warning; the login is removed regardless.
///
/// # Errors
///
/// Returns `invalid_argument` for an invalid `IMIFY_API_URL` or `IMIFY_CREDENTIAL_STORE`,
/// and the store's error when it cannot be read or changed.
pub async fn logout_with(
    env: &Environment,
    keychain: Keychain,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    let api_url = api_url(env)?;
    let host = host_key(&api_url);
    let store = Arc::new(HostStore::open(env, &host, keychain)?);
    let loading = Arc::clone(&store);
    let credentials = on_blocking_thread(move || loading.load()).await?;
    let revoked = match &credentials {
        Some(credentials) => revoke(&api_url, credentials, renderer).await?,
        None => false,
    };
    let removed = on_blocking_thread(move || store.delete()).await?;
    write_result(
        renderer,
        &LogoutResult {
            host,
            removed,
            revoked,
        },
    )
}

/// Revokes the refresh token and reports whether the identity provider confirmed it.
async fn revoke(
    api_url: &Url,
    credentials: &Credentials,
    renderer: &mut dyn Renderer,
) -> Result<bool, CliError> {
    if !credentials.has_refresh_token() {
        return Ok(false);
    }
    let client = unauthenticated_client(api_url)?;
    match imify::auth::revoke(&client, credentials).await {
        Ok(Revocation::Revoked) => Ok(true),
        Ok(Revocation::Unsupported) => {
            warn(
                renderer,
                "The identity provider does not support revocation. The refresh token stays valid until it expires.",
            )?;
            Ok(false)
        }
        Err(error) => {
            warn(
                renderer,
                &format!("Cannot revoke the refresh token: {error}"),
            )?;
            Ok(false)
        }
    }
}

/// The result of `imify logout`.
struct LogoutResult {
    host: String,
    removed: bool,
    revoked: bool,
}

impl Render for LogoutResult {
    fn kind(&self) -> &'static str {
        "logout"
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "host": self.host,
            "removed": self.removed,
            "revoked": self.revoked,
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        if !self.removed {
            return writeln!(w, "Not logged in to {}.", self.host);
        }
        writeln!(w, "Logged out of {}.", self.host)?;
        writeln!(
            w,
            "Tokens issued before the logout stay valid until they expire."
        )
    }
}

#[cfg(test)]
mod tests;
