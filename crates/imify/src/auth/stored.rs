//! A stored login: the credential store seam, the refreshing token source and revocation.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::Mutex;

use super::id_token::{self, Expected};
use super::login::{Credentials, missing_id_token};
use super::oidc::{self, TokenError};
use super::{Token, TokenSource};
use crate::{Client, Error, LoginFailure};

/// The ID token is refreshed when less than this remains before it expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// The OAuth error a token endpoint returns for an expired or revoked refresh token.
const INVALID_GRANT: &str = "invalid_grant";

/// Persists the credentials of one login.
///
/// Methods may block, such as on a keychain prompt; [`StoredLogin`] calls them on a
/// blocking thread. `Debug` must not print the credentials' tokens.
///
/// # Errors
///
/// An implementation returns [`Error::CredentialStore`] when the underlying storage fails.
///
/// # Examples
///
/// ```
/// use std::sync::Mutex;
/// use imify::Error;
/// use imify::auth::{CredentialStore, Credentials};
///
/// #[derive(Debug, Default)]
/// struct InMemory(Mutex<Option<Credentials>>);
///
/// impl CredentialStore for InMemory {
///     fn load(&self) -> Result<Option<Credentials>, Error> {
///         Ok(self.0.lock().map_err(poisoned)?.clone())
///     }
///     fn save(&self, credentials: &Credentials) -> Result<(), Error> {
///         *self.0.lock().map_err(poisoned)? = Some(credentials.clone());
///         Ok(())
///     }
///     fn delete(&self) -> Result<bool, Error> {
///         Ok(self.0.lock().map_err(poisoned)?.take().is_some())
///     }
/// }
///
/// fn poisoned<T>(_: T) -> Error {
///     Error::CredentialStore { source: "lock poisoned".into() }
/// }
/// ```
pub trait CredentialStore: Send + Sync + fmt::Debug {
    /// Returns the stored credentials, `None` when there are none.
    ///
    /// # Errors
    ///
    /// Returns [`Error::CredentialStore`] when the storage cannot be read.
    fn load(&self) -> Result<Option<Credentials>, Error>;

    /// Stores `credentials`, replacing any stored before.
    ///
    /// # Errors
    ///
    /// Returns [`Error::CredentialStore`] when the storage cannot be written.
    fn save(&self, credentials: &Credentials) -> Result<(), Error>;

    /// Removes the stored credentials. Returns `true` when credentials existed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::CredentialStore`] when the storage cannot be changed.
    fn delete(&self) -> Result<bool, Error>;
}

/// A [`TokenSource`] that returns the ID token of a stored login.
///
/// When less than 60 s remain before the ID token expires, it exchanges the refresh token
/// for a new ID token and saves the result in the store. Concurrent requests wait for one
/// refresh. `Debug` does not print the tokens.
///
/// # Examples
///
/// ```no_run
/// # async fn run(store: std::sync::Arc<dyn imify::auth::CredentialStore>) -> Result<(), imify::Error> {
/// use imify::Client;
/// use imify::auth::StoredLogin;
///
/// let Some(login) = StoredLogin::load(store).await? else {
///     eprintln!("not logged in");
///     return Ok(());
/// };
/// let client = Client::builder("https://api.example.com").token_source(login).build()?;
/// let user = client.me().await?;
/// # Ok(()) }
/// ```
#[derive(Debug)]
pub struct StoredLogin {
    store: Arc<dyn CredentialStore>,
    /// Sends the refresh requests to the token endpoint. It has no token source.
    provider_client: Client,
    credentials: Mutex<Credentials>,
}

impl StoredLogin {
    /// Loads the credentials from `store`. Returns `Ok(None)` when the store holds none.
    ///
    /// # Errors
    ///
    /// Returns the store's error, or [`Error::Config`] when the stored token endpoint is not
    /// `https` on a host other than `localhost`, `127.0.0.1` or `::1`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(store: std::sync::Arc<dyn imify::auth::CredentialStore>) -> Result<(), imify::Error> {
    /// let login = imify::auth::StoredLogin::load(store).await?;
    /// println!("logged in: {}", login.is_some());
    /// # Ok(()) }
    /// ```
    pub async fn load(store: Arc<dyn CredentialStore>) -> Result<Option<Self>, Error> {
        let Some(credentials) = on_blocking_thread(&store, |store| store.load()).await? else {
            return Ok(None);
        };
        let provider_client = Client::builder(credentials.token_endpoint()?.as_str()).build()?;
        Ok(Some(Self {
            store,
            provider_client,
            credentials: Mutex::new(credentials),
        }))
    }

    async fn current_token(&self) -> Result<Token, Error> {
        let mut credentials = self.credentials.lock().await;
        if credentials.expires_within(REFRESH_MARGIN, SystemTime::now()) {
            *credentials = self.refresh(&credentials).await?;
        }
        Ok(Token::new(credentials.id_token()))
    }

    /// Returns credentials with a fresh ID token, saved in the store.
    async fn refresh(&self, current: &Credentials) -> Result<Credentials, Error> {
        let Some(refresh_token) = current.refresh_token() else {
            return Err(expired(
                "the stored login has expired and has no refresh token",
            ));
        };
        let response = oidc::refresh(
            &self.provider_client,
            &current.token_endpoint()?,
            current.client_id(),
            refresh_token,
        )
        .await;
        let tokens = match response {
            Ok(tokens) => tokens,
            Err(TokenError::Rejected(rejection)) if rejection.error == INVALID_GRANT => {
                return self
                    .newer_stored(current)
                    .await?
                    .ok_or_else(|| expired("the stored login has expired or was revoked"));
            }
            Err(error) => return Err(error.into()),
        };
        let id_token = tokens.id_token.ok_or_else(missing_id_token)?;
        let checked = id_token::check(
            &id_token,
            &Expected {
                issuer: current.issuer(),
                client_id: current.client_id(),
                nonce: None,
                subject: current.subject(),
                now: SystemTime::now(),
            },
        )?;
        let mut renewed = current.clone();
        renewed.renew(id_token, checked, tokens.refresh_token);
        let saved = renewed.clone();
        on_blocking_thread(&self.store, move |store| store.save(&saved)).await?;
        Ok(renewed)
    }

    /// Credentials another writer saved since `current` was loaded, when their ID token
    /// is still fresh.
    async fn newer_stored(&self, current: &Credentials) -> Result<Option<Credentials>, Error> {
        let stored = on_blocking_thread(&self.store, |store| store.load()).await?;
        Ok(stored.filter(|stored| {
            stored.id_token() != current.id_token()
                && !stored.expires_within(REFRESH_MARGIN, SystemTime::now())
        }))
    }
}

impl TokenSource for StoredLogin {
    fn token(&self) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + '_>> {
        Box::pin(self.current_token())
    }
}

/// The outcome of [`revoke`].
///
/// # Examples
///
/// ```
/// use imify::auth::Revocation;
///
/// let revoked = Revocation::Revoked == Revocation::Revoked;
/// assert!(revoked);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revocation {
    /// The identity provider revoked the refresh token.
    Revoked,
    /// The revocation endpoint answered 404, or the credentials hold no refresh token.
    Unsupported,
}

/// Revokes the refresh token of `credentials` (RFC 7009) at the revocation endpoint
/// recorded at login. Sends no `Authorization` header.
///
/// ID tokens issued before stay valid until they expire.
///
/// # Errors
///
/// Returns [`Error::Login`] with [`LoginFailure::Provider`] when the endpoint answers with
/// an OAuth error, and the other [`Error`] variants as for every request.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client, credentials: imify::auth::Credentials) -> Result<(), imify::Error> {
/// use imify::auth::{Revocation, revoke};
///
/// if revoke(&client, &credentials).await? == Revocation::Unsupported {
///     eprintln!("the refresh token was not revoked");
/// }
/// # Ok(()) }
/// ```
pub async fn revoke(client: &Client, credentials: &Credentials) -> Result<Revocation, Error> {
    let Some(refresh_token) = credentials.refresh_token() else {
        return Ok(Revocation::Unsupported);
    };
    oidc::revoke(
        client,
        &credentials.revocation_endpoint()?,
        credentials.client_id(),
        refresh_token,
    )
    .await
}

fn expired(message: &str) -> Error {
    Error::Login {
        reason: LoginFailure::Expired,
        message: message.to_owned(),
    }
}

async fn on_blocking_thread<T: Send + 'static>(
    store: &Arc<dyn CredentialStore>,
    operation: impl FnOnce(&dyn CredentialStore) -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    let store = Arc::clone(store);
    tokio::task::spawn_blocking(move || operation(store.as_ref()))
        .await
        .map_err(|error| Error::CredentialStore {
            source: Box::new(error),
        })?
}

#[cfg(test)]
mod tests;
