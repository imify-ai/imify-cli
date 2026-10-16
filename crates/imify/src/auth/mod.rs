//! Access tokens and the sources the client obtains them from.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use crate::Error;

/// Supplies the access token the client sends as `Authorization: Bearer <token>`.
///
/// The client asks for a token before every attempt of a request, so an implementation
/// can refresh an expired token. The `Debug` implementation must not print the token.
///
/// # Errors
///
/// An implementation returns [`Error::Auth`] with `status: None` when it cannot produce a
/// token. The client then fails the request without sending it.
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
    /// Returns [`Error::Auth`] when no token is available.
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
