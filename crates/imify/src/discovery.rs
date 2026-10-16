//! The identity provider settings a host publishes.

use reqwest::Method;

use crate::retry::RetryMode;
use crate::{Client, Error};

/// Issuer and client id of the host's identity provider client.
///
/// # Examples
///
/// ```
/// use imify::OidcSettings;
///
/// let settings = OidcSettings::new("https://login.example.com", "client-1");
/// assert_eq!(settings.client_id, "client-1");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OidcSettings {
    /// OIDC issuer URL, such as `https://login.example.com`.
    pub issuer: String,
    /// OAuth client id the login uses.
    pub client_id: String,
}

impl OidcSettings {
    /// Creates settings from an issuer URL and a client id.
    ///
    /// # Examples
    ///
    /// ```
    /// let settings = imify::OidcSettings::new("https://login.example.com", "client-1");
    /// assert_eq!(settings.issuer, "https://login.example.com");
    /// ```
    pub fn new(issuer: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into(),
            client_id: client_id.into(),
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigJson {
    authority: String,
    client_id: String,
}

impl Client {
    /// Reads the identity provider settings from `config.json` under the base URL: the
    /// keys `authority` (the issuer) and `clientId`. Sends no `Authorization` header and
    /// ignores other keys.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] when the host serves no `config.json`, and
    /// [`Error::UnexpectedResponse`] when a key is missing. Returns the other [`Error`]
    /// variants as for every request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), imify::Error> {
    /// let client = imify::Client::builder("https://api.example.com").build()?;
    /// let settings = client.oidc_settings().await?;
    /// println!("issuer {}, client {}", settings.issuer, settings.client_id);
    /// # Ok(()) }
    /// ```
    pub async fn oidc_settings(&self) -> Result<OidcSettings, Error> {
        let config: ConfigJson = self
            .execute_unauthenticated(RetryMode::Idempotent, Method::GET, self.url("config.json")?)
            .await?;
        Ok(OidcSettings::new(config.authority, config.client_id))
    }
}

#[cfg(test)]
mod tests;
