use serde_json::json;

use super::*;
use crate::auth::testing::{CLIENT_ID, claims, jwt};

const ISSUER: &str = "https://login.example.com/pool-1";
const NONCE: &str = "n-0S6_WzA2Mj";

fn expected(nonce: Option<&str>) -> Expected<'_> {
    Expected {
        issuer: ISSUER,
        client_id: CLIENT_ID,
        nonce,
        now: SystemTime::now(),
    }
}

fn reason(result: Result<u64, Error>) -> LoginFailure {
    match result {
        Err(Error::Login { reason, .. }) => reason,
        other => panic!("expected a login error, got {other:?}"),
    }
}

#[test]
fn a_valid_token_yields_its_expiry() {
    let claims = claims(ISSUER, Some(NONCE), 3600);

    let expiry = check(&jwt(&claims), &expected(Some(NONCE))).unwrap();

    assert_eq!(json!(expiry), claims["exp"]);
}

#[test]
fn an_audience_list_that_contains_the_client_id_is_accepted() {
    let mut claims = claims(ISSUER, None, 3600);
    claims["aud"] = json!(["other-client", CLIENT_ID]);

    assert!(check(&jwt(&claims), &expected(None)).is_ok());
}

#[test]
fn a_wrong_issuer_is_rejected() {
    let claims = claims("https://login.example.com/pool-2", None, 3600);

    let reason = reason(check(&jwt(&claims), &expected(None)));

    assert_eq!(reason, LoginFailure::InvalidIdToken);
}

#[test]
fn a_wrong_audience_is_rejected() {
    let mut claims = claims(ISSUER, None, 3600);
    claims["aud"] = json!("other-client");

    let reason = reason(check(&jwt(&claims), &expected(None)));

    assert_eq!(reason, LoginFailure::InvalidIdToken);
}

#[test]
fn an_expiry_in_the_past_is_rejected() {
    let claims = claims(ISSUER, None, -10);

    let reason = reason(check(&jwt(&claims), &expected(None)));

    assert_eq!(reason, LoginFailure::InvalidIdToken);
}

#[test]
fn a_wrong_nonce_is_rejected() {
    let claims = claims(ISSUER, Some("other-nonce"), 3600);

    let reason = reason(check(&jwt(&claims), &expected(Some(NONCE))));

    assert_eq!(reason, LoginFailure::InvalidIdToken);
}

#[test]
fn a_missing_nonce_is_rejected_when_one_is_expected() {
    let claims = claims(ISSUER, None, 3600);

    let reason = reason(check(&jwt(&claims), &expected(Some(NONCE))));

    assert_eq!(reason, LoginFailure::InvalidIdToken);
}

#[test]
fn a_value_that_is_not_a_jwt_is_rejected_without_echoing_it() {
    let error = check("not-a-jwt-secret", &expected(None)).unwrap_err();

    assert!(matches!(
        error,
        Error::Login {
            reason: LoginFailure::InvalidIdToken,
            ..
        }
    ));
    assert!(!error.to_string().contains("not-a-jwt-secret"));
}

fn expiry_of(token: &str) -> Option<SystemTime> {
    crate::auth::Token::new(token).expires_at()
}

fn unix_time(seconds: u64) -> SystemTime {
    UNIX_EPOCH + std::time::Duration::from_secs(seconds)
}

fn encoded(segment: &str) -> String {
    URL_SAFE_NO_PAD.encode(segment)
}

#[test]
fn expires_at_returns_the_exp_claim_of_a_jwt() {
    let token = jwt(&json!({"sub": "user-1234", "exp": 1_792_152_000}));

    assert_eq!(expiry_of(&token), Some(unix_time(1_792_152_000)));
}

#[test]
fn expires_at_ignores_the_signature_segment() {
    let header = encoded(r#"{"alg":"RS256"}"#);
    let payload = encoded(r#"{"exp":1792152000}"#);

    let token = format!("{header}.{payload}.%%not-base64%%");

    assert_eq!(expiry_of(&token), Some(unix_time(1_792_152_000)));
}

#[test]
fn expires_at_is_none_for_an_opaque_token() {
    assert_eq!(expiry_of("tok_7f3a9c-secret"), None);
}

#[test]
fn expires_at_is_none_for_a_payload_that_is_not_base64url() {
    let header = encoded(r#"{"alg":"RS256"}"#);

    assert_eq!(expiry_of(&format!("{header}.%%%.c2ln")), None);
}

#[test]
fn expires_at_is_none_for_a_payload_that_is_not_json() {
    let header = encoded(r#"{"alg":"RS256"}"#);

    assert_eq!(
        expiry_of(&format!("{header}.{}.c2ln", encoded("not json"))),
        None
    );
}

#[test]
fn expires_at_is_none_without_an_exp_claim() {
    assert_eq!(expiry_of(&jwt(&json!({"sub": "user-1234"}))), None);
}

#[test]
fn expires_at_is_none_for_a_non_numeric_exp_claim() {
    assert_eq!(expiry_of(&jwt(&json!({"exp": "tomorrow"}))), None);
}
