//! The install receipt of the dist shell installer, and running that installer.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use reqwest::{StatusCode, redirect};
use serde::Deserialize;
use tokio::io::AsyncWriteExt as _;

use super::{DEFAULT_DOWNLOAD_URL, Release, ReleaseClient, ReleaseSource};
use crate::Error;
use crate::client::{MAX_REDIRECTS, network_error, read_small_body, redirect_not_followed};
use crate::error::from_response;

/// The shell that runs the installer, with the script on stdin.
const SHELL: &str = "/bin/sh";

/// Installer variables, after the app prefix, that would change where the installer downloads
/// from, where it installs, or whether it writes a receipt. They are removed from the
/// installer's environment; [`ReleaseClient::install`] sets the ones it needs.
const APP_VARIABLES: [&str; 7] = [
    "DOWNLOAD_URL",
    "INSTALLER_GHE_BASE_URL",
    "INSTALLER_GITHUB_BASE_URL",
    "INSTALL_DIR",
    "UNMANAGED_INSTALL",
    "DISABLE_UPDATE",
    "NO_MODIFY_PATH",
];

/// Variables every dist installer reads, with the same effect as [`APP_VARIABLES`].
const SHARED_VARIABLES: [&str; 3] = [
    "INSTALLER_DOWNLOAD_URL",
    "CARGO_DIST_FORCE_INSTALL_DIR",
    "INSTALLER_NO_MODIFY_PATH",
];

/// An install of the running program by its dist shell installer, read from the install
/// receipt.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use imify::update::{Installation, ReleaseSource};
///
/// let source = ReleaseSource::imify_cli();
/// let receipt = source.receipt_path(Path::new("/home/ada/.config"));
/// let executable = std::env::current_exe().expect("the running executable");
/// if let Some(installation) = Installation::from_receipt(&receipt, &executable, &source)? {
///     println!("installed in {}", installation.bin_dir().display());
/// }
/// # Ok::<(), imify::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    install_prefix: PathBuf,
    modify_path: bool,
}

impl Installation {
    /// Reads the receipt at `receipt` and returns the install it describes when that install
    /// contains `executable`: the receipt names `source`, lists the file name of
    /// `executable` as a binary, and `<install_prefix>/bin/<name>` is the same file as
    /// `executable`. Returns `Ok(None)` when there is no receipt or it describes another
    /// install.
    ///
    /// # Errors
    ///
    /// Returns [`Error::SelfUpdate`] when the receipt cannot be read or parsed, or has an
    /// install layout other than `cargo-home` or `unspecified`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use imify::update::{Installation, ReleaseSource};
    ///
    /// let installation = Installation::from_receipt(
    ///     Path::new("/no/such/receipt.json"),
    ///     Path::new("/usr/local/bin/imify"),
    ///     &ReleaseSource::imify_cli(),
    /// )?;
    /// assert_eq!(installation, None);
    /// # Ok::<(), imify::Error>(())
    /// ```
    pub fn from_receipt(
        receipt: &Path,
        executable: &Path,
        source: &ReleaseSource,
    ) -> Result<Option<Self>, Error> {
        let text = match std::fs::read(receipt) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(receipt_error(format!(
                    "cannot read the install receipt {}: {error}",
                    receipt.display()
                )));
            }
        };
        let parsed: Receipt = serde_json::from_slice(&text).map_err(|error| {
            receipt_error(format!(
                "the install receipt {} is invalid: {error}",
                receipt.display()
            ))
        })?;
        if !parsed.source.names(source) {
            return Ok(None);
        }
        if !matches!(parsed.install_layout.as_str(), "cargo-home" | "unspecified") {
            return Err(receipt_error(format!(
                "the install receipt {} has the install layout {:?}, which cannot be updated",
                receipt.display(),
                parsed.install_layout
            )));
        }
        let installation = Self {
            install_prefix: parsed.install_prefix,
            modify_path: parsed.modify_path,
        };
        let contains_executable = executable
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| parsed.binaries.iter().any(|binary| binary == name))
            .is_some_and(|name| same_file(&installation.bin_dir().join(name), executable));
        Ok(contains_executable.then_some(installation))
    }

    /// The install prefix from the receipt, such as `/home/ada/.cargo`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn run(installation: imify::update::Installation) {
    /// println!("prefix {}", installation.install_prefix().display());
    /// # }
    /// ```
    #[must_use]
    pub fn install_prefix(&self) -> &Path {
        &self.install_prefix
    }

    /// The directory of the installed binaries: `bin` below the install prefix.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn run(installation: imify::update::Installation) {
    /// println!("binaries in {}", installation.bin_dir().display());
    /// # }
    /// ```
    #[must_use]
    pub fn bin_dir(&self) -> PathBuf {
        self.install_prefix.join("bin")
    }
}

/// The fields of a dist install receipt that decide whether and how to update; other fields
/// are ignored.
#[derive(Deserialize)]
struct Receipt {
    install_prefix: PathBuf,
    #[serde(default = "unspecified_layout")]
    install_layout: String,
    binaries: Vec<String>,
    #[serde(default = "modifies_path")]
    modify_path: bool,
    source: ReceiptSource,
}

fn unspecified_layout() -> String {
    "unspecified".to_owned()
}

fn modifies_path() -> bool {
    true
}

#[derive(Deserialize)]
struct ReceiptSource {
    app_name: String,
    name: String,
    owner: String,
    release_type: String,
}

impl ReceiptSource {
    fn names(&self, source: &ReleaseSource) -> bool {
        self.release_type == "github"
            && self.owner == source.owner
            && self.name == source.repo
            && self.app_name == source.app_name
    }
}

/// Whether `a` and `b` resolve to the same existing file.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn receipt_error(message: String) -> Error {
    Error::SelfUpdate {
        message,
        installer_output: None,
    }
}

impl ReleaseClient {
    /// Downloads the shell installer of `release` and runs it with `/bin/sh` to install into
    /// the prefix of `installation`.
    ///
    /// The installer comes from
    /// `<download>/<owner>/<repo>/releases/download/<tag>/<app>-installer.sh`. The download
    /// follows up to 5 redirects to `https` URLs, and must start with `#!`. The installer
    /// runs with `<APP>_INSTALL_DIR` set to the install prefix, with
    /// `<APP>_NO_MODIFY_PATH=1` when the receipt records that the installer left `PATH`
    /// unchanged, and with `<APP>_INSTALLER_GHE_BASE_URL` set to the download URL when that
    /// is not `https://github.com`. Other installer variables that change the download
    /// source, the install directory or the receipt are removed from its environment.
    /// `<APP>` is the app name in upper case with `-` replaced by `_`. The installer's
    /// output is captured.
    ///
    /// Returns the installer's stdout followed by its stderr.
    ///
    /// # Errors
    ///
    /// - [`Error::SelfUpdate`] when the release has no installer (404), the download is not
    ///   a shell script, or the installer cannot be started or exits with an error. When it
    ///   ran, `installer_output` holds its output.
    /// - [`Error::Network`], [`Error::RateLimited`], [`Error::Server`] and
    ///   [`Error::UnexpectedResponse`] when the download fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(
    /// #     client: imify::update::ReleaseClient,
    /// #     installation: imify::update::Installation,
    /// # ) -> Result<(), imify::Error> {
    /// let latest = client.latest().await?;
    /// let output = client.install(&installation, &latest).await?;
    /// eprintln!("{output}");
    /// # Ok(()) }
    /// ```
    pub async fn install(
        &self,
        installation: &Installation,
        release: &Release,
    ) -> Result<String, Error> {
        let script = self.download_installer(release).await?;
        self.run_installer(installation, script).await
    }

    async fn download_installer(&self, release: &Release) -> Result<Vec<u8>, Error> {
        let url = self.installer_url(release)?;
        let response = self
            .download_http
            .get(url.clone())
            .send()
            .await
            .map_err(network_error)?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = read_small_body(response).await?;
        if status == StatusCode::NOT_FOUND {
            return Err(Error::SelfUpdate {
                message: format!(
                    "release {} has no {}",
                    release.tag,
                    self.source.installer_name()
                ),
                installer_output: None,
            });
        }
        if status.is_redirection() {
            return Err(redirect_not_followed(status, &headers));
        }
        if !status.is_success() {
            return Err(from_response(status.as_u16(), &headers, &body));
        }
        if !body.starts_with(b"#!") {
            return Err(Error::SelfUpdate {
                message: format!("the download from {url} is not a shell script"),
                installer_output: None,
            });
        }
        Ok(body)
    }

    async fn run_installer(
        &self,
        installation: &Installation,
        script: Vec<u8>,
    ) -> Result<String, Error> {
        let prefix = self.source.variable_prefix();
        let mut command = tokio::process::Command::new(SHELL);
        command
            .arg("-s")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for name in APP_VARIABLES {
            command.env_remove(format!("{prefix}_{name}"));
        }
        for name in SHARED_VARIABLES {
            command.env_remove(name);
        }
        command.env(
            format!("{prefix}_INSTALL_DIR"),
            installation.install_prefix(),
        );
        if !installation.modify_path {
            command.env(format!("{prefix}_NO_MODIFY_PATH"), "1");
        }
        if self.download_url.as_str() != format!("{DEFAULT_DOWNLOAD_URL}/") {
            command.env(
                format!("{prefix}_INSTALLER_GHE_BASE_URL"),
                self.download_url.as_str().trim_end_matches('/'),
            );
        }

        let not_started = |error: io::Error| Error::SelfUpdate {
            message: format!("cannot run the installer with {SHELL}: {error}"),
            installer_output: None,
        };
        let mut child = command.spawn().map_err(not_started)?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| not_started(io::Error::other("stdin is not piped")))?;
        let write = async move {
            let written = stdin.write_all(&script).await;
            drop(stdin);
            written
        };
        let (written, output) = tokio::join!(write, child.wait_with_output());
        let output = output.map_err(not_started)?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        if !output.status.success() {
            return Err(Error::SelfUpdate {
                message: format!("the installer exited with {}", output.status),
                installer_output: Some(text),
            });
        }
        if let Err(error) = written {
            return Err(Error::SelfUpdate {
                message: format!("cannot pass the installer to {SHELL}: {error}"),
                installer_output: Some(text),
            });
        }
        Ok(text)
    }
}

/// Follows up to [`MAX_REDIRECTS`] redirects of any status to `https` URLs, or within the
/// origin of the first request.
pub(super) fn download_redirect_policy() -> redirect::Policy {
    redirect::Policy::custom(|attempt| {
        let previous = attempt.previous();
        let next = attempt.url();
        let allowed = previous.len() <= MAX_REDIRECTS
            && (next.scheme() == "https"
                || previous
                    .first()
                    .is_some_and(|first| first.origin() == next.origin()));
        if allowed {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}
