//! Updates a program that its dist shell installer installed to the latest GitHub release.
//!
//! ```no_run
//! use std::path::Path;
//! use imify::update::{Installation, ReleaseClient, ReleaseSource, Version};
//!
//! # async fn run() -> Result<(), imify::Error> {
//! let source = ReleaseSource::imify_cli();
//! let receipt = source.receipt_path(Path::new("/home/ada/.config"));
//! let executable = Path::new("/home/ada/.cargo/bin/imify");
//! let Some(installation) = Installation::from_receipt(&receipt, executable, &source)? else {
//!     return Ok(()); // not an installer install
//! };
//! let client = ReleaseClient::builder(source).build()?;
//! let latest = client.latest().await?;
//! if latest.version > Version::new(0, 1, 0) {
//!     client.install(&installation, &latest).await?;
//! }
//! # Ok(()) }
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::header::{ACCEPT, HeaderMap, HeaderValue};
use reqwest::{StatusCode, Url};
pub use semver::Version;
use serde::Deserialize;

use crate::client::{
    network_error, parse_base_url, read_small_body, redirect_not_followed, require_secure,
    same_origin_redirect_policy,
};
use crate::error::{from_response, retry_after};
use crate::{Error, VERSION};

mod install;

pub use install::Installation;

/// Default GitHub API URL.
const DEFAULT_API_URL: &str = "https://api.github.com";
/// Default URL that release assets are downloaded from.
const DEFAULT_DOWNLOAD_URL: &str = "https://github.com";
/// Default time limit of one request, including the body.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// Media type of the GitHub REST API.
const GITHUB_JSON: &str = "application/vnd.github+json";

/// The GitHub repository a program is released from, and its dist app name.
///
/// # Examples
///
/// ```
/// use imify::update::ReleaseSource;
///
/// let source = ReleaseSource::new("imify-ai", "imify-cli", "imify-cli");
/// assert_eq!(source, ReleaseSource::imify_cli());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseSource {
    owner: String,
    repo: String,
    app_name: String,
}

impl ReleaseSource {
    /// The repository `owner/repo` with the dist app `app_name`, which names the installer
    /// `<app_name>-installer.sh` and the receipt `<app_name>-receipt.json`.
    ///
    /// # Examples
    ///
    /// ```
    /// let source = imify::update::ReleaseSource::new("robot-fleet", "fleet-cli", "fleet-cli");
    /// ```
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        app_name: impl Into<String>,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            app_name: app_name.into(),
        }
    }

    /// The `imify` command-line tool: repository `imify-ai/imify-cli`, app `imify-cli`.
    ///
    /// # Examples
    ///
    /// ```
    /// let source = imify::update::ReleaseSource::imify_cli();
    /// ```
    #[must_use]
    pub fn imify_cli() -> Self {
        Self::new("imify-ai", "imify-cli", "imify-cli")
    }

    /// Where the shell installer writes its install receipt: `<app>/<app>-receipt.json`
    /// below `config_home`, which is `$XDG_CONFIG_HOME` or `$HOME/.config`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use imify::update::ReleaseSource;
    ///
    /// let path = ReleaseSource::imify_cli().receipt_path(Path::new("/home/ada/.config"));
    /// assert_eq!(path, Path::new("/home/ada/.config/imify-cli/imify-cli-receipt.json"));
    /// ```
    #[must_use]
    pub fn receipt_path(&self, config_home: &Path) -> PathBuf {
        config_home
            .join(&self.app_name)
            .join(format!("{}-receipt.json", self.app_name))
    }

    fn installer_name(&self) -> String {
        format!("{}-installer.sh", self.app_name)
    }

    /// The prefix of the installer's environment variables: the app name in upper case with
    /// `-` replaced by `_`, such as `IMIFY_CLI`.
    fn variable_prefix(&self) -> String {
        self.app_name.to_ascii_uppercase().replace('-', "_")
    }
}

/// The latest release of a [`ReleaseSource`]: its tag as published, such as `v0.2.0`, and the
/// version the tag names.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::update::ReleaseClient) -> Result<(), imify::Error> {
/// let release = client.latest().await?;
/// println!("{} is version {}", release.tag, release.version);
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Release {
    /// The Git tag, such as `v0.2.0`.
    pub tag: String,
    /// The version from the tag, without a leading `v`.
    pub version: Version,
}

/// Looks up releases on GitHub and installs them.
///
/// Requests carry `User-Agent: imify-cli/<version>` and no credentials. The release lookup
/// also sends `Accept: application/vnd.github+json`. Each request, body included, is limited
/// to the timeout and its body to 1 MiB.
///
/// # Examples
///
/// ```
/// use imify::update::{ReleaseClient, ReleaseSource};
///
/// let client = ReleaseClient::builder(ReleaseSource::imify_cli()).build()?;
/// # Ok::<(), imify::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct ReleaseClient {
    source: ReleaseSource,
    api_url: Url,
    download_url: Url,
    api_http: reqwest::Client,
    download_http: reqwest::Client,
}

impl ReleaseClient {
    /// Starts configuring a client for the releases of `source`.
    ///
    /// # Examples
    ///
    /// ```
    /// use imify::update::{ReleaseClient, ReleaseSource};
    ///
    /// let builder = ReleaseClient::builder(ReleaseSource::imify_cli());
    /// ```
    pub fn builder(source: ReleaseSource) -> ReleaseClientBuilder {
        ReleaseClientBuilder {
            source,
            api_url: DEFAULT_API_URL.to_owned(),
            download_url: DEFAULT_DOWNLOAD_URL.to_owned(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Returns the latest release: `GET <api>/repos/<owner>/<repo>/releases/latest`, which
    /// excludes drafts and prereleases. Follows redirects with status 307 and 308 within the
    /// API's origin only. Unknown response fields are ignored.
    ///
    /// # Errors
    ///
    /// - [`Error::RateLimited`] on 403 and 429.
    /// - [`Error::NotFound`] on 404: the repository has no release.
    /// - [`Error::Server`] on 5xx.
    /// - [`Error::Network`] when the request fails or exceeds the timeout.
    /// - [`Error::UnexpectedResponse`] on any other status, a body over 1 MiB, or a tag that
    ///   is not a semantic version with an optional leading `v`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use imify::update::{ReleaseClient, ReleaseSource};
    ///
    /// # async fn run() -> Result<(), imify::Error> {
    /// let client = ReleaseClient::builder(ReleaseSource::imify_cli()).build()?;
    /// println!("latest: {}", client.latest().await?.version);
    /// # Ok(()) }
    /// ```
    pub async fn latest(&self) -> Result<Release, Error> {
        let mut url = self.api_url.clone();
        url.path_segments_mut()
            .map_err(|()| invalid_url(&self.api_url))?
            .pop_if_empty()
            .extend([
                "repos",
                &self.source.owner,
                &self.source.repo,
                "releases",
                "latest",
            ]);
        let response = self
            .api_http
            .get(url)
            .header(ACCEPT, HeaderValue::from_static(GITHUB_JSON))
            .send()
            .await
            .map_err(network_error)?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = read_small_body(response).await?;
        decode_release(status, &headers, &body)
    }

    /// The URL of the release's shell installer:
    /// `<download>/<owner>/<repo>/releases/download/<tag>/<app>-installer.sh`.
    fn installer_url(&self, release: &Release) -> Result<Url, Error> {
        let mut url = self.download_url.clone();
        url.path_segments_mut()
            .map_err(|()| invalid_url(&self.download_url))?
            .pop_if_empty()
            .extend([
                self.source.owner.as_str(),
                &self.source.repo,
                "releases",
                "download",
                &release.tag,
                &self.source.installer_name(),
            ]);
        Ok(url)
    }
}

fn invalid_url(url: &Url) -> Error {
    Error::Config {
        message: format!("the URL {url} cannot have a path"),
    }
}

/// The JSON of a release; other fields are ignored.
#[derive(Deserialize)]
struct ReleaseJson {
    tag_name: String,
}

fn decode_release(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> Result<Release, Error> {
    match status.as_u16() {
        200..=299 => {}
        300..=399 => return Err(redirect_not_followed(status, headers)),
        403 => return Err(Error::RateLimited { retry_after: None }),
        429 => {
            return Err(Error::RateLimited {
                retry_after: retry_after(headers),
            });
        }
        404 | 500..=599 => return Err(from_response(status.as_u16(), headers, body)),
        other => {
            return Err(Error::UnexpectedResponse {
                status: other,
                message: format!("the release lookup answered with {status}"),
            });
        }
    }
    let unexpected = |message: String| Error::UnexpectedResponse {
        status: status.as_u16(),
        message,
    };
    let release: ReleaseJson = serde_json::from_slice(body)
        .map_err(|error| unexpected(format!("the response is not a release: {error}")))?;
    let version = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .map_err(|error| {
        unexpected(format!(
            "the release tag {:?} is not a version: {error}",
            release.tag_name
        ))
    })?;
    Ok(Release {
        tag: release.tag_name,
        version,
    })
}

/// Configures a [`ReleaseClient`]. Created by [`ReleaseClient::builder`].
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use imify::update::{ReleaseClient, ReleaseSource};
///
/// let client = ReleaseClient::builder(ReleaseSource::imify_cli())
///     .api_url("https://ghe.example.com/api/v3")
///     .download_url("https://ghe.example.com")
///     .timeout(Duration::from_secs(5))
///     .build()?;
/// # Ok::<(), imify::Error>(())
/// ```
#[derive(Debug)]
#[must_use]
pub struct ReleaseClientBuilder {
    source: ReleaseSource,
    api_url: String,
    download_url: String,
    timeout: Duration,
}

impl ReleaseClientBuilder {
    /// Sets the GitHub API URL. Default: `https://api.github.com`. For GitHub Enterprise
    /// Server, it is `https://<host>/api/v3`.
    ///
    /// # Examples
    ///
    /// ```
    /// use imify::update::{ReleaseClient, ReleaseSource};
    ///
    /// let builder = ReleaseClient::builder(ReleaseSource::imify_cli())
    ///     .api_url("https://ghe.example.com/api/v3");
    /// ```
    pub fn api_url(mut self, url: &str) -> Self {
        url.clone_into(&mut self.api_url);
        self
    }

    /// Sets the URL that release assets are downloaded from. Default: `https://github.com`.
    /// With another URL, the installer also downloads its archives from there.
    ///
    /// # Examples
    ///
    /// ```
    /// use imify::update::{ReleaseClient, ReleaseSource};
    ///
    /// let builder = ReleaseClient::builder(ReleaseSource::imify_cli())
    ///     .download_url("https://ghe.example.com");
    /// ```
    pub fn download_url(mut self, url: &str) -> Self {
        url.clone_into(&mut self.download_url);
        self
    }

    /// Sets the time limit of each request, body included. Default: 10 s.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use imify::update::{ReleaseClient, ReleaseSource};
    ///
    /// let builder = ReleaseClient::builder(ReleaseSource::imify_cli())
    ///     .timeout(Duration::from_secs(1));
    /// ```
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Builds the client.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] when a URL is not an `https` URL, or an `http` URL on a host
    /// other than `localhost`, `127.0.0.1` or `::1`, or when the TLS backend fails to
    /// initialize.
    ///
    /// # Examples
    ///
    /// ```
    /// use imify::update::{ReleaseClient, ReleaseSource};
    ///
    /// let error = ReleaseClient::builder(ReleaseSource::imify_cli())
    ///     .api_url("http://api.example.com")
    ///     .build()
    ///     .unwrap_err();
    /// assert!(matches!(error, imify::Error::Config { .. }));
    /// ```
    pub fn build(self) -> Result<ReleaseClient, Error> {
        let api_url = require_secure(parse_base_url(&self.api_url)?, "release API")?;
        let download_url = require_secure(parse_base_url(&self.download_url)?, "download")?;
        let http = |redirects| {
            reqwest::Client::builder()
                .user_agent(format!("imify-cli/{VERSION}"))
                .timeout(self.timeout)
                .redirect(redirects)
                .build()
                .map_err(|error| Error::Config {
                    message: format!("cannot initialize the HTTP client: {error}"),
                })
        };
        Ok(ReleaseClient {
            api_http: http(same_origin_redirect_policy())?,
            download_http: http(install::download_redirect_policy())?,
            source: self.source,
            api_url,
            download_url,
        })
    }
}

#[cfg(test)]
mod tests;
