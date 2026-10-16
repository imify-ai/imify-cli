//! The error type returned by every fallible SDK operation.

use std::path::PathBuf;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde_json::Value;

/// An error returned by the SDK.
///
/// Messages come from the API's `detail` field when the response has one, and from the
/// HTTP reason phrase otherwise. No variant contains the access token.
///
/// # Examples
///
/// ```
/// use imify::Error;
///
/// fn describe(error: &Error) -> String {
///     match error {
///         Error::Auth { .. } => "sign in again".to_owned(),
///         Error::RateLimited { retry_after: Some(delay) } => format!("retry in {delay:?}"),
///         other => other.to_string(),
///     }
/// }
///
/// let error = Error::NotFound { message: "Dataset not found".into() };
/// assert_eq!(describe(&error), "not found: Dataset not found");
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The API rejected the credentials (401, or 403 with a JSON body), or the token source
    /// failed to produce a token (`status` is `None`).
    #[error("authentication failed: {message}")]
    Auth {
        /// HTTP status of the response, `None` when no request was sent.
        status: Option<u16>,
        /// Reason given by the API or the token source.
        message: String,
    },

    /// The requested resource does not exist (404).
    #[error("not found: {message}")]
    NotFound {
        /// Reason given by the API.
        message: String,
    },

    /// The API rejected the request content (400 or 422).
    #[error("validation failed: {message}")]
    Validation {
        /// Reason given by the API.
        message: String,
        /// One entry per invalid field; empty when the API names no field.
        fields: Vec<FieldError>,
    },

    /// The request conflicts with the current state of the resource (409).
    #[error("conflict: {message}")]
    Conflict {
        /// Reason given by the API.
        message: String,
    },

    /// The API or the network edge throttled the request (429, or 403 without a JSON body).
    #[error("rate limited")]
    RateLimited {
        /// Delay requested by the `Retry-After` header, when present in seconds.
        retry_after: Option<Duration>,
    },

    /// The API failed to process the request (5xx).
    #[error("server error {status}: {message}")]
    Server {
        /// HTTP status of the response.
        status: u16,
        /// Reason given by the API.
        message: String,
    },

    /// The request did not complete: connection, TLS, timeout or body read failure.
    #[error("network error: {source}")]
    Network {
        /// The underlying transport error.
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The API answered with a status the SDK does not handle, or with a success body that
    /// does not match the expected type.
    #[error("unexpected response {status}: {message}")]
    UnexpectedResponse {
        /// HTTP status of the response.
        status: u16,
        /// Reason given by the API, or the decoding failure.
        message: String,
    },

    /// A client setting is invalid.
    #[error("invalid configuration: {message}")]
    Config {
        /// Which setting is invalid and why.
        message: String,
    },

    /// A local file or directory cannot be read, or changed while it was being read.
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// let error = imify::validate::validate_dataset(Path::new("/no/such/dataset")).unwrap_err();
    /// assert!(matches!(error, imify::Error::Io { ref path, .. } if path == Path::new("/no/such/dataset")));
    /// ```
    #[error("cannot read {}: {source}", path.display())]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// A browser login or the refresh of a stored login failed. The message names no
    /// token, authorization code, PKCE verifier, `state` or `nonce`.
    ///
    /// ```
    /// use imify::{Error, LoginFailure};
    ///
    /// fn needs_new_login(error: &Error) -> bool {
    ///     matches!(error, Error::Login { reason: LoginFailure::Expired, .. })
    /// }
    ///
    /// let error = Error::Login {
    ///     reason: LoginFailure::Expired,
    ///     message: "the stored login has expired".into(),
    /// };
    /// assert!(needs_new_login(&error));
    /// ```
    #[error("login failed: {message}")]
    Login {
        /// Which step failed.
        reason: LoginFailure,
        /// What failed, without secrets.
        message: String,
    },

    /// A [`CredentialStore`](crate::auth::CredentialStore) failed to load, save or delete
    /// credentials.
    #[error("credential store failed: {source}")]
    CredentialStore {
        /// The error the store returned.
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The resume journal of an upload cannot be used.
    ///
    /// ```
    /// use imify::{Error, JournalError};
    ///
    /// fn needs_restart(error: &Error) -> bool {
    ///     matches!(
    ///         error,
    ///         Error::Journal { reason: JournalError::Mismatch(_) | JournalError::Corrupt(_), .. }
    ///     )
    /// }
    ///
    /// let error = Error::Journal {
    ///     path: "uploads/3f2c.jsonl".into(),
    ///     reason: JournalError::Corrupt("line 2 is not valid JSON".into()),
    /// };
    /// assert!(needs_restart(&error));
    /// ```
    #[error("upload journal {}: {reason}", path.display())]
    Journal {
        /// The journal file, or the journal directory when it cannot be created.
        path: PathBuf,
        /// Why the journal cannot be used.
        #[source]
        reason: JournalError,
    },
}

/// Why the resume journal of an upload cannot be used, reported in [`Error::Journal`].
///
/// # Examples
///
/// ```
/// use imify::JournalError;
///
/// assert_eq!(JournalError::Missing.to_string(), "no journal exists for the dataset directory");
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JournalError {
    /// [`ResumePolicy::Required`](crate::ingest::ResumePolicy::Required) is set and no
    /// journal exists for the dataset directory.
    #[error("no journal exists for the dataset directory")]
    Missing,
    /// The journal or its directory cannot be read, written or removed.
    #[error("{0}")]
    Io(#[source] std::io::Error),
    /// The journal is not valid JSON Lines or has an unknown `version`.
    #[error("corrupt: {0}")]
    Corrupt(String),
    /// The journal belongs to another upload: the session name or task description
    /// differs, or a journaled file is no longer in the plan.
    #[error("belongs to another upload: {0}")]
    Mismatch(String),
}

/// Why an [`Error::Login`] occurred.
///
/// # Examples
///
/// ```
/// use imify::LoginFailure;
///
/// fn retry_without_browser(reason: LoginFailure) -> bool {
///     matches!(reason, LoginFailure::TimedOut | LoginFailure::PortsInUse)
/// }
///
/// assert!(retry_without_browser(LoginFailure::PortsInUse));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoginFailure {
    /// The stored login cannot be refreshed: the refresh token is missing, expired or
    /// revoked.
    Expired,
    /// The user or the identity provider denied the authorization request.
    Denied,
    /// The redirect carried a `state` that does not belong to this login.
    StateMismatch,
    /// The identity provider returned an ID token with a wrong issuer, audience or
    /// `nonce`, an expiry in the past, or a payload that cannot be decoded.
    InvalidIdToken,
    /// The pasted redirect URL is not a URL or lacks `code` and `state`.
    InvalidRedirectUrl,
    /// No redirect arrived within the timeout.
    TimedOut,
    /// None of the redirect ports can be bound.
    PortsInUse,
    /// The identity provider answered with an error or an incomplete response.
    Provider,
}

/// One entry of a request validation error.
///
/// # Examples
///
/// ```
/// use imify::{Error, FieldError};
///
/// fn field_paths(error: &Error) -> Vec<String> {
///     match error {
///         Error::Validation { fields, .. } => fields
///             .iter()
///             .map(|field: &FieldError| field.location.join("."))
///             .collect(),
///         _ => Vec::new(),
///     }
/// }
///
/// let error = Error::Validation {
///     message: "invalid request".into(),
///     fields: vec![FieldError {
///         location: vec!["body".into(), "name".into()],
///         message: "Field required".into(),
///         kind: "missing".into(),
///     }],
/// };
/// assert_eq!(field_paths(&error), ["body.name"]);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// Path to the invalid value, such as `["body", "episodes", "0", "length"]`.
    pub location: Vec<String>,
    /// What is wrong with the value.
    pub message: String,
    /// Machine-readable error type, such as `missing` or `greater_than`.
    pub kind: String,
}

/// Marks the source of an [`Error::Network`] whose connection was never established, so
/// the request did not reach the API.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub(crate) struct ConnectFailed(pub(crate) Box<dyn std::error::Error + Send + Sync>);

impl Error {
    /// Returns `true` for a network error that occurred before the request reached the API.
    pub(crate) fn is_connect_failure(&self) -> bool {
        matches!(self, Error::Network { source } if source.is::<ConnectFailed>())
    }
}

/// Maps a non-success response to an [`Error`].
pub(crate) fn from_response(status: u16, headers: &HeaderMap, body: &[u8]) -> Error {
    let json = serde_json::from_slice::<Value>(body).ok();
    let detail = json.as_ref().and_then(|value| value.get("detail"));
    let message = detail.map_or_else(|| reason_phrase(status), detail_message);

    match status {
        400 | 422 => Error::Validation {
            message,
            fields: detail.map(field_errors).unwrap_or_default(),
        },
        401 => Error::Auth {
            status: Some(status),
            message,
        },
        403 if json.is_some() => Error::Auth {
            status: Some(status),
            message,
        },
        403 => Error::RateLimited { retry_after: None },
        404 => Error::NotFound { message },
        409 => Error::Conflict { message },
        429 => Error::RateLimited {
            retry_after: retry_after(headers),
        },
        500..=599 => Error::Server { status, message },
        _ => Error::UnexpectedResponse { status, message },
    }
}

fn reason_phrase(status: u16) -> String {
    StatusCode::from_u16(status)
        .ok()
        .and_then(|code| code.canonical_reason())
        .unwrap_or("unknown status")
        .to_owned()
}

fn detail_message(detail: &Value) -> String {
    match detail {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn field_errors(detail: &Value) -> Vec<FieldError> {
    detail
        .as_array()
        .map(|entries| entries.iter().filter_map(field_error).collect())
        .unwrap_or_default()
}

fn field_error(entry: &Value) -> Option<FieldError> {
    let location = entry
        .get("loc")?
        .as_array()?
        .iter()
        .map(|segment| match segment {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .collect();
    Some(FieldError {
        location,
        message: entry.get("msg")?.as_str()?.to_owned(),
        kind: entry.get("type")?.as_str()?.to_owned(),
    })
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let seconds = headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests;
