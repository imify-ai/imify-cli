use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};

use super::*;

fn map(status: u16, body: &str) -> Error {
    from_response(status, &HeaderMap::new(), body.as_bytes())
}

#[test]
fn status_401_maps_to_auth() {
    let error = map(401, r#"{"detail":"Not authenticated"}"#);

    assert!(matches!(
        error,
        Error::Auth { status: Some(401), ref message } if message == "Not authenticated"
    ));
}

#[test]
fn status_403_with_json_detail_maps_to_auth() {
    let error = map(403, r#"{"detail":"Not enough permissions"}"#);

    assert!(matches!(
        error,
        Error::Auth { status: Some(403), ref message } if message == "Not enough permissions"
    ));
}

#[test]
fn status_403_with_empty_body_maps_to_rate_limited() {
    let error = map(403, "");

    assert!(matches!(error, Error::RateLimited { retry_after: None }));
}

#[test]
fn status_403_with_html_body_maps_to_rate_limited() {
    let error = map(403, "<html><body>Forbidden</body></html>");

    assert!(matches!(error, Error::RateLimited { retry_after: None }));
}

#[test]
fn status_404_maps_to_not_found_with_detail_as_message() {
    let error = map(404, r#"{"detail":"Dataset not found"}"#);

    assert!(matches!(error, Error::NotFound { ref message } if message == "Dataset not found"));
}

#[test]
fn status_409_maps_to_conflict() {
    let error = map(409, r#"{"detail":"Session already completed"}"#);

    assert!(
        matches!(error, Error::Conflict { ref message } if message == "Session already completed")
    );
}

#[test]
fn status_422_maps_validation_list_to_field_errors() {
    let body = r#"{"detail":[
        {"loc":["body","episodes",0,"length"],"msg":"Input should be greater than 0","type":"greater_than"},
        {"loc":["query","limit"],"msg":"Field required","type":"missing"}
    ]}"#;

    let Error::Validation { fields, .. } = map(422, body) else {
        panic!("expected Error::Validation");
    };

    assert_eq!(
        fields,
        vec![
            FieldError {
                location: vec![
                    "body".into(),
                    "episodes".into(),
                    "0".into(),
                    "length".into()
                ],
                message: "Input should be greater than 0".into(),
                kind: "greater_than".into(),
            },
            FieldError {
                location: vec!["query".into(), "limit".into()],
                message: "Field required".into(),
                kind: "missing".into(),
            },
        ]
    );
}

#[test]
fn status_400_with_string_detail_maps_to_validation_without_field_errors() {
    let error = map(400, r#"{"detail":"Unsupported dataset format"}"#);

    assert!(matches!(
        error,
        Error::Validation { ref message, ref fields }
            if message == "Unsupported dataset format" && fields.is_empty()
    ));
}

#[test]
fn non_string_detail_is_rendered_as_compact_json() {
    let error = map(409, r#"{"detail": {"code": "locked", "retry": true}}"#);

    assert!(
        matches!(error, Error::Conflict { ref message } if message == r#"{"code":"locked","retry":true}"#)
    );
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
    let error = map(429, "");

    assert!(matches!(error, Error::RateLimited { retry_after: None }));
}

#[test]
fn status_5xx_maps_to_server_with_status_and_message() {
    let error = map(503, r#"{"detail":"Database unavailable"}"#);

    assert!(matches!(
        error,
        Error::Server { status: 503, ref message } if message == "Database unavailable"
    ));
}

#[test]
fn unlisted_status_maps_to_unexpected_response() {
    let error = map(405, r#"{"detail":"Method Not Allowed"}"#);

    assert!(matches!(
        error,
        Error::UnexpectedResponse { status: 405, .. }
    ));
}
