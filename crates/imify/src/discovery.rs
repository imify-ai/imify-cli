//! The identity provider settings a host publishes.

use reqwest::header::HeaderMap;
use reqwest::{StatusCode, Url};

use crate::client::{decode_json, require_secure};
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

/// The path prefix of protected resource metadata; the resource's path follows it.
const METADATA_PREFIX: &str = "/.well-known/oauth-protected-resource";

/// The path of the API below the base URL; the protected resource.
const RESOURCE_PATH: &str = "api/v1";

/// The protected resource metadata fields the CLI uses.
#[derive(serde::Deserialize)]
struct ResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
    imify_cli_client_id: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigJson {
    authority: String,
    client_id: String,
}

impl Client {
    /// Reads the identity provider settings the host publishes, from one of two sources.
    ///
    /// 1. The protected resource metadata of the API at
    ///    `/.well-known/oauth-protected-resource<base URL path>api/v1` on the API origin:
    ///    the first entry of `authorization_servers` (the issuer) and
    ///    `imify_cli_client_id`. Its `resource` must equal the base URL followed by
    ///    `api/v1`; a trailing slash is ignored.
    /// 2. When that request answers 404, or 403 without a JSON body, `config.json` under
    ///    the base URL: the keys `authority` (the issuer) and `clientId`.
    ///
    /// Both requests send no `Authorization` header, follow redirects only within the API
    /// origin and ignore other keys. A 404 or 403 of the metadata request is not retried.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] without sending a request when the base URL uses `http`
    /// on a host other than `localhost`, `127.0.0.1` or `::1`, and when the metadata names
    /// another resource. Returns [`Error::NotFound`] when the host serves neither source,
    /// and [`Error::UnexpectedResponse`] when a key is missing or `authorization_servers`
    /// is empty. Returns the other [`Error`] variants as for every request; a failed
    /// metadata request other than the 404 and 403 above does not fall back to
    /// `config.json`.
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
        require_secure(self.base_url().clone(), "API")?;
        let metadata = self
            .get_same_origin_with(self.metadata_url(), &decode_metadata)
            .await?;
        if let Some(metadata) = metadata {
            return self.settings_from_metadata(metadata);
        }
        let config: ConfigJson = self.get_same_origin(self.url("config.json")?).await?;
        Ok(OidcSettings::new(config.authority, config.client_id))
    }

    /// The URL of the protected resource metadata: the well-known prefix on the API
    /// origin, followed by the path of the resource.
    fn metadata_url(&self) -> Url {
        let mut url = self.base_url().clone();
        let base_path = url.path().trim_end_matches('/').to_owned();
        url.set_path(&format!("{METADATA_PREFIX}{base_path}/{RESOURCE_PATH}"));
        url.set_query(None);
        url.set_fragment(None);
        url
    }

    fn settings_from_metadata(&self, metadata: ResourceMetadata) -> Result<OidcSettings, Error> {
        let expected = self.url(RESOURCE_PATH)?;
        if metadata.resource.trim_end_matches('/') != expected.as_str().trim_end_matches('/') {
            return Err(Error::Config {
                message: format!(
                    "the protected resource metadata names the resource {}, expected {expected}",
                    metadata.resource
                ),
            });
        }
        let issuer = metadata
            .authorization_servers
            .into_iter()
            .next()
            .ok_or_else(|| Error::UnexpectedResponse {
                status: StatusCode::OK.as_u16(),
                message: "the protected resource metadata lists no authorization server".to_owned(),
            })?;
        Ok(OidcSettings::new(issuer, metadata.imify_cli_client_id))
    }
}

/// Decodes the protected resource metadata. `None` when the host publishes none: a 404,
/// or a 403 without a JSON body, which an object store sends for a missing file.
fn decode_metadata(
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Option<ResourceMetadata>, Error> {
    let absent = status == StatusCode::NOT_FOUND
        || (status == StatusCode::FORBIDDEN
            && serde_json::from_slice::<serde_json::Value>(body).is_err());
    if absent {
        return Ok(None);
    }
    decode_json(status, headers, body).map(Some)
}

#[cfg(test)]
mod tests;
