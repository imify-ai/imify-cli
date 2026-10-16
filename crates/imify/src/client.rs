//! The HTTP client every API call goes through.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, LOCATION};
use reqwest::{Method, StatusCode, Url, redirect};
use serde::de::DeserializeOwned;

use crate::auth::TokenSource;
use crate::error::{ConnectFailed, from_response};
use crate::retry::{self, RetryMode, RetryPolicy};
use crate::{Error, VERSION};

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Most redirects followed for one request.
pub(crate) const MAX_REDIRECTS: usize = 5;

/// A client for the imify platform API.
///
/// Every request carries a `User-Agent` header and, when a token source is configured,
/// an `Authorization: Bearer` header. Failed requests are retried according to the
/// client's [`RetryPolicy`]. Redirects with status 307 and 308 are followed, up to 5 in a
/// row and never from `https` to `http`; the `Authorization` header is not sent to
/// another host or port. Other redirects fail with [`Error::UnexpectedResponse`]. Cloning
/// is cheap; clones share their connection pools.
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
    upload_http: reqwest::Client,
    base_url: Url,
    token_source: Option<Arc<dyn TokenSource>>,
    retry_policy: RetryPolicy,
    read_timeout: Duration,
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
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "endpoint methods build on this in later changes")
    )]
    pub(crate) async fn execute<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T, Error> {
        let mode = RetryMode::from(&method);
        self.execute_at(mode, method, self.url(path)?, body).await
    }

    /// Sends a request to `url` with an optional JSON body and decodes the JSON response
    /// as `T`, retrying failures that `mode` allows.
    pub(crate) async fn execute_at<T: DeserializeOwned>(
        &self,
        mode: RetryMode,
        method: Method,
        url: Url,
        body: Option<&serde_json::Value>,
    ) -> Result<T, Error> {
        self.retry(
            mode,
            |_, _, _| {},
            || self.send(method.clone(), url.clone(), body),
        )
        .await
    }

    /// Runs `attempt` until it succeeds, fails with an error that `mode` does not retry,
    /// or the retry policy is exhausted. Before each retry, `on_retry` receives the retry
    /// number (starting at 1), the error and the delay before the retry.
    pub(crate) async fn retry<T, F, Fut>(
        &self,
        mode: RetryMode,
        mut on_retry: impl FnMut(u32, &Error, Duration),
        mut attempt: F,
    ) -> Result<T, Error>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, Error>>,
    {
        let mut retries = 0;
        loop {
            match attempt().await {
                Err(error) if retry::should_retry(&self.retry_policy, mode, &error, retries) => {
                    let delay = retry::delay(&self.retry_policy, retries, &error);
                    retries += 1;
                    on_retry(retries, &error, delay);
                    tokio::time::sleep(delay).await;
                }
                result => return result,
            }
        }
    }

    /// The HTTP client for streamed file uploads. It has no read timeout; the caller
    /// limits the time without progress to [`Client::read_timeout`].
    pub(crate) fn upload_http(&self) -> &reqwest::Client {
        &self.upload_http
    }

    /// The configured read timeout.
    pub(crate) fn read_timeout(&self) -> Duration {
        self.read_timeout
    }

    /// Returns `true` when `url` has the scheme, host and port of the base URL.
    pub(crate) fn is_same_origin(&self, url: &Url) -> bool {
        url.origin() == self.base_url.origin()
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

    /// The `Authorization` header value from the token source, `None` without one.
    pub(crate) async fn authorization(&self) -> Result<Option<HeaderValue>, Error> {
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

    /// Joins `path` to the base URL.
    pub(crate) fn url(&self, path: &str) -> Result<Url, Error> {
        self.base_url
            .join(path.trim_start_matches('/'))
            .map_err(|error| Error::Config {
                message: format!("invalid request path {path:?}: {error}"),
            })
    }
}

/// Decodes a successful JSON response as `T` and maps any other response to an [`Error`].
pub(crate) async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, Error> {
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(network_error)?;
    if status.is_redirection() {
        return Err(redirect_not_followed(status, &headers));
    }
    if !status.is_success() {
        return Err(from_response(status.as_u16(), &headers, &body));
    }
    serde_json::from_slice(&body).map_err(|error| Error::UnexpectedResponse {
        status: status.as_u16(),
        message: format!("response body does not match the expected type: {error}"),
    })
}

fn redirect_not_followed(status: StatusCode, headers: &HeaderMap) -> Error {
    let location = headers
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<no location>");
    Error::UnexpectedResponse {
        status: status.as_u16(),
        message: format!("redirect to {location} not followed"),
    }
}

/// Returns `true` when a redirect from `previous` to `next` with `status` may be followed.
pub(crate) fn may_follow_redirect(status: StatusCode, previous: &Url, next: &Url) -> bool {
    let downgrade = previous.scheme() == "https" && next.scheme() == "http";
    matches!(
        status,
        StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT
    ) && !downgrade
}

fn redirect_policy() -> redirect::Policy {
    redirect::Policy::custom(|attempt| {
        let previous = attempt.previous();
        let allowed = previous.len() <= MAX_REDIRECTS
            && previous
                .last()
                .is_some_and(|last| may_follow_redirect(attempt.status(), last, attempt.url()));
        if allowed {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

pub(crate) fn network_error(error: reqwest::Error) -> Error {
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
    /// For file uploads, it limits the time in which no part of the file is sent and the
    /// time between the last part and the response.
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
        let http_builder = || {
            reqwest::Client::builder()
                .user_agent(user_agent.clone())
                .connect_timeout(self.connect_timeout)
                .redirect(redirect_policy())
        };
        let http = build_http(http_builder().read_timeout(self.read_timeout))?;
        let upload_http = build_http(http_builder())?;
        Ok(Client {
            http,
            upload_http,
            base_url,
            token_source: self.token_source,
            retry_policy: self.retry_policy,
            read_timeout: self.read_timeout,
        })
    }
}

fn build_http(builder: reqwest::ClientBuilder) -> Result<reqwest::Client, Error> {
    builder.build().map_err(|error| Error::Config {
        message: format!("cannot initialize the HTTP client: {error}"),
    })
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
