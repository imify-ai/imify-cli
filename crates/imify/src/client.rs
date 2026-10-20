//! The HTTP client every API call goes through.

use std::future::Future;
use std::sync::{Arc, OnceLock};
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

/// Largest error body read, and largest body of an identity provider response.
pub(crate) const MAX_SMALL_BODY_BYTES: usize = 1024 * 1024;

/// Largest body of a successful API response.
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

/// A client for the imify platform API.
///
/// Every request carries a `User-Agent` header and, when a token source is configured,
/// an `Authorization: Bearer` header. Failed requests are retried according to the
/// client's [`RetryPolicy`]. Redirects with status 307 and 308 are followed, up to 5 in a
/// row and never from `https` to `http`; the `Authorization` header is not sent to
/// another host or port. A file upload also never follows a redirect to `http` on a host
/// other than `localhost`, `127.0.0.1` or `::1`. Other redirects fail with
/// [`Error::UnexpectedResponse`]. A successful response body larger than 32 MiB fails with
/// [`Error::UnexpectedResponse`]; an error body larger than 1 MiB is dropped. Requests
/// to the identity provider differ: the protected resource metadata, `config.json` and the
/// discovery document follow redirects only within their origin, and token and revocation requests follow none.
/// Storage requests of a [`Downloader`](crate::download::Downloader) carry no
/// `Authorization` header and follow redirects only within the origin of the presigned URL.
/// Cloning is cheap; clones share their connection pools.
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
    provider_http: Arc<ProviderHttp>,
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
            || {
                self.send(
                    Authorize::Yes,
                    method.clone(),
                    url.clone(),
                    body,
                    &decode_json,
                )
            },
        )
        .await
    }

    /// Sends a request with the `Authorization` header to `url` and turns the response
    /// into `T` with `decode`, retrying failures that `mode` allows. `decode` receives every
    /// response except redirects, which fail with [`Error::UnexpectedResponse`].
    pub(crate) async fn execute_with<T, D>(
        &self,
        mode: RetryMode,
        method: Method,
        url: Url,
        decode: &D,
    ) -> Result<T, Error>
    where
        D: Fn(StatusCode, &HeaderMap, &[u8]) -> Result<T, Error>,
    {
        self.retry(
            mode,
            |_, _, _| {},
            || self.send(Authorize::Yes, method.clone(), url.clone(), None, decode),
        )
        .await
    }

    /// Sends a `GET` without an `Authorization` header to `url` and decodes the JSON
    /// response as `T`, retrying failures the retry policy allows. Follows redirects only
    /// within the origin of `url`.
    pub(crate) async fn get_same_origin<T: DeserializeOwned>(&self, url: Url) -> Result<T, Error> {
        self.get_same_origin_with(url, &decode_json).await
    }

    /// Like [`Client::get_same_origin`], and turns the response into `T` with `decode`.
    /// `decode` receives every response except redirects, which fail with
    /// [`Error::UnexpectedResponse`].
    pub(crate) async fn get_same_origin_with<T, D>(&self, url: Url, decode: &D) -> Result<T, Error>
    where
        D: Fn(StatusCode, &HeaderMap, &[u8]) -> Result<T, Error>,
    {
        let http = self.provider_http.same_origin()?;
        self.retry(
            RetryMode::Idempotent,
            |_, _, _| {},
            || async {
                let response = http.get(url.clone()).send().await.map_err(network_error)?;
                decode_with(response, BodySize::Small, decode).await
            },
        )
        .await
    }

    /// Sends a request without an `Authorization` header to `url` and turns the response
    /// into `T` with `decode`, retrying failures that `mode` allows. `decode` receives every
    /// response except redirects, which fail with [`Error::UnexpectedResponse`].
    pub(crate) async fn execute_unauthenticated_with<T, D>(
        &self,
        mode: RetryMode,
        method: Method,
        url: Url,
        decode: &D,
    ) -> Result<T, Error>
    where
        D: Fn(StatusCode, &HeaderMap, &[u8]) -> Result<T, Error>,
    {
        self.retry(
            mode,
            |_, _, _| {},
            || self.send(Authorize::No, method.clone(), url.clone(), None, decode),
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

    /// The HTTP client for form posts to the identity provider. It sends no
    /// `Authorization` header and follows no redirects.
    pub(crate) fn form_http(&self) -> Result<&reqwest::Client, Error> {
        self.provider_http.no_redirects()
    }

    /// The HTTP client for streamed file uploads. It has no read timeout; the caller
    /// limits the time without progress to [`Client::read_timeout`].
    pub(crate) fn upload_http(&self) -> &reqwest::Client {
        &self.upload_http
    }

    /// The HTTP client for storage downloads. It sends no `Authorization` header, follows
    /// redirects only within the origin of the request and applies the read timeout.
    pub(crate) fn download_http(&self) -> Result<&reqwest::Client, Error> {
        self.provider_http.same_origin()
    }

    /// The configured read timeout.
    pub(crate) fn read_timeout(&self) -> Duration {
        self.read_timeout
    }

    /// Returns `true` when `url` has the scheme, host and port of the base URL.
    pub(crate) fn is_same_origin(&self, url: &Url) -> bool {
        url.origin() == self.base_url.origin()
    }

    async fn send<T, D>(
        &self,
        authorize: Authorize,
        method: Method,
        url: Url,
        body: Option<&serde_json::Value>,
        decode: &D,
    ) -> Result<T, Error>
    where
        D: Fn(StatusCode, &HeaderMap, &[u8]) -> Result<T, Error>,
    {
        let mut request = self.http.request(method, url);
        if authorize == Authorize::Yes
            && let Some(authorization) = self.authorization().await?
        {
            request = request.header(AUTHORIZATION, authorization);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(network_error)?;
        decode_with(response, BodySize::Large, decode).await
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

    /// Returns the base URL of the API.
    pub(crate) fn base_url(&self) -> &Url {
        &self.base_url
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

/// Whether a request carries the token source's `Authorization` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Authorize {
    Yes,
    No,
}

/// Decodes a successful JSON response as `T` and maps any other response to an [`Error`].
pub(crate) async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, Error> {
    decode_with(response, BodySize::Large, &decode_json).await
}

/// Reads the response and turns it into `T` with `decode`. A success body is read as
/// `success_body` says, any other body as [`read_small_body`] does. A redirect fails with
/// [`Error::UnexpectedResponse`] without calling `decode`.
async fn decode_with<T, D>(
    response: reqwest::Response,
    success_body: BodySize,
    decode: &D,
) -> Result<T, Error>
where
    D: Fn(StatusCode, &HeaderMap, &[u8]) -> Result<T, Error>,
{
    let status = response.status();
    let headers = response.headers().clone();
    let body = match success_body {
        BodySize::Large if status.is_success() => read_body(response, MAX_RESPONSE_BYTES).await?,
        _ => read_small_body(response).await?,
    };
    if status.is_redirection() {
        return Err(redirect_not_followed(status, &headers));
    }
    decode(status, &headers, &body)
}

/// How much of a success body is read: at most [`MAX_RESPONSE_BYTES`] or
/// [`MAX_SMALL_BODY_BYTES`]. Other bodies are read as [`read_small_body`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodySize {
    Large,
    Small,
}

/// Reads a body of at most [`MAX_SMALL_BODY_BYTES`] as [`read_body`] does.
pub(crate) async fn read_small_body(response: reqwest::Response) -> Result<Vec<u8>, Error> {
    read_body(response, MAX_SMALL_BODY_BYTES).await
}

/// Reads a body of at most `limit` bytes, checking `Content-Length` first and counting the
/// bytes received. A larger success body fails with [`Error::UnexpectedResponse`]; a
/// larger body of another status reads as empty, so the error built from it carries the
/// reason phrase.
async fn read_body(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, Error> {
    let status = response.status();
    let too_large = || {
        if status.is_success() {
            Err(Error::UnexpectedResponse {
                status: status.as_u16(),
                message: format!("the response body exceeds {limit} bytes"),
            })
        } else {
            Ok(Vec::new())
        }
    };
    let announced = response
        .content_length()
        .map(|length| usize::try_from(length).unwrap_or(usize::MAX));
    if announced.is_some_and(|length| length > limit) {
        return too_large();
    }
    let mut body = Vec::with_capacity(announced.unwrap_or_default());
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if body.len() + chunk.len() > limit {
            return too_large();
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Decodes a successful JSON response as `T` and maps any other status to an [`Error`].
pub(crate) fn decode_json<T: DeserializeOwned>(
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<T, Error> {
    if !status.is_success() {
        return Err(from_response(status.as_u16(), headers, body));
    }
    serde_json::from_slice(body).map_err(|error| Error::UnexpectedResponse {
        status: status.as_u16(),
        message: format!("response body does not match the expected type: {error}"),
    })
}

/// The error for a redirect that was not followed, naming its `Location`.
pub(crate) fn redirect_not_followed(status: StatusCode, headers: &HeaderMap) -> Error {
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

/// Like [`may_follow_redirect`], and never to plain `http` on a host other than
/// `localhost`, `127.0.0.1` or `::1`, whatever the scheme of `previous`.
pub(crate) fn may_follow_upload_redirect(status: StatusCode, previous: &Url, next: &Url) -> bool {
    may_follow_redirect(status, previous, next) && (next.scheme() != "http" || is_loopback(next))
}

fn redirect_policy() -> redirect::Policy {
    policy_following(may_follow_redirect)
}

/// Follows up to [`MAX_REDIRECTS`] redirects that `may_follow` allows.
fn policy_following(may_follow: fn(StatusCode, &Url, &Url) -> bool) -> redirect::Policy {
    redirect::Policy::custom(move |attempt| {
        let previous = attempt.previous();
        let allowed = previous.len() <= MAX_REDIRECTS
            && previous
                .last()
                .is_some_and(|last| may_follow(attempt.status(), last, attempt.url()));
        if allowed {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

/// Like [`redirect_policy`], and only to the origin of the first request.
pub(crate) fn same_origin_redirect_policy() -> redirect::Policy {
    redirect::Policy::custom(|attempt| {
        let previous = attempt.previous();
        let same_origin = previous
            .first()
            .is_some_and(|first| first.origin() == attempt.url().origin());
        let allowed = same_origin
            && previous.len() <= MAX_REDIRECTS
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

/// The settings every HTTP client of a [`Client`] shares.
#[derive(Debug, Clone)]
struct HttpSettings {
    user_agent: HeaderValue,
    connect_timeout: Duration,
    read_timeout: Duration,
}

impl HttpSettings {
    fn builder(&self, redirects: redirect::Policy) -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .user_agent(self.user_agent.clone())
            .connect_timeout(self.connect_timeout)
            .redirect(redirects)
    }
}

/// The HTTP clients for identity provider requests and storage downloads. Each is built on
/// first use, since building one loads the TLS root certificates.
#[derive(Debug)]
struct ProviderHttp {
    settings: HttpSettings,
    same_origin: OnceLock<reqwest::Client>,
    no_redirects: OnceLock<reqwest::Client>,
}

impl ProviderHttp {
    fn new(settings: HttpSettings) -> Self {
        Self {
            settings,
            same_origin: OnceLock::new(),
            no_redirects: OnceLock::new(),
        }
    }

    /// Follows redirects only within the origin of the request.
    fn same_origin(&self) -> Result<&reqwest::Client, Error> {
        self.get_or_build(&self.same_origin, same_origin_redirect_policy)
    }

    /// Follows no redirects.
    fn no_redirects(&self) -> Result<&reqwest::Client, Error> {
        self.get_or_build(&self.no_redirects, redirect::Policy::none)
    }

    fn get_or_build<'a>(
        &self,
        cell: &'a OnceLock<reqwest::Client>,
        redirects: fn() -> redirect::Policy,
    ) -> Result<&'a reqwest::Client, Error> {
        if let Some(http) = cell.get() {
            return Ok(http);
        }
        let http = build_http(
            self.settings
                .builder(redirects())
                .read_timeout(self.settings.read_timeout),
        )?;
        Ok(cell.get_or_init(|| http))
    }
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
    /// Returns [`Error::Config`] when the base URL is not an `http` or `https` URL, when a
    /// token source is set and the base URL uses `http` on a host other than `localhost`,
    /// `127.0.0.1` or `::1`, when the user agent is not a valid header value, or when the
    /// TLS backend fails to initialize.
    ///
    /// # Examples
    ///
    /// ```
    /// let error = imify::Client::builder("not a url").build().unwrap_err();
    /// assert!(matches!(error, imify::Error::Config { .. }));
    /// ```
    pub fn build(self) -> Result<Client, Error> {
        let mut base_url = parse_base_url(&self.base_url)?;
        if self.token_source.is_some() {
            base_url = require_secure(base_url, "API")?;
        }
        let user_agent = HeaderValue::try_from(self.user_agent).map_err(|_| Error::Config {
            message: "user agent contains characters not allowed in an HTTP header".to_owned(),
        })?;
        let settings = HttpSettings {
            user_agent,
            connect_timeout: self.connect_timeout,
            read_timeout: self.read_timeout,
        };
        let http = build_http(
            settings
                .builder(redirect_policy())
                .read_timeout(self.read_timeout),
        )?;
        let upload_http =
            build_http(settings.builder(policy_following(may_follow_upload_redirect)))?;
        Ok(Client {
            http,
            upload_http,
            provider_http: Arc::new(ProviderHttp::new(settings)),
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

/// Returns `url` when it uses `https`, or `http` on the host `localhost`, `127.0.0.1` or
/// `::1`. `name` names the URL in the error.
pub(crate) fn require_secure(url: Url, name: &str) -> Result<Url, Error> {
    match url.scheme() {
        "https" => Ok(url),
        "http" if is_loopback(&url) => Ok(url),
        _ => Err(Error::Config {
            message: format!(
                "the {name} URL {url} must use https unless its host is localhost, 127.0.0.1 or ::1"
            ),
        }),
    }
}

/// Returns `true` for the hosts `localhost`, `127.0.0.1` and `::1`.
pub(crate) fn is_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "[::1]"
    })
}

pub(crate) fn parse_base_url(raw: &str) -> Result<Url, Error> {
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
