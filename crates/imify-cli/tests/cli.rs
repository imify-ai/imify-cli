use std::process::Output;

use assert_cmd::Command;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "tok_7f3a9c-secret";

fn imify(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_imify"));
    command
        .env_remove("IMIFY_OUTPUT")
        .env_remove("NO_COLOR")
        .env_remove("IMIFY_TOKEN")
        .env_remove("IMIFY_API_URL");
    for (key, value) in env {
        command.env(key, value);
    }
    command.args(args).assert().get_output().clone()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn version_flag_prints_the_package_version() {
    let output = imify(&["--version"], &[]);

    assert!(output.status.success());
    assert_eq!(
        text(&output.stdout),
        format!("imify {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_argument_exits_with_usage_error() {
    let output = imify(&["--no-such-flag"], &[]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn json_flag_renders_a_usage_error_as_an_envelope_on_stderr() {
    let output = imify(&["--json", "--no-such-flag"], &[]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(text(&output.stdout), "");
    insta::assert_snapshot!("usage_error_json", text(&output.stderr));
}

#[test]
fn imify_output_json_renders_the_same_envelope_as_the_json_flag() {
    let from_flag = imify(&["--json", "--no-such-flag"], &[]);
    let from_env = imify(&["--no-such-flag"], &[("IMIFY_OUTPUT", "json")]);

    assert_eq!(from_env.status.code(), Some(2));
    assert_eq!(text(&from_env.stderr), text(&from_flag.stderr));
}

#[test]
fn human_mode_renders_a_usage_error_as_text_on_stderr() {
    let output = imify(&["--no-such-flag"], &[]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(text(&output.stdout), "");
    insta::assert_snapshot!("usage_error_human", text(&output.stderr));
}

#[test]
fn json_flag_after_the_separator_does_not_select_json_output() {
    let output = imify(&["--no-such-flag", "--", "--json"], &[]);

    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).starts_with("error: "));
}

#[test]
fn unsupported_imify_output_value_is_a_usage_error() {
    let output = imify(&["--json"], &[("IMIFY_OUTPUT", "xml")]);

    assert_eq!(output.status.code(), Some(2));
    insta::assert_snapshot!("invalid_imify_output_json", text(&output.stderr));
}

#[test]
fn quiet_and_verbose_together_are_a_usage_error() {
    let output = imify(&["--quiet", "-v"], &[]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn global_flags_are_accepted_without_a_command() {
    let output = imify(&["--json", "--yes", "--no-color", "-vv"], &[]);

    assert!(output.status.success());
    assert_eq!(text(&output.stdout), "");
}

async fn api_answering(status: u16, body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(&server)
        .await;
    server
}

fn user_body() -> serde_json::Value {
    json!({"subject": "user-1234", "name": "Ada Lovelace", "role": "admin"})
}

async fn api_with_user() -> MockServer {
    api_answering(200, user_body()).await
}

async fn api_rejecting_the_token() -> MockServer {
    api_answering(401, json!({"detail": "Invalid token"})).await
}

fn whoami(server: &MockServer, extra_args: &[&str]) -> Output {
    let mut args = vec!["whoami"];
    args.extend_from_slice(extra_args);
    imify(
        &args,
        &[("IMIFY_TOKEN", TOKEN), ("IMIFY_API_URL", &server.uri())],
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_prints_the_user() {
    let server = api_with_user().await;

    let output = whoami(&server, &[]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("whoami_human", text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_json_prints_the_user_as_an_object() {
    let server = api_with_user().await;

    let output = whoami(&server, &["--json"]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("whoami_json", text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_sends_imify_token_as_the_bearer_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(user_body()))
        .expect(1)
        .mount(&server)
        .await;

    let output = whoami(&server, &[]);

    assert!(output.status.success(), "{}", text(&output.stderr));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_without_imify_token_fails_with_auth_required_and_sends_no_request() {
    let server = api_with_user().await;

    let output = imify(&["whoami", "--json"], &[("IMIFY_API_URL", &server.uri())]);

    assert_eq!(output.status.code(), Some(3));
    assert!(server.received_requests().await.unwrap().is_empty());
    insta::assert_snapshot!("whoami_auth_required_json", text(&output.stderr));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_with_a_rejected_token_fails_with_auth_rejected() {
    let server = api_rejecting_the_token().await;

    let output = whoami(&server, &["--json"]);

    assert_eq!(output.status.code(), Some(3));
    insta::assert_snapshot!("whoami_auth_rejected_json", text(&output.stderr));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_forbidden_with_a_json_body_fails_with_permission_denied() {
    let server = api_answering(403, json!({"detail": "Not allowed"})).await;

    let output = whoami(&server, &["--json"]);

    assert_eq!(output.status.code(), Some(3));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "permission_denied");
}

#[test]
fn whoami_without_imify_api_url_is_a_usage_error_naming_the_variable() {
    let output = imify(&["whoami", "--json"], &[("IMIFY_TOKEN", TOKEN)]);

    assert_eq!(output.status.code(), Some(2));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "invalid_argument");
    assert!(
        envelope["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("IMIFY_API_URL")
    );
}

#[test]
fn whoami_with_a_non_http_imify_api_url_is_a_usage_error() {
    let output = imify(
        &["whoami", "--json"],
        &[
            ("IMIFY_TOKEN", TOKEN),
            ("IMIFY_API_URL", "ftp://api.example.com"),
        ],
    );

    assert_eq!(output.status.code(), Some(2));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "invalid_argument");
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_trace_output_on_success_does_not_contain_the_token() {
    let server = api_with_user().await;

    let output = whoami(&server, &["-vv"]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(!text(&output.stdout).contains(TOKEN));
    assert!(!text(&output.stderr).contains(TOKEN));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_trace_output_on_a_rejected_token_does_not_contain_the_token() {
    let server = api_rejecting_the_token().await;

    let output = whoami(&server, &["-vv"]);

    assert_eq!(output.status.code(), Some(3));
    assert!(!text(&output.stdout).contains(TOKEN));
    assert!(!text(&output.stderr).contains(TOKEN));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_error_envelope_contains_no_part_of_the_token() {
    let server = api_rejecting_the_token().await;

    let output = whoami(&server, &["--json"]);

    let stderr = text(&output.stderr);
    for part in TOKEN.split(['_', '-']).filter(|part| part.len() >= 6) {
        assert!(!stderr.contains(part), "{stderr}");
    }
}
