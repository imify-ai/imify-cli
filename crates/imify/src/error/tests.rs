use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
use serde_json::{Value, json};

use super::*;

fn map(status: u16, body: &str) -> Error {
    from_response(status, &HeaderMap::new(), body.as_bytes())
}

/// An AIP-193 error body with `details`.
fn api_error(code: u16, status: &str, message: &str, details: &Value) -> String {
    json!({
        "error": {"code": code, "status": status, "message": message, "details": details},
        "traceId": "Root=1-6740-2a9c",
    })
    .to_string()
}

fn error_info(reason: &str) -> Value {
    json!({
        "@type": "type.googleapis.com/google.rpc.ErrorInfo",
        "reason": reason,
        "domain": "imify-cloud",
        "metadata": {},
    })
}

#[test]
fn status_401_maps_to_auth_with_the_error_message() {
    let error = map(
        401,
        &api_error(401, "UNAUTHENTICATED", "Token expired", &json!([])),
    );

    assert!(matches!(
        error,
        Error::Auth { status: Some(401), ref message } if message == "Token expired"
    ));
}

#[test]
fn status_403_with_json_maps_to_permission_denied_with_reason() {
    let body = api_error(
        403,
        "PERMISSION_DENIED",
        "The request was blocked",
        &json!([error_info("REQUEST_BLOCKED")]),
    );

    let error = map(403, &body);

    assert!(matches!(
        error,
        Error::PermissionDenied { ref message, reason: Some(ref reason) }
            if message == "The request was blocked" && reason == "REQUEST_BLOCKED"
    ));
}

#[test]
fn status_403_with_text_body_maps_to_permission_denied_for_the_network() {
    let error = map(403, "Forbidden");

    assert!(matches!(
        error,
        Error::PermissionDenied { ref message, reason: None }
            if message == "this network has no access to the API"
    ));
}

#[test]
fn status_403_with_empty_body_is_not_a_rate_limit() {
    let error = map(403, "");

    assert!(matches!(error, Error::PermissionDenied { .. }));
}

#[test]
fn status_404_maps_to_not_found_with_the_error_message() {
    let error = map(
        404,
        &api_error(
            404,
            "NOT_FOUND",
            "Recording 3f2c9a does not exist",
            &json!([error_info("RECORDING_NOT_FOUND")]),
        ),
    );

    assert!(
        matches!(error, Error::NotFound { ref message } if message == "Recording 3f2c9a does not exist")
    );
}

#[test]
fn status_409_maps_to_conflict_with_reason() {
    let error = map(
        409,
        &api_error(
            409,
            "ALREADY_EXISTS",
            "data/a.parquet was announced with another size",
            &json!([error_info("FILE_CONFLICT")]),
        ),
    );

    assert!(matches!(
        error,
        Error::Conflict { reason: Some(ref reason), .. } if reason == "FILE_CONFLICT"
    ));
}

#[test]
fn status_412_maps_to_failed_precondition_with_violations() {
    let details = json!([
        error_info("FILES_NOT_UPLOADED"),
        {
            "@type": "type.googleapis.com/google.rpc.PreconditionFailure",
            "violations": [{
                "type": "FILE_NOT_UPLOADED",
                "subject": "data/chunk-000/file-000.parquet",
                "description": "not uploaded",
            }],
        },
    ]);

    let error = map(
        412,
        &api_error(412, "FAILED_PRECONDITION", "1 file is missing", &details),
    );

    let Error::FailedPrecondition {
        reason, violations, ..
    } = error
    else {
        panic!("expected Error::FailedPrecondition, got {error:?}");
    };
    assert_eq!(
        (reason.as_deref(), violations),
        (
            Some("FILES_NOT_UPLOADED"),
            vec![Violation {
                kind: "FILE_NOT_UPLOADED".into(),
                subject: "data/chunk-000/file-000.parquet".into(),
                description: "not uploaded".into(),
            }]
        )
    );
}

#[test]
fn status_400_maps_bad_request_field_violations_to_field_errors() {
    let details = json!([{
        "@type": "type.googleapis.com/google.rpc.BadRequest",
        "fieldViolations": [
            {"field": "labels.Shift", "description": "must start with a lowercase letter"},
            {"field": "robot", "description": "too long"},
        ],
    }]);

    let Error::Validation { fields, .. } = map(
        400,
        &api_error(400, "INVALID_ARGUMENT", "Invalid recording", &details),
    ) else {
        panic!("expected Error::Validation");
    };

    assert_eq!(
        fields,
        vec![
            FieldError {
                location: vec!["labels".into(), "Shift".into()],
                message: "must start with a lowercase letter".into(),
                kind: String::new(),
            },
            FieldError {
                location: vec!["robot".into()],
                message: "too long".into(),
                kind: String::new(),
            },
        ]
    );
}

#[test]
fn status_400_keeps_the_reason_of_the_error_info() {
    let error = map(
        400,
        &api_error(
            400,
            "INVALID_ARGUMENT",
            "meta/info.json cannot be read",
            &json!([error_info("INVALID_DATASET")]),
        ),
    );

    assert!(matches!(
        error,
        Error::Validation { ref message, ref fields, reason: Some(ref reason) }
            if message == "meta/info.json cannot be read" && fields.is_empty() && reason == "INVALID_DATASET"
    ));
}

#[test]
fn details_of_unknown_types_are_skipped() {
    let details = json!([
        {"@type": "type.googleapis.com/google.rpc.Help", "links": []},
        {"reason": "NO_TYPE"},
        error_info("FILE_CONFLICT"),
    ]);

    let error = map(409, &api_error(409, "ALREADY_EXISTS", "conflict", &details));

    assert!(matches!(
        error,
        Error::Conflict { reason: Some(ref reason), .. } if reason == "FILE_CONFLICT"
    ));
}

#[test]
fn a_detail_text_of_another_service_is_the_message() {
    let error = map(404, r#"{"detail":"Not Found"}"#);

    assert!(matches!(error, Error::NotFound { ref message } if message == "Not Found"));
}

#[test]
fn missing_body_yields_reason_phrase_as_message() {
    let error = map(404, "");

    assert!(matches!(error, Error::NotFound { ref message } if message == "Not Found"));
}

#[test]
fn status_429_maps_to_rate_limited_with_retry_after_seconds() {
    let mut headers = HeaderMap::new();
    headers.insert(RETRY_AFTER, HeaderValue::from_static("7"));

    let error = from_response(429, &headers, b"");

    assert!(matches!(
        error,
        Error::RateLimited { retry_after: Some(delay) } if delay == Duration::from_secs(7)
    ));
}

#[test]
fn status_429_without_retry_after_has_no_delay() {
    let error = map(
        429,
        &api_error(
            429,
            "RESOURCE_EXHAUSTED",
            "Too many requests",
            &json!([error_info("RATE_LIMITED")]),
        ),
    );

    assert!(matches!(error, Error::RateLimited { retry_after: None }));
}

#[test]
fn status_503_maps_to_server_with_retry_after_seconds() {
    let mut headers = HeaderMap::new();
    headers.insert(RETRY_AFTER, HeaderValue::from_static("5"));
    let body = api_error(
        503,
        "UNAVAILABLE",
        "The catalog is unavailable",
        &json!([error_info("CATALOG_UNAVAILABLE")]),
    );

    let error = from_response(503, &headers, body.as_bytes());

    assert!(matches!(
        error,
        Error::Server { status: 503, ref message, retry_after: Some(delay) }
            if message == "The catalog is unavailable" && delay == Duration::from_secs(5)
    ));
}

#[test]
fn status_501_maps_to_server() {
    let error = map(
        501,
        &api_error(501, "UNIMPLEMENTED", "No upload URLs", &json!([])),
    );

    assert!(matches!(error, Error::Server { status: 501, .. }));
}

#[test]
fn unlisted_status_maps_to_unexpected_response() {
    assert!(matches!(
        map(405, ""),
        Error::UnexpectedResponse { status: 405, .. }
    ));
}

#[test]
fn io_error_message_names_the_path() {
    let error = Error::Io {
        path: "pick-place/meta/info.json".into(),
        source: std::io::Error::from(std::io::ErrorKind::NotFound),
    };

    assert!(
        error
            .to_string()
            .starts_with("cannot read pick-place/meta/info.json: ")
    );
}

#[test]
fn journal_error_message_names_the_journal_and_the_reason() {
    let error = Error::Journal {
        path: "uploads/3f2c.jsonl".into(),
        reason: JournalError::Mismatch("the task differs".into()),
    };

    assert_eq!(
        error.to_string(),
        "upload journal uploads/3f2c.jsonl: belongs to another upload: the task differs"
    );
}

#[test]
fn upload_error_message_names_the_file_and_the_storage_code() {
    let error = Error::Upload {
        path: "data/a.parquet".into(),
        reason: UploadError::Storage {
            status: 400,
            code: Some("EntityTooLarge".into()),
        },
    };

    assert_eq!(
        error.to_string(),
        "upload data/a.parquet: storage answered 400 EntityTooLarge"
    );
}
