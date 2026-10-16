//! Liveness and readiness of the API.

use std::time::SystemTime;

use reqwest::header::{DATE, HeaderMap};
use reqwest::{Method, StatusCode};

use crate::client::decode_json;
use crate::error::from_response;
use crate::retry::RetryMode;
use crate::{Client, Error};

/// Liveness of the API. Unknown response fields are ignored.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
/// let health: imify::Health = client.health().await?;
/// println!("API status {}", health.status);
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Health {
    /// `ok` when the API process is serving requests.
    pub status: String,
    /// The response's `Date` header; `None` when it is missing or unparsable.
    pub server_time: Option<SystemTime>,
}

/// Readiness of the API and its dependencies. Unknown response fields are ignored.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
/// let readiness: imify::Readiness = client.readiness().await?;
/// println!("database {}, storage {}", readiness.db, readiness.storage);
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[non_exhaustive]
pub struct Readiness {
    /// `ok` when the API can serve every request.
    pub status: String,
    /// State of the database, such as `ok`.
    pub db: String,
    /// State of the object storage, such as `ok`.
    pub storage: String,
}

#[derive(serde::Deserialize)]
struct HealthJson {
    status: String,
}

impl Client {
    /// Returns the liveness of the API (`GET /api/health`). Sends no `Authorization`
    /// header.
    ///
    /// # Errors
    ///
    /// Returns the [`Error`] variants as for every request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), imify::Error> {
    /// let client = imify::Client::builder("https://api.example.com").build()?;
    /// let health = client.health().await?;
    /// println!("{} at {:?}", health.status, health.server_time);
    /// # Ok(()) }
    /// ```
    pub async fn health(&self) -> Result<Health, Error> {
        let decode = |status: StatusCode, headers: &HeaderMap, body: &[u8]| {
            let health: HealthJson = decode_json(status, headers, body)?;
            Ok(Health {
                status: health.status,
                server_time: server_time(headers),
            })
        };
        self.execute_unauthenticated_with(
            RetryMode::Idempotent,
            Method::GET,
            self.url("api/health")?,
            &decode,
        )
        .await
    }

    /// Returns the readiness of the API and its dependencies (`GET /api/health/ready`).
    /// Sends no `Authorization` header. Returns the body for status 200 and for 503.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Server`] for a 503 whose body is not a readiness document. Returns
    /// the other [`Error`] variants as for every request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), imify::Error> {
    /// let client = imify::Client::builder("https://api.example.com").build()?;
    /// let readiness = client.readiness().await?;
    /// if readiness.status != "ok" {
    ///     println!("database {}, storage {}", readiness.db, readiness.storage);
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn readiness(&self) -> Result<Readiness, Error> {
        let decode = |status: StatusCode, headers: &HeaderMap, body: &[u8]| {
            if status == StatusCode::SERVICE_UNAVAILABLE {
                serde_json::from_slice(body)
                    .map_err(|_| from_response(status.as_u16(), headers, body))
            } else {
                decode_json(status, headers, body)
            }
        };
        self.execute_unauthenticated_with(
            RetryMode::Idempotent,
            Method::GET,
            self.url("api/health/ready")?,
            &decode,
        )
        .await
    }
}

/// The `Date` header as a point in time; `None` when it is missing or unparsable.
fn server_time(headers: &HeaderMap) -> Option<SystemTime> {
    let value = headers.get(DATE)?.to_str().ok()?;
    httpdate::parse_http_date(value).ok()
}

#[cfg(test)]
mod tests;
