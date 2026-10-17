//! Updates of the `imify` binary: the release lookup, how the binary was installed, which
//! command updates it, and the hint after a command when a newer release exists.

pub mod check;
pub mod install;

use std::path::Path;
use std::time::Duration;

use imify::update::{Installation, ReleaseClient, ReleaseSource, Version};

use crate::dirs::config_home;
use crate::output::{CliError, Environment, ErrorCode};

/// The variable that points the release lookup and the installer at a GitHub Enterprise
/// Server or a mirror, as the imify installer reads it.
const GHE_BASE_URL_VARIABLE: &str = "IMIFY_CLI_INSTALLER_GHE_BASE_URL";

/// The release client: GitHub, or the GitHub Enterprise Server that
/// `IMIFY_CLI_INSTALLER_GHE_BASE_URL` names, with its API below `/api/v3`. `timeout` limits
/// each request; `None` keeps the SDK's default.
///
/// # Errors
///
/// Returns `invalid_argument` naming `IMIFY_CLI_INSTALLER_GHE_BASE_URL` when the SDK rejects
/// its URL.
pub fn release_client(
    env: &Environment,
    source: ReleaseSource,
    timeout: Option<Duration>,
) -> Result<ReleaseClient, CliError> {
    let mut builder = ReleaseClient::builder(source);
    if let Some(base) = env
        .installer_ghe_base_url
        .as_deref()
        .filter(|base| !base.is_empty())
    {
        let base = base.trim_end_matches('/');
        builder = builder
            .api_url(&format!("{base}/api/v3"))
            .download_url(base);
    }
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    builder.build().map_err(|error| {
        CliError::new(ErrorCode::InvalidArgument, error.to_string()).with_hint(format!(
            "Set `{GHE_BASE_URL_VARIABLE}` to an https URL, or unset it."
        ))
    })
}

/// The version of the running binary.
///
/// # Errors
///
/// Returns `internal` when the package version is not a semantic version.
pub fn running_version() -> Result<Version, CliError> {
    Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| {
        CliError::new(
            ErrorCode::Internal,
            format!("The version of this imify is not a semantic version: {error}"),
        )
    })
}

/// The install from the installer's receipt that contains `executable`; `None` without a
/// receipt, or without a configuration directory to look for one in.
///
/// # Errors
///
/// Returns `self_update_failed` when the receipt cannot be read or has an unsupported layout.
pub fn installation(
    env: &Environment,
    executable: &Path,
    source: &ReleaseSource,
) -> Result<Option<Installation>, CliError> {
    let Some(config_home) = config_home(env) else {
        return Ok(None);
    };
    Ok(Installation::from_receipt(
        &source.receipt_path(&config_home),
        executable,
        source,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invalid_ghe_base_url_is_an_invalid_argument_naming_the_variable() {
        let env = Environment {
            installer_ghe_base_url: Some("http://ghe.example.com".to_owned()),
            ..Environment::default()
        };

        let error = release_client(&env, ReleaseSource::imify_cli(), None).unwrap_err();

        assert_eq!(
            (error.code, error.hint.as_deref()),
            (
                ErrorCode::InvalidArgument,
                Some("Set `IMIFY_CLI_INSTALLER_GHE_BASE_URL` to an https URL, or unset it.")
            )
        );
    }
}
