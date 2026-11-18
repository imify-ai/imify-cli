//! The authenticated account.

use serde::Deserialize;

use crate::resource::{self, last_segment};
use crate::{Client, Error};

/// The user the access token belongs to. Unknown response fields are ignored.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
/// let user: imify::User = client.me().await?;
/// println!("{} ({})", user.name, user.role.as_deref().unwrap_or("no access"));
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "WireUser")]
#[non_exhaustive]
pub struct User {
    /// Stable user id: the `sub` claim of the access token, the `{u}` of the resource
    /// name `users/{u}`.
    pub subject: String,
    /// Display name or email address.
    pub name: String,
    /// Platform role as the API sends it, such as `ADMIN` or `REVIEWER`. `None` for a
    /// user in no role group: every route other than `GET /api/v1/users/me` answers such
    /// a user with 403.
    pub role: Option<String>,
}

/// `GET /api/v1/users/me` as the API sends it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireUser {
    name: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    role: Option<String>,
}

impl From<WireUser> for User {
    fn from(wire: WireUser) -> Self {
        Self {
            subject: last_segment(&wire.name).to_owned(),
            name: wire.display_name,
            role: wire.role,
        }
    }
}

impl Client {
    /// Returns the user the access token belongs to (`GET /api/v1/users/me`).
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
        resource::get(self, resource::url(self, ["users", "me"])?).await
    }
}

#[cfg(test)]
mod tests;
