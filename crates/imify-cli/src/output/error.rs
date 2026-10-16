//! Stable error codes, their exit codes and the error envelope.

use serde::Serialize;

/// A stable, documented error code. The `snake_case` name is part of the JSON contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// An unexpected failure inside the CLI.
    Internal,
    /// The command line is invalid or input is missing.
    InvalidArgument,
    /// The command needs a login or a token.
    AuthRequired,
    /// The server rejected the access token.
    AuthRejected,
    /// The access token is valid but lacks the permission for the request.
    PermissionDenied,
    /// The requested resource does not exist.
    NotFound,
    /// The local dataset failed validation.
    DatasetInvalid,
    /// The request conflicts with the current state of the resource.
    Conflict,
    /// The server could not be reached.
    Network,
    /// The server failed to handle the request.
    Server,
    /// The server rejected the request because of its rate limit.
    RateLimited,
    /// The user declined a confirmation prompt.
    Cancelled,
    /// The user stopped the command with an interrupt signal.
    Interrupted,
}

impl ErrorCode {
    /// Every error code, in documentation order.
    #[cfg(test)]
    pub const ALL: [Self; 13] = [
        Self::Internal,
        Self::InvalidArgument,
        Self::AuthRequired,
        Self::AuthRejected,
        Self::PermissionDenied,
        Self::NotFound,
        Self::DatasetInvalid,
        Self::Conflict,
        Self::Network,
        Self::Server,
        Self::RateLimited,
        Self::Cancelled,
        Self::Interrupted,
    ];

    /// The process exit code for this error.
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Internal => 1,
            Self::InvalidArgument => 2,
            Self::AuthRequired | Self::AuthRejected | Self::PermissionDenied => 3,
            Self::NotFound => 4,
            Self::DatasetInvalid => 5,
            Self::Conflict => 6,
            Self::Network | Self::Server | Self::RateLimited => 7,
            Self::Cancelled => 8,
            Self::Interrupted => 130,
        }
    }
}

/// An error as the user sees it: a code, what happened, and what to do next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CliError {
    pub code: ErrorCode,
    pub message: String,
    pub hint: Option<String>,
}

impl CliError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
        }
    }

    /// The error for a command stopped by an interrupt signal: `interrupted`, exit code 130.
    pub fn interrupted() -> Self {
        Self::new(ErrorCode::Interrupted, "Interrupted.")
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// The JSON error envelope: `{"error":{"code":...,"message":...,"hint":...}}`.
    pub fn envelope(&self) -> Envelope<'_> {
        Envelope { error: self }
    }
}

/// The JSON form of a [`CliError`], written to stderr.
#[derive(Debug, Serialize)]
pub struct Envelope<'a> {
    error: &'a CliError,
}

impl From<clap::Error> for CliError {
    fn from(error: clap::Error) -> Self {
        let rendered = error.render().to_string();
        let first_line = rendered.lines().next().unwrap_or_default();
        let message = first_line.strip_prefix("error: ").unwrap_or(first_line);
        Self::new(ErrorCode::InvalidArgument, message).with_hint("Run `imify --help`.")
    }
}

/// The hint for an `auth_required` error.
pub const AUTH_REQUIRED_HINT: &str = "Set `IMIFY_TOKEN` to an access token.";

/// The hint for an `auth_rejected` error.
const AUTH_REJECTED_HINT: &str = "Set `IMIFY_TOKEN` to a valid, unexpired token.";

impl From<imify::Error> for CliError {
    fn from(error: imify::Error) -> Self {
        let message = error.to_string();
        match error {
            imify::Error::Auth { status: None, .. } => {
                Self::new(ErrorCode::AuthRequired, message).with_hint(AUTH_REQUIRED_HINT)
            }
            imify::Error::Auth {
                status: Some(403), ..
            } => Self::new(ErrorCode::PermissionDenied, message),
            imify::Error::Auth {
                status: Some(_), ..
            } => Self::new(ErrorCode::AuthRejected, message).with_hint(AUTH_REJECTED_HINT),
            imify::Error::NotFound { .. } => Self::new(ErrorCode::NotFound, message),
            imify::Error::Conflict { .. } => Self::new(ErrorCode::Conflict, message),
            imify::Error::RateLimited { retry_after } => {
                let error = Self::new(ErrorCode::RateLimited, message);
                match retry_after {
                    Some(delay) => error.with_hint(format!("Retry in {} s.", delay.as_secs())),
                    None => error,
                }
            }
            imify::Error::Server { .. } => Self::new(ErrorCode::Server, message),
            imify::Error::Network { .. } => Self::new(ErrorCode::Network, message),
            imify::Error::Config { .. } => Self::new(ErrorCode::InvalidArgument, message),
            _ => Self::new(ErrorCode::Internal, message),
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;

    #[test]
    fn each_error_code_maps_to_its_documented_exit_code() {
        let exit_codes: Vec<(ErrorCode, u8)> = ErrorCode::ALL
            .iter()
            .map(|code| (*code, code.exit_code()))
            .collect();

        assert_eq!(
            exit_codes,
            vec![
                (ErrorCode::Internal, 1),
                (ErrorCode::InvalidArgument, 2),
                (ErrorCode::AuthRequired, 3),
                (ErrorCode::AuthRejected, 3),
                (ErrorCode::PermissionDenied, 3),
                (ErrorCode::NotFound, 4),
                (ErrorCode::DatasetInvalid, 5),
                (ErrorCode::Conflict, 6),
                (ErrorCode::Network, 7),
                (ErrorCode::Server, 7),
                (ErrorCode::RateLimited, 7),
                (ErrorCode::Cancelled, 8),
                (ErrorCode::Interrupted, 130),
            ]
        );
    }

    #[test]
    fn each_error_code_serializes_to_its_snake_case_name() {
        let names: Vec<serde_json::Value> = ErrorCode::ALL
            .iter()
            .map(|code| serde_json::to_value(code).unwrap())
            .collect();

        assert_eq!(
            names,
            vec![
                "internal",
                "invalid_argument",
                "auth_required",
                "auth_rejected",
                "permission_denied",
                "not_found",
                "dataset_invalid",
                "conflict",
                "network",
                "server",
                "rate_limited",
                "cancelled",
                "interrupted",
            ]
        );
    }

    #[test]
    fn envelope_contains_a_null_hint_when_the_error_has_no_hint() {
        let error = CliError::new(ErrorCode::NotFound, "Dataset 42 does not exist.");

        assert_eq!(
            serde_json::to_string(&error.envelope()).unwrap(),
            r#"{"error":{"code":"not_found","message":"Dataset 42 does not exist.","hint":null}}"#
        );
    }

    #[test]
    fn envelope_contains_the_hint_when_the_error_has_one() {
        let error = CliError::new(ErrorCode::AuthRequired, "Not logged in.")
            .with_hint("Run `imify login`.");

        assert_eq!(
            serde_json::to_value(error.envelope()).unwrap()["error"]["hint"],
            serde_json::json!("Run `imify login`.")
        );
    }

    #[test]
    fn interrupted_error_has_the_interrupted_code_exit_code_130_and_no_hint() {
        let error = CliError::interrupted();

        assert_eq!(
            (error.code, error.code.exit_code(), error.hint),
            (ErrorCode::Interrupted, 130, None)
        );
    }

    #[test]
    fn clap_error_becomes_an_invalid_argument_with_its_first_line_as_message() {
        #[derive(Debug, Parser)]
        #[command(name = "imify")]
        struct Probe {}

        let clap_error = Probe::command()
            .try_get_matches_from(["imify", "--nope"])
            .unwrap_err();

        let error = CliError::from(clap_error);

        assert_eq!(
            error,
            CliError::new(
                ErrorCode::InvalidArgument,
                "unexpected argument '--nope' found"
            )
            .with_hint("Run `imify --help`.")
        );
    }

    #[test]
    fn every_error_code_is_documented_with_its_exit_code() {
        let doc = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/reference/json-output.md"
        ))
        .unwrap();

        for code in ErrorCode::ALL {
            let name = serde_json::to_value(code).unwrap();
            let row = format!("| `{}` | {} |", name.as_str().unwrap(), code.exit_code());
            assert!(doc.contains(&row), "json-output.md lacks the row {row}");
        }
    }

    #[test]
    fn error_schema_lists_exactly_the_error_codes() {
        let schema: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/reference/schemas/error.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let codes: Vec<serde_json::Value> = ErrorCode::ALL
            .iter()
            .map(|code| serde_json::to_value(code).unwrap())
            .collect();

        assert_eq!(
            schema["properties"]["error"]["properties"]["code"]["enum"],
            serde_json::Value::Array(codes)
        );
    }

    fn code_of(error: imify::Error) -> ErrorCode {
        CliError::from(error).code
    }

    #[test]
    fn auth_error_without_status_maps_to_auth_required() {
        let error = imify::Error::Auth {
            status: None,
            message: "not signed in".into(),
        };

        assert_eq!(code_of(error), ErrorCode::AuthRequired);
    }

    #[test]
    fn auth_error_with_status_401_maps_to_auth_rejected() {
        let error = imify::Error::Auth {
            status: Some(401),
            message: "Invalid token".into(),
        };

        assert_eq!(code_of(error), ErrorCode::AuthRejected);
    }

    #[test]
    fn auth_error_with_status_403_maps_to_permission_denied() {
        let error = imify::Error::Auth {
            status: Some(403),
            message: "Not allowed".into(),
        };

        assert_eq!(code_of(error), ErrorCode::PermissionDenied);
    }

    #[test]
    fn auth_rejected_hints_at_replacing_the_token() {
        let error = CliError::from(imify::Error::Auth {
            status: Some(401),
            message: "Invalid token".into(),
        });

        assert_eq!(
            error.hint.as_deref(),
            Some("Set `IMIFY_TOKEN` to a valid, unexpired token.")
        );
    }

    #[test]
    fn permission_denied_has_no_hint() {
        let error = CliError::from(imify::Error::Auth {
            status: Some(403),
            message: "Not allowed".into(),
        });

        assert_eq!(error.hint, None);
    }

    #[test]
    fn resource_and_server_errors_map_to_their_codes() {
        let codes = [
            code_of(imify::Error::NotFound {
                message: "Dataset not found".into(),
            }),
            code_of(imify::Error::Conflict {
                message: "Dataset exists".into(),
            }),
            code_of(imify::Error::RateLimited { retry_after: None }),
            code_of(imify::Error::Server {
                status: 503,
                message: "Service Unavailable".into(),
            }),
            code_of(imify::Error::Network {
                source: Box::new(std::io::Error::other("connection refused")),
            }),
        ];

        assert_eq!(
            codes,
            [
                ErrorCode::NotFound,
                ErrorCode::Conflict,
                ErrorCode::RateLimited,
                ErrorCode::Server,
                ErrorCode::Network,
            ]
        );
    }

    #[test]
    fn config_error_maps_to_invalid_argument() {
        let error = imify::Error::Config {
            message: "IMIFY_TOKEN is not valid Unicode".into(),
        };

        assert_eq!(code_of(error), ErrorCode::InvalidArgument);
    }

    #[test]
    fn validation_and_unexpected_response_map_to_internal() {
        let codes = [
            code_of(imify::Error::Validation {
                message: "invalid request".into(),
                fields: Vec::new(),
            }),
            code_of(imify::Error::UnexpectedResponse {
                status: 418,
                message: "I'm a teapot".into(),
            }),
        ];

        assert_eq!(codes, [ErrorCode::Internal, ErrorCode::Internal]);
    }

    #[test]
    fn rate_limited_with_retry_after_hints_at_the_delay() {
        let error = CliError::from(imify::Error::RateLimited {
            retry_after: Some(std::time::Duration::from_secs(30)),
        });

        assert_eq!(error.hint.as_deref(), Some("Retry in 30 s."));
    }
}
