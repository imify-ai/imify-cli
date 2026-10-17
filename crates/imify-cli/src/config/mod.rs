//! Settings and where they come from: `--api-url`, `IMIFY_API_URL`, the selected profile in
//! the config file, the top-level keys of the config file and the built-in default, in that
//! order.
//!
//! The config file is `config.toml` in the directory [`config_dir`] returns. Its top-level
//! documented [`Key`]s form the profile `default`; each `[profiles.<name>]` table holds the
//! same keys for a named profile:
//!
//! ```toml
//! api_url = "https://platform.imify.ai"
//!
//! [profiles.staging]
//! api_url = "https://staging.example.com"
//! ```

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::dirs::config_dir;
use crate::output::sanitize::printable;
use crate::output::{CliError, Environment, ErrorCode};
use crate::private_file::{self, Permissions};

/// The API URL when neither `--api-url`, `IMIFY_API_URL` nor the config file sets one.
pub const DEFAULT_API_URL: &str = "https://platform.imify.ai";

/// The variable that sets the API URL.
pub const API_URL_VARIABLE: &str = "IMIFY_API_URL";

/// The variable that selects the profile.
pub const PROFILE_VARIABLE: &str = "IMIFY_PROFILE";

const FILE_NAME: &str = "config.toml";

/// The key of the table that holds the named profiles.
const PROFILES_KEY: &str = "profiles";

/// The profile the top-level keys of the config file form.
static DEFAULT_PROFILE: ProfileName = ProfileName(Cow::Borrowed(ProfileName::DEFAULT));

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

/// The name of a profile: 1 to 64 ASCII letters, digits, `-` and `_`, starting with a letter
/// or digit. Names are case-sensitive.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProfileName(Cow<'static, str>);

/// The reason a profile name is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidProfileName;

impl fmt::Display for InvalidProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("use 1 to 64 letters, digits, '-' and '_', starting with a letter or digit")
    }
}

impl ProfileName {
    /// The name of the profile the top-level keys of the config file form.
    pub const DEFAULT: &str = "default";

    /// The profile `default`, formed by the top-level keys of the config file.
    pub fn default_profile() -> Self {
        DEFAULT_PROFILE.clone()
    }

    /// Checks `raw` against the naming rule.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidProfileName`] when `raw` breaks the rule.
    pub fn parse(raw: &str) -> Result<Self, InvalidProfileName> {
        let allowed = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        let starts_well = raw
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric());
        if starts_well && raw.len() <= 64 && raw.chars().all(allowed) {
            Ok(Self(Cow::Owned(raw.to_owned())))
        } else {
            Err(InvalidProfileName)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the profile `default`.
    pub fn is_default(&self) -> bool {
        *self == DEFAULT_PROFILE
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where the selected profile comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    Flag,
    Env,
    Default,
}

impl ProfileSource {
    /// The source for people: `--profile`, `IMIFY_PROFILE` or `default`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Flag => "--profile",
            Self::Env => PROFILE_VARIABLE,
            Self::Default => "default",
        }
    }
}

/// The profile a command reads its settings from, and where the selection comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedProfile {
    pub name: ProfileName,
    pub source: ProfileSource,
}

impl SelectedProfile {
    /// Checks that `contents`, read from `path`, has this profile. `default` always exists.
    ///
    /// # Errors
    ///
    /// Returns `invalid_argument` naming the profile, its source and `path` when the profile
    /// does not exist.
    pub fn require(&self, contents: &Contents, path: &Path) -> Result<(), CliError> {
        if contents.profile(&self.name).is_some() {
            return Ok(());
        }
        let create = format!(
            "`imify config set api-url <URL> --profile {}` to create it",
            self.name
        );
        let hint = match self.source {
            ProfileSource::Env => format!(
                "Run `imify config profiles` to list the profiles, {create}, or unset {PROFILE_VARIABLE}."
            ),
            ProfileSource::Flag | ProfileSource::Default => {
                format!("Run `imify config profiles` to list the profiles, or {create}.")
            }
        };
        Err(CliError::new(
            ErrorCode::InvalidArgument,
            format!(
                "{} selects the profile '{}', which is not in {}.",
                self.source.label(),
                self.name,
                path.display()
            ),
        )
        .with_hint(hint))
    }
}

/// The selected profile: `--profile`, else `IMIFY_PROFILE`, else `default`. An empty flag or
/// variable counts as unset. Does not read the config file.
///
/// # Errors
///
/// Returns `invalid_argument` naming the flag or the variable when the name breaks the naming
/// rule of [`ProfileName`].
pub fn select_profile(env: &Environment) -> Result<SelectedProfile, CliError> {
    let given = [
        (env.profile_flag.as_deref(), ProfileSource::Flag),
        (env.profile.as_deref(), ProfileSource::Env),
    ]
    .into_iter()
    .find_map(|(value, source)| value.filter(|value| !value.is_empty()).map(|v| (v, source)));
    let Some((raw, source)) = given else {
        return Ok(SelectedProfile {
            name: ProfileName::default_profile(),
            source: ProfileSource::Default,
        });
    };
    let name = ProfileName::parse(raw).map_err(|reason| {
        let hint = match source {
            ProfileSource::Env => format!(
                "Set `{PROFILE_VARIABLE}` to a profile name such as `staging`, or unset it."
            ),
            ProfileSource::Flag | ProfileSource::Default => {
                "Pass a profile name such as `staging` to `--profile`.".to_owned()
            }
        };
        CliError::new(
            ErrorCode::InvalidArgument,
            format!(
                "{} has the invalid profile name '{}': {reason}.",
                source.label(),
                printable(raw)
            ),
        )
        .with_hint(hint)
    })?;
    Ok(SelectedProfile { name, source })
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
    /// The profile whose table supplied the value, `default` for the top-level key, when
    /// `source` is [`Source::Config`]; otherwise `None`.
    pub profile: Option<String>,
    /// Names the source in error messages, such as `IMIFY_API_URL`.
    origin: String,
    /// The hint of an error about this URL.
    hint: String,
}

impl ResolvedApiUrl {
    /// The `invalid_argument` error for this URL, for `reason`, naming its source.
    pub fn invalid(&self, reason: &dyn fmt::Display) -> CliError {
        invalid_api_url(&self.origin, &self.hint, reason)
    }
}

/// The URL without a trailing slash, such as `https://platform.imify.ai`.
impl fmt::Display for ResolvedApiUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.url.as_str().trim_end_matches('/'))
    }
}

/// The API URL: `--api-url`, else `IMIFY_API_URL`, else `api_url` in the selected profile,
/// else the top-level `api_url` in the config file, else [`DEFAULT_API_URL`]. An empty flag or
/// variable counts as unset. The config file is read when neither the flag nor the variable
/// is set, or when a profile other than `default` is selected.
///
/// # Errors
///
/// Returns `invalid_argument` when the selected value fails [`check_api_url`], the errors of
/// [`select_profile`] and [`SelectedProfile::require`], and the errors of [`ConfigFile::load`]
/// when the file is read.
pub fn resolve_api_url(env: &Environment) -> Result<ResolvedApiUrl, CliError> {
    let selected = select_profile(env)?;
    if let Some(explicit) = explicit_api_url(env) {
        if !selected.name.is_default() {
            load_with_profile(env, &selected)?;
        }
        return explicit.check();
    }
    let (file, contents) = load_with_profile(env, &selected)?;
    configured_api_url(&contents, &selected.name, file.path()).check()
}

/// The config file and its contents, which must have the `selected` profile.
fn load_with_profile(
    env: &Environment,
    selected: &SelectedProfile,
) -> Result<(ConfigFile, Contents), CliError> {
    let file = ConfigFile::open(env)?;
    let contents = file.load()?;
    selected.require(&contents, file.path())?;
    Ok((file, contents))
}

/// An API URL before it is checked.
struct Candidate {
    raw: String,
    source: Source,
    profile: Option<String>,
    origin: String,
    hint: String,
}

impl Candidate {
    fn check(self) -> Result<ResolvedApiUrl, CliError> {
        let url = check_api_url(&self.raw, &self.origin, &self.hint)?;
        Ok(ResolvedApiUrl {
            url,
            source: self.source,
            profile: self.profile,
            origin: self.origin,
            hint: self.hint,
        })
    }

    fn default_api_url() -> Self {
        Self {
            raw: DEFAULT_API_URL.to_owned(),
            source: Source::Default,
            profile: None,
            origin: "The default API URL".to_owned(),
            hint: Source::Default.api_url_hint(),
        }
    }
}

/// The API URL from `--api-url` or `IMIFY_API_URL`; `None` when both are unset or empty.
fn explicit_api_url(env: &Environment) -> Option<Candidate> {
    let non_blank = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let (raw, source, origin) = if let Some(raw) = non_blank(&env.api_url_flag) {
        (raw, Source::Flag, "--api-url")
    } else {
        (non_blank(&env.api_url)?, Source::Env, API_URL_VARIABLE)
    };
    Some(Candidate {
        raw,
        source,
        profile: None,
        origin: origin.to_owned(),
        hint: source.api_url_hint(),
    })
}

/// The API URL of `profile` in `contents`, read from `path`, else the default.
fn configured_api_url(contents: &Contents, profile: &ProfileName, path: &Path) -> Candidate {
    let Some((raw, supplier)) = contents.lookup(profile, Key::ApiUrl) else {
        return Candidate::default_api_url();
    };
    let key = Key::ApiUrl.file_name();
    let (origin, hint) = if supplier.is_default() {
        (
            format!("{key} in {}", path.display()),
            Source::Config.api_url_hint(),
        )
    } else {
        (
            format!("{key} in profile {supplier} in {}", path.display()),
            format!(
                "Run `imify config set api-url <URL> --profile {supplier}`, or `imify config unset api-url --profile {supplier}`."
            ),
        )
    };
    Candidate {
        raw: raw.to_owned(),
        source: Source::Config,
        profile: Some(supplier.to_string()),
        origin,
        hint,
    }
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
pub fn is_loopback(url: &Url) -> bool {
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

/// The values of one profile. A key the profile does not set is `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Values {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
}

impl Values {
    /// The value of `key`, `None` when the profile does not set it.
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

/// The contents of the config file: the top-level values, which form the profile `default`,
/// and the named profiles.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contents {
    top: Values,
    profiles: BTreeMap<ProfileName, Values>,
}

impl Contents {
    /// The values of `profile`; `None` when the file has no such profile. `default` always
    /// exists.
    pub fn profile(&self, profile: &ProfileName) -> Option<&Values> {
        if profile.is_default() {
            Some(&self.top)
        } else {
            self.profiles.get(profile)
        }
    }

    /// The values of `profile`, created empty when the file has no such profile.
    pub fn profile_or_insert(&mut self, profile: ProfileName) -> &mut Values {
        if profile.is_default() {
            &mut self.top
        } else {
            self.profiles.entry(profile).or_default()
        }
    }

    /// Removes `key` from `profile`, and the profile's table when no key is left. Returns
    /// whether the profile set the key.
    pub fn remove(&mut self, profile: &ProfileName, key: Key) -> bool {
        if profile.is_default() {
            let removed = self.top.get(key).is_some();
            self.top.set(key, None);
            return removed;
        }
        let Some(values) = self.profiles.get_mut(profile) else {
            return false;
        };
        let removed = values.get(key).is_some();
        values.set(key, None);
        if removed && values.is_empty() {
            self.profiles.remove(profile);
        }
        removed
    }

    /// The value of `key` for `profile` and the profile that supplies it: the profile's own
    /// value, else the top-level value from `default`. `None` when neither is set.
    pub fn lookup<'a>(
        &'a self,
        profile: &'a ProfileName,
        key: Key,
    ) -> Option<(&'a str, &'a ProfileName)> {
        self.profile(profile)
            .and_then(|values| values.get(key))
            .map(|value| (value, profile))
            .or_else(|| self.top.get(key).map(|value| (value, &DEFAULT_PROFILE)))
    }

    /// Every profile: `default` first, then the named profiles sorted by name.
    pub fn profile_names(&self) -> impl Iterator<Item = &ProfileName> {
        std::iter::once(&DEFAULT_PROFILE).chain(self.profiles.keys())
    }

    fn is_empty(&self) -> bool {
        self.top.is_empty() && self.profiles.is_empty()
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

    /// The contents of the file; empty when the file or its directory does not exist.
    ///
    /// # Errors
    ///
    /// Returns `invalid_argument` naming the file when the file is not TOML, has a key that is
    /// not documented (the message names the key and the profile), has a profile name that
    /// breaks the naming rule of [`ProfileName`], or has a `[profiles.default]` table.
    /// Returns `internal` when the file or its directory is refused or cannot be read.
    pub fn load(&self) -> Result<Contents, CliError> {
        if !private_file::check_dir(self.dir())? {
            return Ok(Contents::default());
        }
        let Some(content) = private_file::read(&self.path, Permissions::OwnerWritable)? else {
            return Ok(Contents::default());
        };
        let content =
            String::from_utf8(content).map_err(|_| self.invalid(&"the file is not valid UTF-8"))?;
        let mut table: toml::Table =
            toml::from_str(&content).map_err(|error| self.invalid(&error.message().trim_end()))?;
        let profiles = match table.remove(PROFILES_KEY) {
            None => BTreeMap::new(),
            Some(toml::Value::Table(profiles)) => self.parse_profiles(profiles)?,
            Some(_) => return Err(self.invalid(&"profiles is not a table")),
        };
        let top = toml::Value::Table(table)
            .try_into()
            .map_err(|error: toml::de::Error| self.invalid(&error.message().trim_end()))?;
        Ok(Contents { top, profiles })
    }

    fn parse_profiles(
        &self,
        profiles: toml::Table,
    ) -> Result<BTreeMap<ProfileName, Values>, CliError> {
        profiles
            .into_iter()
            .map(|(raw, value)| {
                let shown = printable(&raw);
                let name = ProfileName::parse(&raw).map_err(|reason| {
                    self.invalid(&format!(
                        "the table profiles.{shown} has an invalid profile name: {reason}"
                    ))
                })?;
                if name.is_default() {
                    return Err(self.invalid(
                        &"the table profiles.default is not allowed; the top-level keys form the profile default",
                    ));
                }
                let toml::Value::Table(_) = value else {
                    return Err(self.invalid(&format!("profiles.{name} is not a table")));
                };
                let values = value.try_into().map_err(|error: toml::de::Error| {
                    self.invalid(&format!(
                        "in profile {name}: {}",
                        error.message().trim_end()
                    ))
                })?;
                Ok((name, values))
            })
            .collect()
    }

    /// Writes `contents` to the file: the top-level keys first, then the profiles sorted by
    /// name. Removes the file when `contents` has no key and no profile.
    ///
    /// # Errors
    ///
    /// Returns `internal` when the file or its directory is refused or cannot be written.
    pub fn save(&self, contents: &Contents) -> Result<(), CliError> {
        if contents.is_empty() {
            if self.path.exists() {
                private_file::remove(&self.path)?;
            }
            return Ok(());
        }
        let content = to_toml(contents).map_err(|error| {
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

/// `contents` as TOML: the top-level keys, then a `[profiles.<name>]` table per profile,
/// sorted by name.
fn to_toml(contents: &Contents) -> Result<String, toml::ser::Error> {
    let mut table = toml::Table::try_from(&contents.top)?;
    if !contents.profiles.is_empty() {
        let profiles = contents
            .profiles
            .iter()
            .map(|(name, values)| {
                Ok((
                    name.as_str().to_owned(),
                    toml::Value::Table(toml::Table::try_from(values)?),
                ))
            })
            .collect::<Result<toml::Table, toml::ser::Error>>()?;
        table.insert(PROFILES_KEY.to_owned(), toml::Value::Table(profiles));
    }
    toml::to_string(&table)
}

#[cfg(test)]
mod tests;
