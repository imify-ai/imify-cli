//! Where a login is kept: the OS keychain, or a file readable only by the user.
//!
//! Logins are keyed by host: [`host_key`] of the API URL. `IMIFY_CREDENTIAL_STORE` selects
//! the store ([`StoreChoice`]); `IMIFY_CONFIG_DIR` the directory of the file.

mod file;
mod keychain;

use std::path::PathBuf;

use imify::Error;
use imify::auth::{CredentialStore, Credentials};
use url::Url;

pub use self::file::{FileStore, FileStoreError};
pub use self::keychain::{Keychain, KeychainError, KeychainStore};
use crate::output::{CliError, Environment, ErrorCode};

/// The variable that selects the credential store.
pub const STORE_VARIABLE: &str = "IMIFY_CREDENTIAL_STORE";

/// The variable that sets the directory of the credentials file.
pub const CONFIG_DIR_VARIABLE: &str = "IMIFY_CONFIG_DIR";

/// The key of the login for `api_url`: scheme, host, port unless it is the scheme's
/// default, and path without a trailing slash. `https://API.example.com:443/` gives
/// `https://api.example.com`.
pub fn host_key(api_url: &Url) -> String {
    let host = api_url.host_str().unwrap_or_default();
    let port = api_url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let path = api_url.path().trim_end_matches('/');
    format!("{}://{host}{port}{path}", api_url.scheme())
}

/// The store `IMIFY_CREDENTIAL_STORE` selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreChoice {
    /// The keychain; the file when no keychain is available.
    Auto,
    Keychain,
    File,
}

impl StoreChoice {
    /// Parses the value of `IMIFY_CREDENTIAL_STORE`. Unset or empty is [`StoreChoice::Auto`].
    ///
    /// # Errors
    ///
    /// Returns `invalid_argument` for any other value than `auto`, `keychain` or `file`.
    pub fn parse(value: Option<&str>) -> Result<Self, CliError> {
        match value.map(str::trim) {
            None | Some("" | "auto") => Ok(Self::Auto),
            Some("keychain") => Ok(Self::Keychain),
            Some("file") => Ok(Self::File),
            Some(other) => Err(CliError::new(
                ErrorCode::InvalidArgument,
                format!("{STORE_VARIABLE} has the unsupported value '{other}'."),
            )
            .with_hint(format!(
                "Set {STORE_VARIABLE} to `auto`, `keychain` or `file`, or unset it."
            ))),
        }
    }
}

/// The directory of the credentials file: `IMIFY_CONFIG_DIR`, or the platform's
/// configuration directory for imify.
///
/// # Errors
///
/// Returns `internal` when `IMIFY_CONFIG_DIR` is unset and the home directory is unknown.
pub fn config_dir(env: &Environment) -> Result<PathBuf, CliError> {
    if let Some(dir) = env
        .config_dir
        .as_ref()
        .filter(|dir| !dir.as_os_str().is_empty())
    {
        return Ok(dir.clone());
    }
    directories::ProjectDirs::from("ai", "imify", "imify")
        .map(|dirs| dirs.config_dir().to_path_buf())
        .ok_or_else(|| {
            CliError::new(
                ErrorCode::Internal,
                "Cannot determine the configuration directory: the home directory is unknown.",
            )
            .with_hint(format!("Set `{CONFIG_DIR_VARIABLE}` to a directory."))
        })
}

/// Where [`HostStore::save_reporting`] stored the credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    Keychain,
    /// The file `IMIFY_CREDENTIAL_STORE=file` selects.
    File(PathBuf),
    /// The file, because no keychain service can be opened; `reason` says why.
    FileFallback {
        path: PathBuf,
        reason: String,
    },
}

/// The outcome of [`HostStore::save_reporting`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveReport {
    pub saved: Saved,
    /// Why an older login of the host could not be removed from the file after the login
    /// was saved in the keychain.
    pub cleanup_failure: Option<String>,
}

impl Saved {
    /// The `storage` value of `imify login --json`: `keychain` or `file`.
    pub fn storage(&self) -> &'static str {
        match self {
            Self::Keychain => "keychain",
            Self::File(_) | Self::FileFallback { .. } => "file",
        }
    }
}

/// The credential store of one host, as `IMIFY_CREDENTIAL_STORE` selects it.
///
/// With `auto`, `load` reads the keychain, then the file; `save` writes to the keychain and
/// removes the host's entry from the file, or writes to the file when no keychain service can
/// be opened; `delete` removes the login from both. A keychain that exists but refuses access,
/// such as a locked keyring, is an error, never a reason to use the file.
#[derive(Debug)]
pub enum HostStore {
    Auto {
        keychain: KeychainStore,
        file: FileStore,
    },
    Keychain(KeychainStore),
    File(FileStore),
}

impl HostStore {
    /// The store for the login of `host_key`.
    ///
    /// # Errors
    ///
    /// Returns `invalid_argument` for an unsupported `IMIFY_CREDENTIAL_STORE`, and
    /// `internal` when the configuration directory is unknown.
    pub fn open(env: &Environment, host_key: &str, keychain: Keychain) -> Result<Self, CliError> {
        let file = || config_dir(env).map(|dir| FileStore::new(&dir, host_key));
        Ok(match StoreChoice::parse(env.credential_store.as_deref())? {
            StoreChoice::Auto => Self::Auto {
                keychain: KeychainStore::new(keychain, host_key),
                file: file()?,
            },
            StoreChoice::Keychain => Self::Keychain(KeychainStore::new(keychain, host_key)),
            StoreChoice::File => Self::File(file()?),
        })
    }

    /// Saves `credentials` and reports where.
    ///
    /// # Errors
    ///
    /// Returns [`Error::CredentialStore`] when the selected store fails.
    pub fn save_reporting(&self, credentials: &Credentials) -> Result<SaveReport, Error> {
        let saved = |saved| SaveReport {
            saved,
            cleanup_failure: None,
        };
        match self {
            Self::Auto { keychain, file } => match keychain.try_save(credentials) {
                Ok(()) => Ok(SaveReport {
                    saved: Saved::Keychain,
                    cleanup_failure: file.delete().err().map(|error| error.to_string()),
                }),
                Err(KeychainError::Unavailable(reason)) => {
                    file.save(credentials)?;
                    Ok(saved(Saved::FileFallback {
                        path: file.path().to_path_buf(),
                        reason,
                    }))
                }
                Err(error) => Err(error.into()),
            },
            Self::Keychain(keychain) => {
                keychain.try_save(credentials)?;
                Ok(saved(Saved::Keychain))
            }
            Self::File(file) => {
                file.save(credentials)?;
                Ok(saved(Saved::File(file.path().to_path_buf())))
            }
        }
    }
}

impl CredentialStore for HostStore {
    fn load(&self) -> Result<Option<Credentials>, Error> {
        match self {
            Self::Auto { keychain, file } => match keychain.try_load() {
                Ok(Some(credentials)) => Ok(Some(credentials)),
                Ok(None) | Err(KeychainError::Unavailable(_)) => file.load(),
                Err(error) => Err(error.into()),
            },
            Self::Keychain(keychain) => Ok(keychain.try_load()?),
            Self::File(file) => file.load(),
        }
    }

    fn save(&self, credentials: &Credentials) -> Result<(), Error> {
        self.save_reporting(credentials).map(drop)
    }

    fn delete(&self) -> Result<bool, Error> {
        match self {
            Self::Auto { keychain, file } => {
                let in_keychain = match keychain.try_delete() {
                    Ok(existed) => Ok(existed),
                    Err(KeychainError::Unavailable(_)) => Ok(false),
                    Err(error) => Err(Error::from(error)),
                };
                let in_file = file.delete();
                Ok(in_keychain? | in_file?)
            }
            Self::Keychain(keychain) => Ok(keychain.try_delete()?),
            Self::File(file) => file.delete(),
        }
    }
}

#[cfg(test)]
mod tests;
