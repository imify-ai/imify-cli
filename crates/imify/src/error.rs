//! The error type returned by every fallible SDK operation.

use std::path::PathBuf;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde_json::Value;

/// An error returned by the SDK.
///
/// Messages come from the `message` of the API's error body
/// (`{"error": {"code", "status", "message", "details"}}`, [AIP-193]) when the response has
/// one, and from the HTTP reason phrase otherwise. `reason` is the `reason` of the body's
/// `ErrorInfo` detail, such as `FILE_CONFLICT`: branch on it, not on the message. No variant
/// contains the access token or a presigned storage URL.
///
/// [AIP-193]: https://google.aip.dev/193
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
    /// The API rejected the credentials (401), also after one refresh of the token, or the
    /// token source failed to produce a token (`status` is `None`).
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

    /// The token is valid, but the request is not allowed (403): the user has no role for
    /// it, the firewall blocked it (`reason` `REQUEST_BLOCKED`), or the network the request
    /// comes from has no access to the API (a 403 without a JSON body).
    #[error("permission denied: {message}")]
    PermissionDenied {
        /// Reason given by the API.
        message: String,
        /// The `reason` of the `ErrorInfo` detail, such as `REQUEST_BLOCKED`.
        reason: Option<String>,
    },

    /// The API rejected the request content (400 or 422), or a value of
    /// [`NewRecording`](crate::ingest::NewRecording) is outside the limits of the API.
    #[error("validation failed: {message}")]
    Validation {
        /// Reason given by the API.
        message: String,
        /// One entry per invalid field; empty when the API names no field.
        fields: Vec<FieldError>,
        /// The `reason` of the `ErrorInfo` detail, such as `INVALID_DATASET`.
        reason: Option<String>,
    },

    /// The request conflicts with the current state of the resource (409).
    #[error("conflict: {message}")]
    Conflict {
        /// Reason given by the API.
        message: String,
        /// The `reason` of the `ErrorInfo` detail, such as `FILE_CONFLICT`.
        reason: Option<String>,
    },

    /// The state of the resource does not allow the request (412), such as completing a
    /// recording whose files are not all uploaded.
    #[error("failed precondition: {message}")]
    FailedPrecondition {
        /// Reason given by the API.
        message: String,
        /// The `reason` of the `ErrorInfo` detail, such as `FILES_NOT_UPLOADED`.
        reason: Option<String>,
        /// The violations of the `PreconditionFailure` detail, such as one per file that
        /// is not uploaded; empty when the API names none.
        violations: Vec<Violation>,
    },

    /// The API or the network edge throttled the request (429).
    #[error("rate limited")]
    RateLimited {
        /// Delay requested by the `Retry-After` header, when present in seconds.
        retry_after: Option<Duration>,
    },

    /// The API or storage failed to process the request (5xx).
    #[error("server error {status}: {message}")]
    Server {
        /// HTTP status of the response.
        status: u16,
        /// Reason given by the API.
        message: String,
        /// Delay requested by the `Retry-After` header, when present in seconds.
        retry_after: Option<Duration>,
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

    /// Updating the program failed: the install receipt cannot be read or describes an
    /// install that cannot be updated, the release has no shell installer, or the installer
    /// failed.
    ///
    /// ```
    /// use imify::Error;
    ///
    /// fn installer_log(error: &Error) -> Option<&str> {
    ///     match error {
    ///         Error::SelfUpdate { installer_output, .. } => installer_output.as_deref(),
    ///         _ => None,
    ///     }
    /// }
    ///
    /// let error = Error::SelfUpdate {
    ///     message: "the installer exited with exit status: 1".into(),
    ///     installer_output: Some("ERROR: no prebuilt binaries for this platform".into()),
    /// };
    /// assert_eq!(installer_log(&error), Some("ERROR: no prebuilt binaries for this platform"));
    /// ```
    #[error("self update failed: {message}")]
    SelfUpdate {
        /// What failed.
        message: String,
        /// stdout followed by stderr of the installer; `None` when it did not run.
        installer_output: Option<String>,
    },

    /// A download cannot use a manifest entry, or cannot write or verify a file.
    ///
    /// ```
    /// use imify::{DownloadError, Error};
    ///
    /// fn must_be_removed_by_hand(error: &Error) -> bool {
    ///     matches!(error, Error::Download { reason: DownloadError::Foreign(_), .. })
    /// }
    ///
    /// let error = Error::Download {
    ///     path: "policies/pick-place/notes.txt".into(),
    ///     reason: DownloadError::Foreign("a file not in the manifest".into()),
    /// };
    /// assert!(must_be_removed_by_hand(&error));
    /// ```
    #[error("download {}: {reason}", path.display())]
    Download {
        /// The file below the target directory, the target directory itself, or the
        /// manifest's relative path when the manifest entry is refused.
        path: PathBuf,
        /// What failed.
        #[source]
        reason: DownloadError,
    },

    /// Storage refused the upload of a file to its presigned URL.
    ///
    /// ```
    /// use imify::{Error, UploadError};
    ///
    /// fn local_file_changed(error: &Error) -> bool {
    ///     matches!(error, Error::Upload { reason: UploadError::ChecksumMismatch, .. })
    /// }
    ///
    /// let error = Error::Upload {
    ///     path: "data/chunk-000/file-000.parquet".into(),
    ///     reason: UploadError::ChecksumMismatch,
    /// };
    /// assert!(local_file_changed(&error));
    /// ```
    #[error("upload {path}: {reason}")]
    Upload {
        /// Path of the file relative to the dataset directory.
        path: String,
        /// What failed.
        #[source]
        reason: UploadError,
    },
}

/// Why storage refused the upload of a file, reported in [`Error::Upload`]. No variant
/// contains the presigned URL.
///
/// # Examples
///
/// ```
/// use imify::UploadError;
///
/// let error = UploadError::Storage { status: 400, code: Some("EntityTooLarge".into()) };
/// assert_eq!(error.to_string(), "storage answered 400 EntityTooLarge");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum UploadError {
    /// Storage answered 403 to the `PUT`, also with a fresh presigned URL: the URL expired
    /// or a signed header is missing.
    #[error("storage rejected the presigned URL, also a fresh one")]
    UrlExpired,
    /// Storage answered `BadDigest`, also after the file was read again: the bytes do not
    /// match the SHA-256 announced for the file, so the local file changed since then.
    #[error("the file changed since its SHA-256 was announced")]
    ChecksumMismatch,
    /// Storage answered with another client error, such as `400` for a chunked body.
    #[error("storage answered {status}{}", code.as_deref().map(|code| format!(" {code}")).unwrap_or_default())]
    Storage {
        /// HTTP status of the response.
        status: u16,
        /// The `Code` of the S3 error body, when it has one.
        code: Option<String>,
    },
}

/// Why a download failed, reported in [`Error::Download`].
///
/// # Examples
///
/// ```
/// use imify::DownloadError;
///
/// let error = DownloadError::SizeMismatch { expected: 1024, received: 2048 };
/// assert_eq!(error.to_string(), "storage sent 2048 bytes, the manifest lists 1024");
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DownloadError {
    /// A relative path of the manifest is empty or absolute, has an empty, `.` or `..`
    /// component, contains `\` or NUL, or collides with another entry: it is equal to
    /// another entry, equal after lowercasing, a directory of another entry, or the
    /// `.part` file of another entry.
    #[error("unsafe path in the manifest: {0}")]
    UnsafePath(String),
    /// A presigned URL is not `https`, and not `http` on `localhost`, `127.0.0.1` or
    /// `::1`. The message names scheme and host only.
    #[error("unsafe storage URL: {0}")]
    UnsafeUrl(String),
    /// The target directory holds an entry the download does not own: a file not in the
    /// manifest, a manifest file with another size, a symbolic link, or an entry other
    /// than a regular file or directory. The entry is left unchanged.
    #[error("the target holds {0}")]
    Foreign(String),
    /// Storage announced or sent another number of bytes than the manifest lists.
    #[error("storage sent {received} bytes, the manifest lists {expected}")]
    SizeMismatch {
        /// Size in the manifest.
        expected: u64,
        /// Size announced in `Content-Length`, plus the bytes continued from a `.part`
        /// file, or the bytes received when the body exceeds the manifest size.
        received: u64,
    },
    /// Storage answered 403 again after the manifest was fetched again.
    #[error("storage rejected the presigned URL of the fetched-again manifest")]
    UrlExpired,
    /// The fetched-again manifest lists other files or sizes.
    #[error("the manifest changed during the download: {0}")]
    ManifestChanged(String),
    /// Storage answered with a status other than success, 403, 429 or 5xx, such as 404
    /// or a redirect that is not followed.
    #[error("storage answered {status}")]
    Storage {
        /// HTTP status of the response.
        status: u16,
    },
    /// The target directory or a file below it cannot be created, written, synced or
    /// renamed.
    #[error("{0}")]
    Io(#[source] std::io::Error),
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
    /// An older SDK version wrote the journal, for an API without recordings. It cannot be
    /// continued; [`ResumePolicy::Restart`](crate::ingest::ResumePolicy::Restart) starts a
    /// new recording.
    #[error("written by an older version (journal version {0})")]
    Outdated(u32),
    /// The journal belongs to another upload: a setting of the
    /// [`NewRecording`](crate::ingest::NewRecording) differs, or a journaled file is no
    /// longer in the plan.
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
    /// The pasted redirect URL carried a `state` that does not belong to this login.
    StateMismatch,
    /// The identity provider returned an ID token with a wrong issuer, audience, `azp`,
    /// `nonce` or subject, an expiry more than 60 s in the past or more than one day
    /// ahead, or a payload that cannot be decoded.
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
///         location: vec!["labels".into(), "site".into()],
///         message: "must start with a lowercase letter".into(),
///         kind: String::new(),
///     }],
///     reason: None,
/// };
/// assert_eq!(field_paths(&error), ["labels.site"]);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// Path to the invalid value: the camelCase `field` of the API split at `.`, such as
    /// `["updateMask"]`.
    pub location: Vec<String>,
    /// What is wrong with the value.
    pub message: String,
    /// Machine-readable error type when the API sends one; empty otherwise.
    pub kind: String,
}

/// One violation of a [`Error::FailedPrecondition`]: the `PreconditionFailure` detail of
/// the API.
///
/// # Examples
///
/// ```
/// use imify::{Error, Violation};
///
/// fn missing_files(error: &Error) -> Vec<&str> {
///     match error {
///         Error::FailedPrecondition { violations, .. } => violations
///             .iter()
///             .map(|violation: &Violation| violation.subject.as_str())
///             .collect(),
///         _ => Vec::new(),
///     }
/// }
///
/// let error = Error::FailedPrecondition {
///     message: "1 file is not uploaded".into(),
///     reason: Some("FILES_NOT_UPLOADED".into()),
///     violations: vec![Violation {
///         kind: "FILE_NOT_UPLOADED".into(),
///         subject: "meta/info.json".into(),
///         description: "not uploaded".into(),
///     }],
/// };
/// assert_eq!(missing_files(&error), ["meta/info.json"]);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The `type` of the violation, such as `FILE_NOT_UPLOADED`.
    pub kind: String,
    /// What the violation is about, such as the path of a file.
    pub subject: String,
    /// What is wrong.
    pub description: String,
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

/// The `@type` of the details this SDK reads.
const ERROR_INFO: &str = "type.googleapis.com/google.rpc.ErrorInfo";
const BAD_REQUEST: &str = "type.googleapis.com/google.rpc.BadRequest";
const PRECONDITION_FAILURE: &str = "type.googleapis.com/google.rpc.PreconditionFailure";

/// The message of a 403 without a JSON body: a network edge that admits only some
/// networks answers it.
const NETWORK_DENIED: &str = "this network has no access to the API";

/// Maps a non-success response to an [`Error`]. Reads the error body of AIP-193
/// (`{"error": {"code", "status", "message", "details"}}`); another body only gives the
/// `detail` text of other services, or the reason phrase.
pub(crate) fn from_response(status: u16, headers: &HeaderMap, body: &[u8]) -> Error {
    let json = serde_json::from_slice::<Value>(body).ok();
    let error = json
        .as_ref()
        .and_then(|value| value.get("error"))
        .filter(|error| error.is_object());
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| detail_text(json.as_ref()?))
        .unwrap_or_else(|| reason_phrase(status));
    let details: &[Value] = error
        .and_then(|error| error.get("details"))
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    let reason = error_reason(details);

    match status {
        400 | 422 => Error::Validation {
            message,
            fields: field_errors(details),
            reason,
        },
        401 => Error::Auth {
            status: Some(status),
            message,
        },
        403 if json.is_some() => Error::PermissionDenied { message, reason },
        403 => Error::PermissionDenied {
            message: NETWORK_DENIED.to_owned(),
            reason: None,
        },
        404 => Error::NotFound { message },
        409 => Error::Conflict { message, reason },
        412 => Error::FailedPrecondition {
            message,
            reason,
            violations: violations(details),
        },
        429 => Error::RateLimited {
            retry_after: retry_after(headers),
        },
        500..=599 => Error::Server {
            status,
            message,
            retry_after: retry_after(headers),
        },
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

/// The `detail` text of a body that is not an AIP-193 error, such as one of an identity
/// provider.
fn detail_text(json: &Value) -> Option<String> {
    match json.get("detail")? {
        Value::String(text) => Some(text.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// The details of `@type` `kind`. Details of other types are skipped.
fn details_of<'a>(details: &'a [Value], kind: &'a str) -> impl Iterator<Item = &'a Value> {
    details
        .iter()
        .filter(move |detail| detail.get("@type").and_then(Value::as_str) == Some(kind))
}

fn error_reason(details: &[Value]) -> Option<String> {
    details_of(details, ERROR_INFO)
        .find_map(|detail| detail.get("reason")?.as_str())
        .map(str::to_owned)
}

/// The `fieldViolations` of every `BadRequest` detail.
fn field_errors(details: &[Value]) -> Vec<FieldError> {
    details_of(details, BAD_REQUEST)
        .filter_map(|detail| detail.get("fieldViolations")?.as_array())
        .flatten()
        .filter_map(field_error)
        .collect()
}

fn field_error(entry: &Value) -> Option<FieldError> {
    let field = entry.get("field")?.as_str()?;
    Some(FieldError {
        location: field.split('.').map(str::to_owned).collect(),
        message: text_of(entry, "description"),
        kind: text_of(entry, "reason"),
    })
}

/// The `violations` of every `PreconditionFailure` detail.
fn violations(details: &[Value]) -> Vec<Violation> {
    details_of(details, PRECONDITION_FAILURE)
        .filter_map(|detail| detail.get("violations")?.as_array())
        .flatten()
        .map(|entry| Violation {
            kind: text_of(entry, "type"),
            subject: text_of(entry, "subject"),
            description: text_of(entry, "description"),
        })
        .collect()
}

/// The string `key` of `entry`, empty when it is missing or not a string.
fn text_of(entry: &Value, key: &str) -> String {
    entry
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

pub(crate) fn retry_after(headers: &HeaderMap) -> Option<Duration> {
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
