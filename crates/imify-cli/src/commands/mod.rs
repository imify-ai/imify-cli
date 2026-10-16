//! Command implementations and what they share: the API client and result output.

pub mod auth;

use crate::output::renderer::{Render, Renderer};
use crate::output::{AUTH_REQUIRED_HINT, CliError, Environment, ErrorCode};

const API_URL_VARIABLE: &str = "IMIFY_API_URL";

/// Builds an API client for the host in `IMIFY_API_URL`, authenticated by the token source
/// [`auth::token_source`] resolves.
///
/// # Errors
///
/// Returns `invalid_argument` when `IMIFY_API_URL` is unset or not an `http` or `https`
/// URL, or when the token source cannot be read. Returns `auth_required` when no token
/// source is available.
pub fn client(env: &Environment) -> Result<imify::Client, CliError> {
    let api_url = env.api_url.as_deref().unwrap_or_default().trim();
    if api_url.is_empty() {
        return Err(CliError::new(
            ErrorCode::InvalidArgument,
            format!("{API_URL_VARIABLE} is not set."),
        )
        .with_hint(api_url_hint()));
    }
    let Some(token_source) = auth::token_source()? else {
        return Err(CliError::new(
            ErrorCode::AuthRequired,
            format!(
                "No access token: {} is not set.",
                imify::auth::EnvToken::VARIABLE
            ),
        )
        .with_hint(AUTH_REQUIRED_HINT));
    };
    imify::Client::builder(api_url)
        .token_source(token_source)
        .build()
        .map_err(|error| {
            CliError::new(
                ErrorCode::InvalidArgument,
                format!("{API_URL_VARIABLE} is invalid: {error}"),
            )
            .with_hint(api_url_hint())
        })
}

fn api_url_hint() -> String {
    format!("Set `{API_URL_VARIABLE}` to the API host, such as `https://api.example.com`.")
}

/// Writes the result of a command.
///
/// # Errors
///
/// Returns `internal` when stdout cannot be written.
pub fn write_result(renderer: &mut dyn Renderer, value: &dyn Render) -> Result<(), CliError> {
    renderer.result(value).map_err(|error| {
        CliError::new(
            ErrorCode::Internal,
            format!("Cannot write the result: {error}"),
        )
    })
}
