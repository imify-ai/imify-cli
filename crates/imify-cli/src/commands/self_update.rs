//! `imify self update`: updates an install of the imify installer to the latest release.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use imify::update::{Installation, Release, ReleaseClient, ReleaseSource, Version};
use serde_json::json;

use super::write_result;
use crate::dirs::config_home;
use crate::output::renderer::{Render, Renderer};
use crate::output::{CliError, Environment, ErrorCode};
use crate::update::install::InstallMethod;

/// The variable that points the release lookup and the installer at a GitHub Enterprise
/// Server or a mirror, as the imify installer reads it.
const GHE_BASE_URL_VARIABLE: &str = "IMIFY_CLI_INSTALLER_GHE_BASE_URL";

/// `imify self update`: looks up the latest release and, when it is newer than the running
/// version, runs its installer for the install prefix in the install receipt.
///
/// # Errors
///
/// Returns `self_update_unsupported` when no install receipt describes the running
/// executable, with a hint naming the update command of its install method;
/// `self_update_failed` when the receipt is unusable or the installer fails; `network`,
/// `server` and `rate_limited` when the lookup or the download fails; `invalid_argument` for
/// an invalid `IMIFY_CLI_INSTALLER_GHE_BASE_URL`; and `internal` when stdout cannot be
/// written.
pub async fn update(env: &Environment, renderer: &mut dyn Renderer) -> Result<(), CliError> {
    let executable = std::env::current_exe().map_err(|error| {
        CliError::new(
            ErrorCode::Internal,
            format!("Cannot locate the running executable: {error}"),
        )
    })?;
    let source = ReleaseSource::imify_cli();
    let Some(installation) = installation(env, &executable, &source)? else {
        return Err(unsupported(&executable, env));
    };
    let client = release_client(env, source)?;
    let current = running_version()?;
    let latest = client.latest().await?;
    let updated = latest.version > current;
    if updated {
        install(&client, &installation, &latest).await?;
    }
    let version = if updated {
        latest.version.clone()
    } else {
        current.clone()
    };
    write_result(
        renderer,
        &Outcome {
            updated,
            previous_version: current,
            version,
            latest: latest.version,
            bin_dir: installation.bin_dir(),
            install_prefix: installation.install_prefix().to_owned(),
        },
    )
}

/// Runs the installer of `release` and logs its output at debug level, also when it fails.
async fn install(
    client: &ReleaseClient,
    installation: &Installation,
    release: &Release,
) -> Result<(), CliError> {
    let result = client.install(installation, release).await;
    let output = match &result {
        Ok(output) => Some(output.as_str()),
        Err(imify::Error::SelfUpdate {
            installer_output, ..
        }) => installer_output.as_deref(),
        Err(_) => None,
    };
    if let Some(output) = output {
        tracing::debug!("installer output:\n{output}");
    }
    result.map(drop).map_err(CliError::from)
}

/// The install from the installer's receipt that contains `executable`; `None` without a
/// receipt, or without a configuration directory to look for one in.
fn installation(
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

/// The `self_update_unsupported` error, with the update command of the install method of
/// `executable` as the hint.
fn unsupported(executable: &Path, env: &Environment) -> CliError {
    CliError::new(
        ErrorCode::SelfUpdateUnsupported,
        "This imify was not installed by the imify installer and cannot update itself.",
    )
    .with_hint(InstallMethod::detect(executable, env).hint())
}

/// The release client: GitHub, or the GitHub Enterprise Server that
/// `IMIFY_CLI_INSTALLER_GHE_BASE_URL` names, with its API below `/api/v3`.
fn release_client(env: &Environment, source: ReleaseSource) -> Result<ReleaseClient, CliError> {
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
    builder.build().map_err(|error| {
        CliError::new(ErrorCode::InvalidArgument, error.to_string()).with_hint(format!(
            "Set `{GHE_BASE_URL_VARIABLE}` to an https URL, or unset it."
        ))
    })
}

fn running_version() -> Result<Version, CliError> {
    Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| {
        CliError::new(
            ErrorCode::Internal,
            format!("The version of this imify is not a semantic version: {error}"),
        )
    })
}

/// The result of `imify self update`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    updated: bool,
    previous_version: Version,
    version: Version,
    latest: Version,
    bin_dir: PathBuf,
    install_prefix: PathBuf,
}

impl Render for Outcome {
    fn kind(&self) -> &'static str {
        "self_update"
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "updated": self.updated,
            "previous_version": self.previous_version.to_string(),
            "version": self.version.to_string(),
            "install_prefix": self.install_prefix.display().to_string(),
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        if self.updated {
            writeln!(
                w,
                "Updated imify from {} to {} in {}.",
                self.previous_version,
                self.version,
                self.bin_dir.display()
            )
        } else if self.version == self.latest {
            writeln!(w, "imify {} is the latest release.", self.version)
        } else {
            writeln!(
                w,
                "imify {} is newer than the latest release, {}.",
                self.version, self.latest
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(updated: bool, version: &str, latest: &str) -> Outcome {
        Outcome {
            updated,
            previous_version: Version::parse("0.1.0").unwrap(),
            version: Version::parse(version).unwrap(),
            latest: Version::parse(latest).unwrap(),
            bin_dir: PathBuf::from("/home/ada/.cargo/bin"),
            install_prefix: PathBuf::from("/home/ada/.cargo"),
        }
    }

    fn human(outcome: &Outcome) -> String {
        let mut text = Vec::new();
        outcome.human(&mut text).unwrap();
        String::from_utf8(text).unwrap()
    }

    #[test]
    fn an_update_names_both_versions_and_the_bin_directory() {
        assert_eq!(
            human(&outcome(true, "0.2.0", "0.2.0")),
            "Updated imify from 0.1.0 to 0.2.0 in /home/ada/.cargo/bin.\n"
        );
    }

    #[test]
    fn the_latest_version_says_so() {
        assert_eq!(
            human(&outcome(false, "0.2.0", "0.2.0")),
            "imify 0.2.0 is the latest release.\n"
        );
    }

    #[test]
    fn a_version_newer_than_the_latest_release_names_both() {
        assert_eq!(
            human(&outcome(false, "0.3.0", "0.2.0")),
            "imify 0.3.0 is newer than the latest release, 0.2.0.\n"
        );
    }

    #[test]
    fn an_invalid_ghe_base_url_is_an_invalid_argument_naming_the_variable() {
        let env = Environment {
            installer_ghe_base_url: Some("http://ghe.example.com".to_owned()),
            ..Environment::default()
        };

        let error = release_client(&env, ReleaseSource::imify_cli()).unwrap_err();

        assert_eq!(
            (error.code, error.hint.as_deref()),
            (
                ErrorCode::InvalidArgument,
                Some("Set `IMIFY_CLI_INSTALLER_GHE_BASE_URL` to an https URL, or unset it.")
            )
        );
    }
}
