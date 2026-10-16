//! `imify config`: reads and changes the settings in the config file.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::write_result;
use crate::config::{ConfigFile, Key, Source, check_api_url, resolve_api_url};
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
}

impl Setting {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "key": self.key.name(),
            "value": self.value,
            "source": self.source.as_str(),
        })
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
        writeln!(w, "{} ({})", self.value, self.source.as_str())
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
            table.row(vec![
                setting.key.name().to_owned(),
                setting.value.clone(),
                setting.source.as_str().to_owned(),
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

/// The outcome of `imify config set`.
#[derive(Debug)]
struct Stored {
    key: Key,
    value: String,
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
            "path": self.path.display().to_string(),
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(
            w,
            "Set {} to {} in {}.",
            self.key.name(),
            self.value,
            printable_path(&self.path)
        )
    }
}

/// `imify config set <key> <value>`: checks `value` and stores it in the config file. Warns
/// when `--api-url` or `IMIFY_API_URL` takes precedence over the stored value.
///
/// # Errors
///
/// Returns `invalid_argument` when `value` is not valid for `key` or the config file is
/// invalid; nothing is written then. Returns `internal` when the file cannot be written.
pub fn set(
    env: &Environment,
    key: Key,
    value: &str,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
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
    let mut values = file.load()?;
    values.set(key, Some(value.clone()));
    file.save(&values)?;
    warn_when_overridden(env, key, renderer)?;
    write_result(
        renderer,
        &Stored {
            key,
            value,
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
    renderer
        .warning(&format!(
            "{name} is set and takes precedence over the stored {}.",
            key.name()
        ))
        .map_err(|error| {
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
            "path": self.path.display().to_string(),
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        if self.removed {
            writeln!(
                w,
                "Removed {} from {}.",
                self.key.name(),
                printable_path(&self.path)
            )
        } else {
            writeln!(
                w,
                "{} is not set in {}.",
                self.key.name(),
                printable_path(&self.path)
            )
        }
    }
}

/// `imify config unset <key>`: removes `key` from the config file. Removes the file when no
/// key is left.
///
/// # Errors
///
/// Returns `invalid_argument` when the config file is invalid, and `internal` when it cannot
/// be written.
pub fn unset(env: &Environment, key: Key, renderer: &mut dyn Renderer) -> Result<(), CliError> {
    let file = ConfigFile::open(env)?;
    let mut values = file.load()?;
    let removed = values.get(key).is_some();
    if removed {
        values.set(key, None);
        file.save(&values)?;
    }
    write_result(
        renderer,
        &Unset {
            key,
            removed,
            path: file.path().to_path_buf(),
        },
    )
}

/// `path` with control characters escaped.
fn printable_path(path: &Path) -> String {
    printable(&path.display().to_string()).into_owned()
}
