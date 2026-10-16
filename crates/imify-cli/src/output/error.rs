//! Stable error codes, their exit codes and the error envelope.

use serde::Serialize;

/// A stable, documented error code. The `snake_case` name is part of the JSON contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "commands produce these codes; no command exists yet"
    )
)]
pub enum ErrorCode {
    /// An unexpected failure inside the CLI.
    Internal,
    /// The command line is invalid or input is missing.
    InvalidArgument,
    /// The command needs a login or a token.
    AuthRequired,
    /// The requested resource does not exist.
    NotFound,
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
    pub const ALL: [Self; 10] = [
        Self::Internal,
        Self::InvalidArgument,
        Self::AuthRequired,
        Self::NotFound,
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
            Self::AuthRequired => 3,
            Self::NotFound => 4,
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
                (ErrorCode::NotFound, 4),
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
                "not_found",
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
}
