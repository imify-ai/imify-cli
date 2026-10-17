//! Command implementations and what they share: the API client and result output.

pub mod auth;
pub mod config;
pub mod datasets;
pub mod doctor;

use url::Url;

use self::auth::{LoginLookup, TokenOrigin};
use crate::config::resolve_api_url;
use crate::output::renderer::{Render, Renderer};
use crate::output::{AUTH_REQUIRED_HINT, CliError, Environment, ErrorCode};

/// An API client, where its token comes from and the lookup of the host's stored login.
#[derive(Debug)]
pub struct Api {
    pub client: imify::Client,
    pub origin: TokenOrigin,
    pub lookup: LoginLookup,
}

impl Api {
    /// The CLI error for a failed request of this client. Hints follow the token's origin, as
    /// in [`TokenOrigin::adjust`].
    pub async fn error(self, error: imify::Error) -> CliError {
        self.origin.adjust(error.into(), self.lookup).await
    }
}

/// Builds an API client for the host [`api_url`] resolves, authenticated by the token source
/// [`auth::token_source`] resolves. The client sends the token only to that host, over
/// `https` unless the host is `localhost`, `127.0.0.1` or `::1`.
///
/// # Errors
///
/// Returns the errors of [`api_url`] and [`auth::token_source`]. Returns `auth_required`
/// when there is neither `IMIFY_TOKEN` nor a stored login for the host, and
/// `invalid_argument` when the SDK rejects the URL for a client with a token source.
pub async fn client(env: &Environment) -> Result<Api, CliError> {
    let api_url = resolve_api_url(env)?;
    let host = crate::credentials::host_key(&api_url.url);
    let Some((token_source, origin)) = auth::token_source(env, &host).await? else {
        return Err(CliError::new(
            ErrorCode::AuthRequired,
            format!(
                "Not logged in to {host}, and {} is not set.",
                imify::auth::EnvToken::VARIABLE
            ),
        )
        .with_hint(AUTH_REQUIRED_HINT));
    };
    let client = imify::Client::builder(api_url.url.as_str())
        .token_source(token_source)
        .build()
        .map_err(|error| api_url.invalid(&error))?;
    Ok(Api {
        client,
        origin,
        lookup: LoginLookup::new(env, &host),
    })
}

/// The API URL from `--api-url`, `IMIFY_API_URL`, the config file or the default, in that
/// order.
///
/// # Errors
///
/// Returns the errors of [`resolve_api_url`].
pub fn api_url(env: &Environment) -> Result<Url, CliError> {
    resolve_api_url(env).map(|resolved| resolved.url)
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
