//! Command implementations and what they share: the API client and result output.

pub mod auth;
pub mod datasets;
pub mod doctor;

use url::Url;

use self::auth::TokenOrigin;
use crate::output::renderer::{Render, Renderer};
use crate::output::{AUTH_REQUIRED_HINT, CliError, Environment, ErrorCode};

const API_URL_VARIABLE: &str = "IMIFY_API_URL";

/// An API client and where its token comes from.
#[derive(Debug)]
pub struct Api {
    pub client: imify::Client,
    pub origin: TokenOrigin,
}

impl Api {
    /// The CLI error for a failed request of this client. Hints follow the token's origin.
    pub fn error(&self, error: imify::Error) -> CliError {
        self.origin.adjust(error.into())
    }
}

/// Builds an API client for the host in `IMIFY_API_URL`, authenticated by the token source
/// [`auth::token_source`] resolves. The client sends the token only to that host, over
/// `https` unless the host is `localhost`, `127.0.0.1` or `::1`.
///
/// # Errors
///
/// Returns the errors of [`api_url`] and [`auth::token_source`]. Returns `auth_required`
/// when there is neither `IMIFY_TOKEN` nor a stored login for the host, and
/// `invalid_argument` when the SDK rejects the URL for a client with a token source.
pub async fn client(env: &Environment) -> Result<Api, CliError> {
    let api_url = api_url(env)?;
    let host = crate::credentials::host_key(&api_url);
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
    let client = imify::Client::builder(api_url.as_str())
        .token_source(token_source)
        .build()
        .map_err(|error| invalid_api_url(&error))?;
    Ok(Api { client, origin })
}

/// The API URL in `IMIFY_API_URL`.
///
/// # Errors
///
/// Returns `invalid_argument` when `IMIFY_API_URL` is unset, not a URL, has a user name,
/// password, query or fragment, or is not `https` on a host other than `localhost`,
/// `127.0.0.1` and `::1`. The message never repeats a URL with a user name or password.
pub fn api_url(env: &Environment) -> Result<Url, CliError> {
    let raw = env.api_url.as_deref().unwrap_or_default().trim();
    if raw.is_empty() {
        return Err(CliError::new(
            ErrorCode::InvalidArgument,
            format!("{API_URL_VARIABLE} is not set."),
        )
        .with_hint(api_url_hint()));
    }
    let url = Url::parse(raw).map_err(|error| invalid_api_url(&error))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid_api_url(&"it contains a user name or password"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid_api_url(&format!(
            "{raw} has a query or fragment; use the API host and path only"
        )));
    }
    match url.scheme() {
        "https" => Ok(url),
        "http" if is_loopback(&url) => Ok(url),
        "http" => Err(invalid_api_url(&format!(
            "{raw} uses http; only localhost, 127.0.0.1 and ::1 may use http"
        ))),
        _ => Err(invalid_api_url(&format!("{raw} is not an https URL"))),
    }
}

/// Returns `true` for the hosts `localhost`, `127.0.0.1` and `::1`.
fn is_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "[::1]"
    })
}

/// The `invalid_argument` error for an `IMIFY_API_URL` that is not usable, for `reason`.
pub fn invalid_api_url(reason: &dyn std::fmt::Display) -> CliError {
    CliError::new(
        ErrorCode::InvalidArgument,
        format!("{API_URL_VARIABLE} is invalid: {reason}"),
    )
    .with_hint(api_url_hint())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn url_of(raw: &str) -> Result<Url, CliError> {
        api_url(&Environment {
            api_url: Some(raw.to_owned()),
            ..Environment::default()
        })
    }

    #[test]
    fn an_https_api_url_is_accepted() {
        assert!(url_of("https://api.example.com").is_ok());
    }

    #[test]
    fn http_is_accepted_for_loopback_hosts() {
        let accepted = [
            "http://localhost:8080",
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
        ]
        .map(|raw| url_of(raw).is_ok());

        assert_eq!(accepted, [true; 3]);
    }

    #[test]
    fn http_on_another_host_is_an_invalid_argument() {
        let error = url_of("http://api.example.com").unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("http://api.example.com"));
    }

    #[test]
    fn a_user_name_or_password_is_rejected_without_repeating_the_url() {
        let errors = [
            "https://ada:s3cret@api.example.com",
            "https://ada@api.example.com",
        ]
        .map(|raw| url_of(raw).unwrap_err());

        for error in errors {
            assert_eq!(error.code, ErrorCode::InvalidArgument);
            assert!(!error.message.contains("ada"), "{}", error.message);
            assert!(!error.message.contains("s3cret"), "{}", error.message);
        }
    }

    #[test]
    fn a_query_or_fragment_is_an_invalid_argument() {
        let codes = [
            "https://api.example.com/?debug=1",
            "https://api.example.com/#top",
        ]
        .map(|raw| url_of(raw).unwrap_err().code);

        assert_eq!(codes, [ErrorCode::InvalidArgument; 2]);
    }

    #[test]
    fn a_scheme_other_than_http_or_https_is_an_invalid_argument() {
        assert_eq!(
            url_of("ftp://api.example.com").unwrap_err().code,
            ErrorCode::InvalidArgument
        );
    }
}
