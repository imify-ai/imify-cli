//! The login in the OS keychain: the macOS Keychain or the Secret Service on Linux.

use std::fmt;
use std::sync::{Arc, OnceLock};

use imify::Error;
use imify::auth::Credentials;
use keyring_core::Entry;

/// The keychain service name of every imify login.
const SERVICE: &str = "imify-cli";

type Backend = Arc<keyring_core::CredentialStore>;
type Opener = dyn Fn() -> keyring_core::Result<Backend> + Send + Sync;

/// A keychain backend, opened on first use.
///
/// Opening the Secret Service blocks on D-Bus, so it happens on the thread of the first
/// operation, which callers run outside the async runtime.
pub struct Keychain {
    open: Box<Opener>,
    opened: OnceLock<Result<Backend, String>>,
}

impl Keychain {
    /// The keychain of the operating system.
    pub fn system() -> Self {
        Self::opened_by(open_system)
    }

    /// An in-memory keychain without persistence.
    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::opened_by(|| Ok(keyring_core::mock::Store::new()?))
    }

    /// An in-memory keychain whose next operation on the login of `account` fails as on a
    /// locked keyring.
    #[cfg(test)]
    pub fn locked(account: &str) -> Self {
        let account = account.to_owned();
        Self::opened_by(move || {
            let store = keyring_core::mock::Store::new()?;
            let entry = keyring_core::api::CredentialStoreApi::build(
                store.as_ref(),
                SERVICE,
                &account,
                None,
            )?;
            if let Some(credential) = entry.as_any().downcast_ref::<keyring_core::mock::Cred>() {
                credential.set_error(keyring_core::Error::NoStorageAccess(
                    "the keyring is locked".into(),
                ));
            }
            Ok(store)
        })
    }

    /// A keychain that cannot be opened, as on a machine without a keychain service.
    #[cfg(test)]
    pub fn unavailable() -> Self {
        Self::opened_by(|| Err(keyring_core::Error::NoDefaultStore))
    }

    fn opened_by(open: impl Fn() -> keyring_core::Result<Backend> + Send + Sync + 'static) -> Self {
        Self {
            open: Box::new(open),
            opened: OnceLock::new(),
        }
    }

    fn entry(&self, account: &str) -> Result<Entry, KeychainError> {
        let backend = self
            .opened
            .get_or_init(|| (self.open)().map_err(|error| error.to_string()))
            .as_ref()
            .map_err(|reason| KeychainError::Unavailable(reason.clone()))?;
        backend
            .build(SERVICE, account, None)
            .map_err(KeychainError::from)
    }
}

impl fmt::Debug for Keychain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Keychain")
            .field("opened", &self.opened.get().map(Result::is_ok))
            .finish_non_exhaustive()
    }
}

#[cfg(target_os = "macos")]
fn open_system() -> keyring_core::Result<Backend> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(target_os = "linux")]
fn open_system() -> keyring_core::Result<Backend> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn open_system() -> keyring_core::Result<Backend> {
    Err(keyring_core::Error::NoDefaultStore)
}

/// A keychain operation failed.
#[derive(Debug, thiserror::Error)]
pub enum KeychainError {
    /// The keychain backend cannot be opened: there is no keychain service on this machine.
    #[error("no keychain is available: {0}")]
    Unavailable(String),
    /// The keychain exists but refuses access, such as a locked keyring.
    #[error("the keychain refuses access: {0}")]
    Locked(String),
    /// The keychain failed, or holds an entry that is not an imify login.
    #[error("the keychain failed: {0}")]
    Failed(String),
}

/// Errors of an opened backend. Only failing to open the backend makes it unavailable.
impl From<keyring_core::Error> for KeychainError {
    fn from(error: keyring_core::Error) -> Self {
        match error {
            keyring_core::Error::NoStorageAccess(_) => Self::Locked(error.to_string()),
            other => Self::Failed(other.to_string()),
        }
    }
}

impl From<KeychainError> for Error {
    fn from(error: KeychainError) -> Self {
        Error::CredentialStore {
            source: Box::new(error),
        }
    }
}

/// The login of one host in the keychain: service `imify-cli`, account the host key, secret
/// the serialized credentials.
#[derive(Debug)]
pub struct KeychainStore {
    keychain: Keychain,
    account: String,
}

impl KeychainStore {
    pub fn new(keychain: Keychain, host_key: &str) -> Self {
        Self {
            keychain,
            account: host_key.to_owned(),
        }
    }

    pub fn try_load(&self) -> Result<Option<Credentials>, KeychainError> {
        let secret = match self.keychain.entry(&self.account)?.get_password() {
            Ok(secret) => secret,
            Err(keyring_core::Error::NoEntry) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        serde_json::from_str(&secret).map(Some).map_err(|error| {
            KeychainError::Failed(format!(
                "the entry for {} is not an imify login: {error}",
                self.account
            ))
        })
    }

    pub fn try_save(&self, credentials: &Credentials) -> Result<(), KeychainError> {
        let secret = serde_json::to_string(credentials)
            .map_err(|error| KeychainError::Failed(error.to_string()))?;
        Ok(self.keychain.entry(&self.account)?.set_password(&secret)?)
    }

    /// Returns `true` when an entry existed.
    pub fn try_delete(&self) -> Result<bool, KeychainError> {
        match self.keychain.entry(&self.account)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring_core::Error::NoEntry) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}
