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
use crate::credentials::{HostStore, Keychain, SaveReport, Saved, host_key};
use crate::output::renderer::{Render, Renderer};
use crate::output::sanitize::printable;
use crate::output::{
    AUTH_REJECTED_HINT, CliError, Environment, ErrorCode, LOGIN_HINT, PASTE_HINT, UNSET_TOKEN_HINT,
};

/// How long `imify auth login` waits for the browser to return to the CLI.
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
    /// Replaces the hint of an `auth_rejected` error: [`LOGIN_HINT`] when the token comes from
    /// the stored login, the hint of [`LoginLookup::env_token_hint`] when it comes from
    /// `IMIFY_TOKEN`. Reads the credential store only for an `auth_rejected` error with a token
    /// from `IMIFY_TOKEN`. Other errors keep their hint.
    pub async fn adjust(self, error: CliError, lookup: LoginLookup) -> CliError {
        if error.code != ErrorCode::AuthRejected {
            return error;
        }
        match self {
            Self::Login => error.with_hint(LOGIN_HINT),
            Self::Env => error.with_hint(lookup.env_token_hint().await),
        }
    }
}

/// The stored login of one host, read only when [`LoginLookup::exists`] runs.
#[derive(Debug)]
pub struct LoginLookup {
    env: Environment,
    host: String,
}

impl LoginLookup {
    /// The lookup for the login of `host`, a host key. Reads nothing.
    pub fn new(env: &Environment, host: &str) -> Self {
        Self {
            env: env.clone(),
            host: host.to_owned(),
        }
    }

    /// `true` when a login is stored for the host. A store that cannot be opened or read,
    /// including an unsupported `IMIFY_CREDENTIAL_STORE`, is `false`; its error is logged at
    /// debug level.
    pub async fn exists(self) -> bool {
        let store = match HostStore::open(&self.env, &self.host, Keychain::system()) {
            Ok(store) => store,
            Err(error) => {
                tracing::debug!(
                    message = %error.message,
                    "cannot open the credential store to look for a stored login"
                );
                return false;
            }
        };
        match on_blocking_thread(move || store.load()).await {
            Ok(login) => login.is_some(),
            Err(error) => {
                tracing::debug!(
                    message = %error.message,
                    "cannot read the credential store to look for a stored login"
                );
                false
            }
        }
    }

    /// The hint for an `auth_rejected` error with a token from `IMIFY_TOKEN`:
    /// [`UNSET_TOKEN_HINT`] when a login is stored for the host, else [`AUTH_REJECTED_HINT`].
    pub async fn env_token_hint(self) -> &'static str {
        if self.exists().await {
            UNSET_TOKEN_HINT
        } else {
            AUTH_REJECTED_HINT
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

/// `imify auth whoami`: prints the user the token belongs to.
///
/// # Errors
///
/// Returns the error of building the client, of the API request, or of writing the result.
pub async fn whoami(env: &Environment, renderer: &mut dyn Renderer) -> Result<(), CliError> {
    let api = client(env).await?;
    let user = match api.client.me().await {
        Ok(user) => user,
        Err(error) => return Err(api.error(error).await),
    };
    write_result(renderer, &Whoami(user))
}

/// The result of `imify auth whoami`.
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
        writeln!(w, "Name     {}", printable(&self.0.name))?;
        writeln!(w, "Subject  {}", printable(&self.0.subject))?;
        writeln!(w, "Role     {}", printable(&self.0.role))
    }
}

/// What `imify auth login` uses besides the API and the identity provider.
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

/// `imify auth login`: signs in through the system browser, or by reading the pasted redirect URL
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

/// Signs in to the host of the API URL, confirms the account with the API and stores the
/// login. The API URL is `--api-url`, else `IMIFY_API_URL`, else `api_url` in the config
/// file, else `https://platform.imify.ai`. Writes the API host, the identity provider's host
/// and the authorization URL to `prompt` before it opens the browser or prompts for the
/// pasted URL. Writes the warning about a login stored in the file because no keychain
/// service exists to `prompt`, also with `--quiet`.
///
/// # Errors
///
/// Returns `invalid_argument` for an invalid API URL or `IMIFY_CREDENTIAL_STORE`, for
/// `--no-browser` without a terminal on stdin, when the host publishes no identity provider
/// settings and the overrides are unset, and for an invalid pasted URL. Returns the config
/// file's error when it cannot be read, `login_failed` when the login does not complete, and
/// the mapped SDK and store errors otherwise.
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
    let sign_in = SignIn {
        client: &client,
        settings: &settings,
        host: &host,
    };
    let credentials = if no_browser {
        paste_login(&sign_in, context.prompt, context.read_pasted_url).await?
    } else {
        browser_login(&sign_in, context.browser, context.prompt, renderer).await?
    };
    let user = confirm_account(&api_url, &credentials).await?;
    let report = save(store, credentials).await?;
    warn_about_the_stored_login(context.prompt, renderer, &report, context.env_token_set)?;
    write_result(
        renderer,
        &LoginResult {
            host,
            user,
            saved: report.saved,
        },
    )
}

/// One sign-in: the client for the identity provider requests, its settings and the API host.
struct SignIn<'a> {
    client: &'a imify::Client,
    settings: &'a OidcSettings,
    host: &'a str,
}

impl SignIn<'_> {
    /// The first line before the sign-in: which API the login is for and where the user signs
    /// in. Both hosts come from the API's settings and the identity provider.
    fn intro(&self, provider_host: &str) -> String {
        format!(
            "Signing in to {} through {}.",
            printable(self.host),
            printable(provider_host)
        )
    }
}

fn no_terminal_error() -> CliError {
    CliError::new(
        ErrorCode::InvalidArgument,
        "`imify auth login --no-browser` reads the redirect URL from a terminal, and stdin is not one.",
    )
    .with_hint("Run `imify auth login --no-browser` in a terminal, or set `IMIFY_TOKEN` to a token.")
}

/// Warns when the login went to the file because no keychain service exists (on `prompt`,
/// also with `--quiet`), when an older login could not be removed from the file, and when
/// `IMIFY_TOKEN` takes precedence over the login.
fn warn_about_the_stored_login(
    prompt: &mut dyn Write,
    renderer: &mut dyn Renderer,
    report: &SaveReport,
    env_token_set: bool,
) -> Result<(), CliError> {
    if let Saved::FileFallback { path, reason } = &report.saved {
        writeln!(
            prompt,
            "warning: The login is stored unencrypted in {}, because {}.",
            path.display(),
            printable(reason)
        )
        .and_then(|()| prompt.flush())
        .map_err(|error| prompt_error(&error))?;
    }
    if let Some(failure) = &report.cleanup_failure {
        warn(
            renderer,
            &format!("Cannot remove the older login from the credentials file: {failure}"),
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
    sign_in: &SignIn<'_>,
    browser: &dyn Browser,
    prompt: &mut dyn Write,
    renderer: &mut dyn Renderer,
) -> Result<Credentials, CliError> {
    let login = LoopbackLogin::start(sign_in.client, sign_in.settings).await?;
    writeln!(
        prompt,
        "{} Opening the browser.",
        sign_in.intro(login.provider_host())
    )
    .and_then(|()| writeln!(prompt, "{}", printable(login.authorization_url())))
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
    sign_in: &SignIn<'_>,
    prompt: &mut dyn Write,
    read_pasted_url: &mut dyn FnMut() -> io::Result<String>,
) -> Result<Credentials, CliError> {
    let login = PasteLogin::start(sign_in.client, sign_in.settings).await?;
    writeln!(
        prompt,
        "{} Open this URL in a browser:",
        sign_in.intro(login.provider_host())
    )
    .and_then(|()| writeln!(prompt, "{}", printable(login.authorization_url())))
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
    if error.kind() == io::ErrorKind::Interrupted {
        return CliError::interrupted();
    }
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
async fn save(store: HostStore, credentials: Credentials) -> Result<SaveReport, CliError> {
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

/// The result of `imify auth login`.
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
            "storage_fallback": matches!(self.saved, Saved::FileFallback { .. }),
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(
            w,
            "Logged in to {} as {} ({}).",
            self.host,
            printable(&self.user.name),
            printable(&self.user.role)
        )?;
        match &self.saved {
            Saved::Keychain => writeln!(w, "Credentials stored in the keychain."),
            Saved::File(path) | Saved::FileFallback { path, .. } => {
                writeln!(w, "Credentials stored in {}.", path.display())
            }
        }
    }
}

/// `imify auth logout`: revokes the refresh token of the stored login and removes the login.
///
/// # Errors
///
/// As [`logout_with`].
pub async fn logout(env: &Environment, renderer: &mut dyn Renderer) -> Result<(), CliError> {
    logout_with(env, Keychain::system(), renderer).await
}

/// Revokes the refresh token of the stored login for the host of the API URL and removes the
/// login from every store. The API URL resolves as in [`login_with`]. A login that cannot be
/// read and a failed revocation are warnings; the login is removed regardless.
///
/// # Errors
///
/// Returns `invalid_argument` for an invalid API URL or `IMIFY_CREDENTIAL_STORE`, the config
/// file's error when it cannot be read, and the store's error when the login cannot be
/// removed.
pub async fn logout_with(
    env: &Environment,
    keychain: Keychain,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    let api_url = api_url(env)?;
    let host = host_key(&api_url);
    let store = Arc::new(HostStore::open(env, &host, keychain)?);
    let loading = Arc::clone(&store);
    let revoked = match on_blocking_thread(move || loading.load()).await {
        Ok(Some(credentials)) => revoke(&api_url, &credentials, renderer).await?,
        Ok(None) => false,
        Err(error) => {
            warn(
                renderer,
                &format!(
                    "Cannot read the stored login, so its refresh token is not revoked: {}",
                    error.message
                ),
            )?;
            false
        }
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

/// The result of `imify auth logout`.
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
