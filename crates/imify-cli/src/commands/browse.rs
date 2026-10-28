//! `imify browse`: opens the platform in the web browser.

use std::io::{self, Write};

use super::write_result;
use crate::browser::{Browser, SystemBrowser};
use crate::cli::BrowseArgs;
use crate::config::resolve_api_url;
use crate::output::renderer::{Render, Renderer};
use crate::output::{CliError, Environment, ErrorCode};

/// `imify browse`: opens the API host in the system browser, or prints its URL with
/// `--no-browser`.
///
/// # Errors
///
/// As [`browse_with`].
pub fn browse(
    env: &Environment,
    args: &BrowseArgs,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    browse_with(env, args, &SystemBrowser, renderer)
}

/// Opens the API URL in `browser` and writes it as the result. The API URL is `--api-url`,
/// else `IMIFY_API_URL`, else `api_url` in the config file, else `https://platform.imify.ai`.
/// With `--no-browser`, writes the URL without opening a browser. Sends no request.
///
/// # Errors
///
/// Returns the errors of [`resolve_api_url`], and `internal` when the browser cannot be
/// started; the hint carries the URL.
pub fn browse_with(
    env: &Environment,
    args: &BrowseArgs,
    browser: &dyn Browser,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    let url = resolve_api_url(env)?.to_string();
    if !args.no_browser {
        browser.open(&url).map_err(|error| {
            CliError::new(
                ErrorCode::Internal,
                format!("Cannot open the browser: {error}."),
            )
            .with_hint(format!(
                "Open {url} in a browser, or run `imify browse --no-browser` to print the URL."
            ))
        })?;
    }
    write_result(
        renderer,
        &Browse {
            url,
            opened: !args.no_browser,
        },
    )
}

/// The result of `imify browse`.
struct Browse {
    url: String,
    /// `false` with `--no-browser`.
    opened: bool,
}

impl Render for Browse {
    fn kind(&self) -> &'static str {
        "browse"
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "url": self.url,
            "opened": self.opened,
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        if self.opened {
            writeln!(w, "Opening {} in the browser.", self.url)
        } else {
            writeln!(w, "{}", self.url)
        }
    }
}
