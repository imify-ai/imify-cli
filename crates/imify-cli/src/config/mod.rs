//! Settings and where they come from: `--api-url`, `IMIFY_API_URL`, the config file and the
//! built-in default, in that order.
//!
//! The config file is `config.toml` in the directory [`config_dir`] returns. It holds the
//! documented [`Key`]s only.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::dirs::config_dir;
use crate::output::{CliError, Environment, ErrorCode};
use crate::private_file::{self, Permissions};

/// The API URL when neither `--api-url`, `IMIFY_API_URL` nor the config file sets one.
pub const DEFAULT_API_URL: &str = "https://platform.imify.ai";

/// The variable that sets the API URL.
pub const API_URL_VARIABLE: &str = "IMIFY_API_URL";

const FILE_NAME: &str = "config.toml";

/// A documented setting. On the command line its name has dashes, in the file underscores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Key {
    /// The API host every networked command talks to
    ApiUrl,
}

impl Key {
    /// Every documented key, in the order `imify config list` prints them.
    pub const ALL: [Self; 1] = [Self::ApiUrl];

    /// The name on the command line, such as `api-url`.
    pub fn name(self) -> &'static str {
        match self {
            Self::ApiUrl => "api-url",
        }
    }

    /// The name in the config file, such as `api_url`.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::ApiUrl => "api_url",
        }
    }
}

/// Where the value of a setting comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Flag,
    Env,
    Config,
    Default,
}

impl Source {
    /// The `source` value in JSON output: `flag`, `env`, `config` or `default`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flag => "flag",
            Self::Env => "env",
            Self::Config => "config",
            Self::Default => "default",
        }
    }

    /// Where the API URL comes from, for people: `--api-url`, `IMIFY_API_URL`, `config.toml`
    /// or `default`.
    pub fn api_url_label(self) -> &'static str {
        match self {
            Self::Flag => "--api-url",
            Self::Env => API_URL_VARIABLE,
            Self::Config => FILE_NAME,
            Self::Default => "default",
        }
    }

    fn api_url_hint(self) -> String {
        match self {
            Self::Flag => {
                "Pass the API host to `--api-url`, such as `https://api.example.com`.".to_owned()
            }
            Self::Env => format!(
                "Set `{API_URL_VARIABLE}` to the API host, such as `https://api.example.com`, or unset it."
            ),
            Self::Config => "Run `imify config set api-url <URL>` with the API host, or `imify config unset api-url`.".to_owned(),
            Self::Default => "Pass the API host to `--api-url`.".to_owned(),
        }
    }
}

/// The API URL a command uses, and where it comes from.
#[derive(Debug, Clone)]
pub struct ResolvedApiUrl {
    pub url: Url,
    pub source: Source,
    /// Names the source in error messages, such as `IMIFY_API_URL`.
    origin: String,
}

impl ResolvedApiUrl {
    /// The `invalid_argument` error for this URL, for `reason`, naming its source.
    pub fn invalid(&self, reason: &dyn fmt::Display) -> CliError {
        invalid_api_url(&self.origin, &self.source.api_url_hint(), reason)
    }
}

/// The URL without a trailing slash, such as `https://platform.imify.ai`.
impl fmt::Display for ResolvedApiUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.url.as_str().trim_end_matches('/'))
    }
}

/// The API URL: `--api-url`, else `IMIFY_API_URL`, else `api_url` in the config file, else
/// [`DEFAULT_API_URL`]. An empty flag or variable counts as unset. The config file is read
/// only when neither the flag nor the variable is set.
///
/// # Errors
///
/// Returns `invalid_argument` when the selected value fails [`check_api_url`], and the errors
/// of [`ConfigFile::load`] when the file is read.
pub fn resolve_api_url(env: &Environment) -> Result<ResolvedApiUrl, CliError> {
    let non_blank = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let (raw, source, origin) = if let Some(raw) = non_blank(&env.api_url_flag) {
        (raw, Source::Flag, "--api-url".to_owned())
    } else if let Some(raw) = non_blank(&env.api_url) {
        (raw, Source::Env, API_URL_VARIABLE.to_owned())
    } else {
        let file = ConfigFile::open(env)?;
        match file.load()?.api_url {
            Some(raw) => (
                raw,
                Source::Config,
                format!("{} in {}", Key::ApiUrl.file_name(), file.path().display()),
            ),
            None => (
                DEFAULT_API_URL.to_owned(),
                Source::Default,
                "The default API URL".to_owned(),
            ),
        }
    };
    let url = check_api_url(&raw, &origin, &source.api_url_hint())?;
    Ok(ResolvedApiUrl {
        url,
        source,
        origin,
    })
}

/// Parses `raw` as an API URL. `origin` names the value in the error message, `hint` is the
/// error's hint.
///
/// # Errors
///
/// Returns `invalid_argument` when `raw` is not a URL, has a user name, password, query or
/// fragment, or is not `https` on a host other than `localhost`, `127.0.0.1` and `::1`. The
/// message never repeats a URL with a user name or password.
pub fn check_api_url(raw: &str, origin: &str, hint: &str) -> Result<Url, CliError> {
    let invalid = |reason: &dyn fmt::Display| invalid_api_url(origin, hint, reason);
    let raw = raw.trim();
    let url = Url::parse(raw).map_err(|error| invalid(&error))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid(&"it contains a user name or password"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid(&format!(
            "{raw} has a query or fragment; use the API host and path only"
        )));
    }
    match url.scheme() {
        "https" => Ok(url),
        "http" if is_loopback(&url) => Ok(url),
        "http" => Err(invalid(&format!(
            "{raw} uses http; only localhost, 127.0.0.1 and ::1 may use http"
        ))),
        _ => Err(invalid(&format!("{raw} is not an https URL"))),
    }
}

/// Returns `true` for the hosts `localhost`, `127.0.0.1` and `::1`.
fn is_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "[::1]"
    })
}

fn invalid_api_url(origin: &str, hint: &str, reason: &dyn fmt::Display) -> CliError {
    CliError::new(
        ErrorCode::InvalidArgument,
        format!("{origin} is invalid: {reason}"),
    )
    .with_hint(hint)
}

/// The values in the config file. A key the file does not set is `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Values {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
}

impl Values {
    /// The value of `key`, `None` when the file does not set it.
    pub fn get(&self, key: Key) -> Option<&str> {
        match key {
            Key::ApiUrl => self.api_url.as_deref(),
        }
    }

    /// Sets `key` to `value`; `None` removes it.
    pub fn set(&mut self, key: Key, value: Option<String>) {
        match key {
            Key::ApiUrl => self.api_url = value,
        }
    }

    fn is_empty(&self) -> bool {
        Key::ALL.into_iter().all(|key| self.get(key).is_none())
    }
}

/// `config.toml` in the configuration directory.
///
/// A file that other users can write to, or a directory owned by another user or writable by
/// other users, is refused. Saving creates the directory with mode `0700` and replaces the
/// file atomically with mode `0600`.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    path: PathBuf,
}

impl ConfigFile {
    /// The config file in the directory [`config_dir`] returns.
    ///
    /// # Errors
    ///
    /// Returns `invalid_argument` when the directory depends on `HOME` and `HOME` is not an
    /// absolute path.
    pub fn open(env: &Environment) -> Result<Self, CliError> {
        Ok(Self {
            path: config_dir(env)?.join(FILE_NAME),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("."))
    }

    /// The values in the file; none when the file or its directory does not exist.
    ///
    /// # Errors
    ///
    /// Returns `invalid_argument` when the file is not TOML or has a key that is not
    /// documented; the message names the file and the key. Returns `internal` when the file or
    /// its directory is refused or cannot be read.
    pub fn load(&self) -> Result<Values, CliError> {
        if !private_file::check_dir(self.dir())? {
            return Ok(Values::default());
        }
        let Some(content) = private_file::read(&self.path, Permissions::OwnerWritable)? else {
            return Ok(Values::default());
        };
        let content =
            String::from_utf8(content).map_err(|_| self.invalid(&"the file is not valid UTF-8"))?;
        toml::from_str(&content).map_err(|error| self.invalid(&error.message().trim_end()))
    }

    /// Writes `values` to the file; removes the file when `values` sets no key.
    ///
    /// # Errors
    ///
    /// Returns `internal` when the file or its directory is refused or cannot be written.
    pub fn save(&self, values: &Values) -> Result<(), CliError> {
        if values.is_empty() {
            if self.path.exists() {
                private_file::remove(&self.path)?;
            }
            return Ok(());
        }
        let content = toml::to_string(values).map_err(|error| {
            CliError::new(
                ErrorCode::Internal,
                format!("Cannot write {}: {error}", self.path.display()),
            )
        })?;
        private_file::create_dir(self.dir())?;
        Ok(private_file::write(&self.path, content.as_bytes())?)
    }

    fn invalid(&self, reason: &dyn fmt::Display) -> CliError {
        CliError::new(
            ErrorCode::InvalidArgument,
            format!("{}: {reason}", self.path.display()),
        )
        .with_hint(format!(
            "Fix or remove {}. `imify config list` shows the supported keys.",
            self.path.display()
        ))
    }
}

#[cfg(test)]
mod tests;
