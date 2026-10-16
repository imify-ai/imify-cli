use std::ffi::OsString;

use super::*;
use crate::Client;

const TOKEN: &str = "tok_7f3a9c-secret";

async fn token_of(source: &EnvToken) -> String {
    source.token().await.unwrap().secret().to_owned()
}

#[tokio::test]
async fn token_is_the_value_of_the_variable() {
    let source = EnvToken::from_value(Some(OsString::from(TOKEN)))
        .unwrap()
        .unwrap();

    assert_eq!(token_of(&source).await, TOKEN);
}

#[tokio::test]
async fn surrounding_whitespace_is_trimmed() {
    let source = EnvToken::from_value(Some(OsString::from(format!("  {TOKEN}\n"))))
        .unwrap()
        .unwrap();

    assert_eq!(token_of(&source).await, TOKEN);
}

#[test]
fn unset_variable_resolves_to_none() {
    assert!(EnvToken::from_value(None).unwrap().is_none());
}

#[test]
fn whitespace_only_value_resolves_to_none() {
    assert!(
        EnvToken::from_value(Some(OsString::from(" \t\n")))
            .unwrap()
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn non_unicode_value_is_a_config_error_that_does_not_contain_the_value() {
    use std::os::unix::ffi::OsStringExt;

    let value = OsString::from_vec(b"tok_secret\xff".to_vec());

    let error = EnvToken::from_value(Some(value)).unwrap_err();

    assert!(matches!(error, Error::Config { .. }));
    let message = error.to_string();
    assert!(message.contains("IMIFY_TOKEN"), "{message}");
    assert!(!message.contains("tok_secret"), "{message}");
}

#[test]
fn debug_output_does_not_contain_the_token() {
    let source = EnvToken::from_value(Some(OsString::from(TOKEN)))
        .unwrap()
        .unwrap();

    assert!(!format!("{source:?}").contains(TOKEN));
}

#[test]
fn client_debug_output_does_not_contain_the_token() {
    let source = EnvToken::from_value(Some(OsString::from(TOKEN)))
        .unwrap()
        .unwrap();
    let client = Client::builder("https://api.example.com")
        .token_source(source)
        .build()
        .unwrap();

    assert!(!format!("{client:?}").contains(TOKEN));
}
