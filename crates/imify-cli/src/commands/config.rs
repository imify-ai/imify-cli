//! `imify config`: reads and changes the settings in the config file, in the selected profile.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::write_result;
use crate::config::{
    ConfigFile, DEFAULT_API_URL, Key, ProfileName, Source, check_api_url, resolve_api_url,
    select_profile,
};
use crate::output::renderer::{Render, Renderer};
use crate::output::sanitize::printable;
use crate::output::table::{Column, Table};
use crate::output::{CliError, Environment, ErrorCode};

/// A setting's value and where it comes from.
#[derive(Debug)]
struct Setting {
    key: Key,
    value: String,
    source: Source,
    /// The profile whose table supplied the value, when `source` is [`Source::Config`].
    profile: Option<String>,
}

impl Setting {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "key": self.key.name(),
            "value": self.value,
            "source": self.source.as_str(),
            "profile": self.profile,
        })
    }

    /// The profile that supplied the value, unless it is `default`.
    fn named_profile(&self) -> Option<&str> {
        self.profile
            .as_deref()
            .filter(|profile| *profile != ProfileName::DEFAULT)
    }
}

/// The value of `key` a command uses.
fn resolve(env: &Environment, key: Key) -> Result<Setting, CliError> {
    match key {
        Key::ApiUrl => {
            let api_url = resolve_api_url(env)?;
            Ok(Setting {
                key,
                value: api_url.to_string(),
                source: api_url.source,
                profile: api_url.profile,
            })
        }
    }
}

impl Render for Setting {
    fn kind(&self) -> &'static str {
        "config_setting"
    }

    fn json(&self) -> serde_json::Value {
        Setting::json(self)
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        match self.named_profile() {
            Some(profile) => writeln!(
                w,
                "{} ({}, profile {profile})",
                self.value,
                self.source.as_str()
            ),
            None => writeln!(w, "{} ({})", self.value, self.source.as_str()),
        }
    }
}

/// `imify config get <key>`: prints the value of `key` and its source.
///
/// # Errors
///
/// Returns the errors of resolving the value, and `internal` when stdout cannot be written.
pub fn get(env: &Environment, key: Key, renderer: &mut dyn Renderer) -> Result<(), CliError> {
    write_result(renderer, &resolve(env, key)?)
}

/// Every documented setting.
#[derive(Debug)]
struct Settings(Vec<Setting>);

impl Render for Settings {
    fn kind(&self) -> &'static str {
        "config_settings"
    }

    fn json(&self) -> serde_json::Value {
        self.0.iter().map(Setting::json).collect()
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        let mut table = Table::new(vec![
            Column::left("KEY"),
            Column::left("VALUE"),
            Column::left("SOURCE"),
        ]);
        for setting in &self.0 {
            let source = match setting.named_profile() {
                Some(profile) => format!("{} ({profile})", setting.source.as_str()),
                None => setting.source.as_str().to_owned(),
            };
            table.row(vec![
                setting.key.name().to_owned(),
                setting.value.clone(),
                source,
            ]);
        }
        table.write(w)
    }
}

/// `imify config list`: prints every documented setting with its value and source.
///
/// # Errors
///
/// Returns the errors of resolving a value, and `internal` when stdout cannot be written.
pub fn list(env: &Environment, renderer: &mut dyn Renderer) -> Result<(), CliError> {
    let settings = Key::ALL
        .into_iter()
        .map(|key| resolve(env, key))
        .collect::<Result<Vec<_>, _>>()?;
    write_result(renderer, &Settings(settings))
}

/// The config file at `path`, preceded by `profile <name> in` for a named profile.
fn location(profile: &ProfileName, path: &Path) -> String {
    if profile.is_default() {
        printable_path(path)
    } else {
        format!("profile {profile} in {}", printable_path(path))
    }
}

/// The outcome of `imify config set`.
#[derive(Debug)]
struct Stored {
    key: Key,
    value: String,
    profile: ProfileName,
    path: PathBuf,
}

impl Render for Stored {
    fn kind(&self) -> &'static str {
        "config_set"
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "key": self.key.name(),
            "value": self.value,
            "profile": self.profile.as_str(),
            "path": self.path.display().to_string(),
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        let target = if self.profile.is_default() {
            "in"
        } else {
            "for"
        };
        writeln!(
            w,
            "Set {} to {} {target} {}.",
            self.key.name(),
            self.value,
            location(&self.profile, &self.path)
        )
    }
}

/// `imify config set <key> <value>`: checks `value` and stores it in the selected profile in
/// the config file, creating the profile when it does not exist. Warns when `--api-url` or
/// `IMIFY_API_URL` takes precedence over the stored value.
///
/// # Errors
///
/// Returns `invalid_argument` when `value` is not valid for `key`, the selected profile name
/// is invalid or the config file is invalid; nothing is written then. Returns `internal` when
/// the file cannot be written.
pub fn set(
    env: &Environment,
    key: Key,
    value: &str,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    let profile = select_profile(env)?.name;
    let value = match key {
        Key::ApiUrl => {
            let url = check_api_url(
                value,
                key.name(),
                "Pass the API host, such as `https://api.example.com`.",
            )?;
            url.as_str().trim_end_matches('/').to_owned()
        }
    };
    let file = ConfigFile::open(env)?;
    let mut contents = file.load()?;
    contents
        .profile_or_insert(profile.clone())
        .set(key, Some(value.clone()));
    file.save(&contents)?;
    warn_when_overridden(env, key, renderer)?;
    write_result(
        renderer,
        &Stored {
            key,
            value,
            profile,
            path: file.path().to_path_buf(),
        },
    )
}

fn warn_when_overridden(
    env: &Environment,
    key: Key,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    let overriding = match key {
        Key::ApiUrl => [
            (env.api_url_flag.as_deref(), "--api-url"),
            (env.api_url.as_deref(), crate::config::API_URL_VARIABLE),
        ]
        .into_iter()
        .find(|(value, _)| value.is_some_and(|value| !value.trim().is_empty()))
        .map(|(_, name)| name),
    };
    let Some(name) = overriding else {
        return Ok(());
    };
    warn(
        renderer,
        &format!(
            "{name} is set and takes precedence over the stored {}.",
            key.name()
        ),
    )
}

fn warn(renderer: &mut dyn Renderer, message: &str) -> Result<(), CliError> {
    renderer.warning(message).map_err(|error| {
        CliError::new(
            ErrorCode::Internal,
            format!("Cannot write a warning: {error}"),
        )
    })
}

/// The outcome of `imify config unset`.
#[derive(Debug)]
struct Unset {
    key: Key,
    removed: bool,
    profile: ProfileName,
    path: PathBuf,
}

impl Render for Unset {
    fn kind(&self) -> &'static str {
        "config_unset"
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "key": self.key.name(),
            "removed": self.removed,
            "profile": self.profile.as_str(),
            "path": self.path.display().to_string(),
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        if self.removed {
            writeln!(
                w,
                "Removed {} from {}.",
                self.key.name(),
                location(&self.profile, &self.path)
            )
        } else {
            writeln!(
                w,
                "{} is not set in {}.",
                self.key.name(),
                location(&self.profile, &self.path)
            )
        }
    }
}

/// `imify config unset <key>`: removes `key` from the selected profile in the config file.
/// Removes a named profile's table when no key is left in it, and the file when no key and no
/// profile is left.
///
/// # Errors
///
/// Returns `invalid_argument` when the selected profile is invalid or does not exist or the
/// config file is invalid, and `internal` when the file cannot be written.
pub fn unset(env: &Environment, key: Key, renderer: &mut dyn Renderer) -> Result<(), CliError> {
    let selected = select_profile(env)?;
    let file = ConfigFile::open(env)?;
    let mut contents = file.load()?;
    selected.require(&contents, file.path())?;
    let removed = contents.remove(&selected.name, key);
    if removed {
        file.save(&contents)?;
    }
    write_result(
        renderer,
        &Unset {
            key,
            removed,
            profile: selected.name,
            path: file.path().to_path_buf(),
        },
    )
}

/// One profile in the config file.
#[derive(Debug)]
struct Profile {
    name: ProfileName,
    active: bool,
    api_url: String,
}

/// Every profile in the config file.
#[derive(Debug)]
struct Profiles(Vec<Profile>);

impl Render for Profiles {
    fn kind(&self) -> &'static str {
        "config_profiles"
    }

    fn json(&self) -> serde_json::Value {
        self.0
            .iter()
            .map(|profile| {
                serde_json::json!({
                    "name": profile.name.as_str(),
                    "active": profile.active,
                    "api_url": profile.api_url,
                })
            })
            .collect()
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        let mut table = Table::new(vec![
            Column::left("ACTIVE"),
            Column::left("NAME"),
            Column::left("API-URL"),
        ]);
        for profile in &self.0 {
            table.row(vec![
                if profile.active { "*" } else { "" }.to_owned(),
                profile.name.to_string(),
                profile.api_url.clone(),
            ]);
        }
        table.write(w)
    }
}

/// `imify config profiles`: prints every profile in the config file, `default` first, with
/// the API URL it resolves to without `--api-url` and `IMIFY_API_URL`, and marks the selected
/// profile. Warns when the selected profile does not exist.
///
/// # Errors
///
/// Returns `invalid_argument` when the selected profile name or the config file is invalid,
/// and `internal` when stdout or stderr cannot be written.
pub fn profiles(env: &Environment, renderer: &mut dyn Renderer) -> Result<(), CliError> {
    let selected = select_profile(env)?;
    let file = ConfigFile::open(env)?;
    let contents = file.load()?;
    if contents.profile(&selected.name).is_none() {
        warn(
            renderer,
            &format!(
                "The profile '{}' selected by {} does not exist.",
                selected.name,
                selected.source.label()
            ),
        )?;
    }
    let profiles = contents
        .profile_names()
        .map(|name| Profile {
            name: name.clone(),
            active: *name == selected.name,
            api_url: contents
                .lookup(name, Key::ApiUrl)
                .map_or(DEFAULT_API_URL, |(value, _)| value)
                .to_owned(),
        })
        .collect();
    write_result(renderer, &Profiles(profiles))
}

/// `path` with control characters escaped.
fn printable_path(path: &Path) -> String {
    printable(&path.display().to_string()).into_owned()
}
