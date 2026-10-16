//! The authenticated account.

use reqwest::Method;

use crate::{Client, Error};

/// The user the access token belongs to. Unknown response fields are ignored.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
/// let user: imify::User = client.me().await?;
/// println!("{} ({})", user.name, user.role);
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[non_exhaustive]
pub struct User {
    /// Stable user id: the `sub` claim of the access token.
    pub subject: String,
    /// Display name or email address.
    pub name: String,
    /// Platform role, such as `admin`.
    pub role: String,
}

impl Client {
    /// Returns the user the access token belongs to (`GET /api/v1/me`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Auth`] when the token source produces no token or the API rejects
    /// the token. Returns the other [`Error`] variants as for every request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), imify::Error> {
    /// use imify::Client;
    /// use imify::auth::StaticToken;
    ///
    /// let client = Client::builder("https://api.example.com")
    ///     .token_source(StaticToken::new("secret"))
    ///     .build()?;
    /// let user = client.me().await?;
    /// println!("signed in as {}", user.subject);
    /// # Ok(()) }
    /// ```
    pub async fn me(&self) -> Result<User, Error> {
        self.execute(Method::GET, "/api/v1/me", None).await
    }
}

#[cfg(test)]
mod tests;
