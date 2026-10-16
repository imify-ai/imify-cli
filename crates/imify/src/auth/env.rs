//! A token source that reads `IMIFY_TOKEN`.

use std::ffi::OsString;
use std::future::Future;
use std::pin::Pin;

use super::{Token, TokenSource};
use crate::Error;

/// Reads the access token from the `IMIFY_TOKEN` environment variable. `Debug` does not
/// print the token.
///
/// The variable is read once, when the source is created.
///
/// # Examples
///
/// ```
/// use imify::Client;
/// use imify::auth::EnvToken;
///
/// let mut builder = Client::builder("https://api.example.com");
/// if let Some(source) = EnvToken::from_env()? {
///     builder = builder.token_source(source);
/// }
/// let client = builder.build()?;
/// # Ok::<(), imify::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct EnvToken {
    token: Token,
}

impl EnvToken {
    /// The environment variable this source reads.
    pub const VARIABLE: &'static str = "IMIFY_TOKEN";

    /// Reads `IMIFY_TOKEN` and trims surrounding whitespace from its value.
    ///
    /// Returns `Ok(None)` when the variable is unset or empty after trimming.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] when the value is not valid Unicode. The message names the
    /// variable, not the value.
    ///
    /// # Examples
    ///
    /// ```
    /// match imify::auth::EnvToken::from_env()? {
    ///     Some(_) => println!("IMIFY_TOKEN is set"),
    ///     None => println!("IMIFY_TOKEN is not set"),
    /// }
    /// # Ok::<(), imify::Error>(())
    /// ```
    pub fn from_env() -> Result<Option<Self>, Error> {
        Self::from_value(std::env::var_os(Self::VARIABLE))
    }

    fn from_value(value: Option<OsString>) -> Result<Option<Self>, Error> {
        let Some(value) = value else {
            return Ok(None);
        };
        let value = value.into_string().map_err(|_| Error::Config {
            message: format!("{} is not valid Unicode", Self::VARIABLE),
        })?;
        let value = value.trim();
        if value.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            token: Token::new(value),
        }))
    }
}

impl TokenSource for EnvToken {
    fn token(&self) -> Pin<Box<dyn Future<Output = Result<Token, Error>> + Send + '_>> {
        let token = self.token.clone();
        Box::pin(async move { Ok(token) })
    }
}

#[cfg(test)]
mod tests;
