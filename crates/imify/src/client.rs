//! The HTTP client every API call goes through.

use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderValue};
use reqwest::{Method, Url};
use serde::de::DeserializeOwned;

use crate::auth::TokenSource;
use crate::error::{ConnectFailed, from_response};
use crate::retry::{self, RetryPolicy};
use crate::{Error, VERSION};

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(60);

/// A client for the imify platform API.
///
/// Every request carries a `User-Agent` header and, when a token source is configured,
/// an `Authorization: Bearer` header. Failed requests are retried according to the
/// client's [`RetryPolicy`]. Cloning is cheap; clones share one connection pool.
///
/// # Examples
///
/// ```
/// use imify::Client;
/// use imify::auth::StaticToken;
///
/// let client = Client::builder("https://api.example.com")
///     .token_source(StaticToken::new("secret"))
///     .build()?;
/// # Ok::<(), imify::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: Url,
    token_source: Option<Arc<dyn TokenSource>>,
    retry_policy: RetryPolicy,
}

impl Client {
    /// Starts configuring a client for the API at `base_url`, such as
    /// `https://api.example.com`. A path in the base URL prefixes every request path.
    ///
    /// # Examples
    ///
    /// ```
    /// let client = imify::Client::builder("https://api.example.com").build()?;
    /// # Ok::<(), imify::Error>(())
    /// ```
    pub fn builder(base_url: &str) -> ClientBuilder {
        ClientBuilder {
            base_url: base_url.to_owned(),
            token_source: None,
            user_agent: format!("imify-cli/{VERSION}"),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            read_timeout: DEFAULT_READ_TIMEOUT,
            retry_policy: RetryPolicy::default(),
        }
    }

    /// Sends a request with an optional JSON body and decodes the JSON response as `T`,
    /// retrying per the client's retry policy.
    pub(crate) async fn execute<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T, Error> {
        let url = self.url(path)?;
        let mut attempt = 0;
        loop {
            match self.send(method.clone(), url.clone(), body).await {
                Err(error) if retry::should_retry(&self.retry_policy, &method, &error, attempt) => {
                    tokio::time::sleep(retry::delay(&self.retry_policy, attempt, &error)).await;
                    attempt += 1;
                }
                result => return result,
            }
        }
    }

    async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        body: Option<&serde_json::Value>,
    ) -> Result<T, Error> {
        let mut request = self.http.request(method, url);
        if let Some(authorization) = self.authorization().await? {
            request = request.header(AUTHORIZATION, authorization);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(network_error)?;
        decode(response).await
    }

    async fn authorization(&self) -> Result<Option<HeaderValue>, Error> {
        let Some(source) = &self.token_source else {
            return Ok(None);
        };
        let token = source.token().await?;
        let mut value =
            HeaderValue::try_from(format!("Bearer {}", token.secret())).map_err(|_| {
                Error::Auth {
                    status: None,
                    message: "token contains characters not allowed in an HTTP header".to_owned(),
                }
            })?;
        value.set_sensitive(true);
        Ok(Some(value))
    }

    fn url(&self, path: &str) -> Result<Url, Error> {
        self.base_url
            .join(path.trim_start_matches('/'))
            .map_err(|error| Error::Config {
                message: format!("invalid request path {path:?}: {error}"),
            })
    }
}

async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, Error> {
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(network_error)?;
    if !status.is_success() {
        return Err(from_response(status.as_u16(), &headers, &body));
    }
    serde_json::from_slice(&body).map_err(|error| Error::UnexpectedResponse {
        status: status.as_u16(),
        message: format!("response body does not match the expected type: {error}"),
    })
}

fn network_error(error: reqwest::Error) -> Error {
    let source: Box<dyn std::error::Error + Send + Sync> = if error.is_connect() {
        Box::new(ConnectFailed(Box::new(error)))
    } else {
        Box::new(error)
    };
    Error::Network { source }
}

/// Configures a [`Client`]. Created by [`Client::builder`].
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use imify::{Client, RetryPolicy};
///
/// let client = Client::builder("https://api.example.com")
///     .user_agent("robot-fleet/2.1")
///     .connect_timeout(Duration::from_secs(5))
///     .read_timeout(Duration::from_secs(120))
///     .retry_policy(RetryPolicy::none())
///     .build()?;
/// # Ok::<(), imify::Error>(())
/// ```
#[derive(Debug)]
#[must_use]
pub struct ClientBuilder {
    base_url: String,
    token_source: Option<Arc<dyn TokenSource>>,
    user_agent: String,
    connect_timeout: Duration,
    read_timeout: Duration,
    retry_policy: RetryPolicy,
}

impl ClientBuilder {
    /// Sets where the client obtains access tokens. Without a token source, requests
    /// carry no `Authorization` header.
    ///
    /// # Examples
    ///
    /// ```
    /// let builder = imify::Client::builder("https://api.example.com")
    ///     .token_source(imify::auth::StaticToken::new("secret"));
    /// ```
    pub fn token_source(mut self, source: impl TokenSource + 'static) -> Self {
        self.token_source = Some(Arc::new(source));
        self
    }

    /// Replaces the `User-Agent` header. Default: `imify-cli/<version>`.
    ///
    /// # Examples
    ///
    /// ```
    /// let builder = imify::Client::builder("https://api.example.com").user_agent("robot-fleet/2.1");
    /// ```
    pub fn user_agent(mut self, value: impl Into<String>) -> Self {
        self.user_agent = value.into();
        self
    }

    /// Sets how long to wait for a connection to be established. Default: 10 s.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// let builder = imify::Client::builder("https://api.example.com")
    ///     .connect_timeout(Duration::from_secs(5));
    /// ```
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Sets how long to wait between two reads of a response. Default: 60 s. A request
    /// has no total time limit, so long transfers complete as long as data keeps flowing.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// let builder = imify::Client::builder("https://api.example.com")
    ///     .read_timeout(Duration::from_secs(120));
    /// ```
    pub fn read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
        self
    }

    /// Sets the retry policy. Default: [`RetryPolicy::default`].
    ///
    /// # Examples
    ///
    /// ```
    /// let builder = imify::Client::builder("https://api.example.com")
    ///     .retry_policy(imify::RetryPolicy::none());
    /// ```
    pub fn retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    /// Builds the client.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] when the base URL is not an `http` or `https` URL, when
    /// the user agent is not a valid header value, or when the TLS backend fails to
    /// initialize.
    ///
    /// # Examples
    ///
    /// ```
    /// let error = imify::Client::builder("not a url").build().unwrap_err();
    /// assert!(matches!(error, imify::Error::Config { .. }));
    /// ```
    pub fn build(self) -> Result<Client, Error> {
        let base_url = parse_base_url(&self.base_url)?;
        let user_agent = HeaderValue::try_from(self.user_agent).map_err(|_| Error::Config {
            message: "user agent contains characters not allowed in an HTTP header".to_owned(),
        })?;
        let http = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(self.connect_timeout)
            .read_timeout(self.read_timeout)
            .build()
            .map_err(|error| Error::Config {
                message: format!("cannot initialize the HTTP client: {error}"),
            })?;
        Ok(Client {
            http,
            base_url,
            token_source: self.token_source,
            retry_policy: self.retry_policy,
        })
    }
}

fn parse_base_url(raw: &str) -> Result<Url, Error> {
    let invalid = |reason: &str| Error::Config {
        message: format!("invalid base URL {raw:?}: {reason}"),
    };
    let mut url = Url::parse(raw).map_err(|error| invalid(&error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid("scheme must be http or https"));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

#[cfg(test)]
mod tests;
