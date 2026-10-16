//! Access tokens and the sources the client obtains them from.
//!
//! [`EnvToken`] reads `IMIFY_TOKEN`. [`StaticToken`] returns a fixed token. [`StoredLogin`]
//! returns the ID token of a browser login made with [`LoopbackLogin`] or [`PasteLogin`]
//! and kept in a [`CredentialStore`]. Other sources implement [`TokenSource`].
//!
//! ```no_run
//! # async fn run(store: std::sync::Arc<dyn imify::auth::CredentialStore>) -> Result<(), imify::Error> {
//! use std::time::Duration;
//! use imify::Client;
//! use imify::auth::{LoopbackLogin, StoredLogin};
//!
//! let client = Client::builder("https://api.example.com").build()?;
//! let settings = client.oidc_settings().await?;
//! let login = LoopbackLogin::start(&client, &settings).await?;
//! eprintln!("Open {}", login.authorization_url());
//! let credentials = login.finish(Duration::from_secs(300)).await?;
//! store.save(&credentials)?;
//!
//! let source = StoredLogin::load(store).await?.expect("credentials were saved");
//! let client = Client::builder("https://api.example.com").token_source(source).build()?;
//! # Ok(()) }
//! ```

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::Error;

mod env;
mod id_token;
mod login;
mod loopback;
mod oidc;
mod stored;
#[cfg(test)]
mod testing;

pub use env::EnvToken;
pub use login::{Credentials, LoopbackLogin, PasteLogin};
pub use stored::{CredentialStore, Revocation, StoredLogin, revoke};

/// Supplies the access token the client sends as `Authorization: Bearer <token>`.
///
/// The client asks for a token before every attempt of a request, so an implementation
/// can refresh an expired token. The `Debug` implementation must not print the token.
///
/// # Errors
///
/// An implementation returns [`Error::Auth`] with `status: None` when it has no token, and
/// [`Error::Login`] when a stored login cannot be refreshed. The client then fails the
/// request without sending it.
///
/// # Examples
///
/// ```
/// use std::future::Future;
/// use std::pin::Pin;
/// use imify::Error;
/// use imify::auth::{Token, TokenSource};
///
/// #[derive(Debug)]
/// struct FromVault;
///
/// impl TokenSource for FromVault {
///     fn token(&self) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + '_>> {
///         Box::pin(async { Ok(Token::new("token-from-vault")) })
///     }
/// }
///
/// let client = imify::Client::builder("https://api.example.com")
///     .token_source(FromVault)
///     .build()?;
/// # Ok::<(), imify::Error>(())
/// ```
pub trait TokenSource: Send + Sync + fmt::Debug {
    /// Returns the token for the next request.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Auth`] when no token is available, or [`Error::Login`] when a
    /// refresh fails.
    fn token(&self) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + '_>>;
}

/// An access token. `Debug` prints `Token(<redacted>)`.
///
/// # Examples
///
/// ```
/// use imify::auth::Token;
///
/// let token = Token::new("secret");
/// assert_eq!(format!("{token:?}"), "Token(<redacted>)");
/// ```
#[derive(Clone)]
pub struct Token(String);

impl Token {
    /// Wraps a token value.
    ///
    /// # Examples
    ///
    /// ```
    /// let token = imify::auth::Token::new("secret");
    /// ```
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The `exp` claim when the token is a JWT, read without verifying the signature.
    /// `None` for an opaque token or a JWT without a numeric `exp`.
    ///
    /// # Examples
    ///
    /// ```
    /// let token = imify::auth::Token::new("opaque-token");
    /// assert_eq!(token.expires_at(), None);
    /// ```
    #[must_use]
    pub fn expires_at(&self) -> Option<SystemTime> {
        #[derive(serde::Deserialize)]
        struct Expiry {
            exp: u64,
        }
        let expiry: Expiry = id_token::payload(&self.0).ok()?;
        UNIX_EPOCH.checked_add(Duration::from_secs(expiry.exp))
    }

    pub(crate) fn secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Token(<redacted>)")
    }
}

/// A token source that returns the same token on every call.
///
/// # Examples
///
/// ```
/// use imify::auth::StaticToken;
///
/// let client = imify::Client::builder("https://api.example.com")
///     .token_source(StaticToken::new("secret"))
///     .build()?;
/// # Ok::<(), imify::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct StaticToken {
    token: Token,
}

impl StaticToken {
    /// Creates a source that always returns `token`.
    ///
    /// # Examples
    ///
    /// ```
    /// let source = imify::auth::StaticToken::new("secret");
    /// ```
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: Token::new(token),
        }
    }
}

impl TokenSource for StaticToken {
    fn token(&self) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + '_>> {
        let token = self.token.clone();
        Box::pin(async move { Ok(token) })
    }
}
