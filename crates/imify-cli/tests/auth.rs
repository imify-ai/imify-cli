//! The `imify` binary with a login stored in the file store, against a mock API and identity
//! provider. The browser login itself is tested in `src/commands/auth/tests.rs`.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;
use std::time::{SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CLIENT_ID: &str = "client-1";
const REFRESH_TOKEN: &str = "rt_5b1e-secret";
const NEW_REFRESH_TOKEN: &str = "rt_9c4d-rotated-secret";
const ENV_TOKEN: &str = "tok_7f3a9c-secret";

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// An unsigned ID token from `issuer`, expiring `lifetime` seconds from now.
fn id_token(issuer: &str, subject: &str, lifetime: u64) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(
        json!({
            "iss": issuer,
            "sub": subject,
            "aud": CLIENT_ID,
            "exp": unix_now() + lifetime,
            "iat": unix_now(),
        })
        .to_string(),
    );
    format!("{header}.{payload}.c2lnbmF0dXJl")
}

/// The API, the identity provider and a configuration directory with a stored login.
struct Login {
    api: MockServer,
    provider: MockServer,
    config_dir: tempfile::TempDir,
    id_token: String,
    /// Unix seconds.
    expires_at: u64,
}

impl Login {
    /// A login whose ID token expires `lifetime` seconds from now.
    async fn stored(lifetime: u64) -> std::io::Result<Self> {
        let api = MockServer::start().await;
        let provider = MockServer::start().await;
        let config_dir = tempfile::tempdir()?;
        let id_token = id_token(&provider.uri(), "stored", lifetime);
        let login = Self {
            api,
            provider,
            config_dir,
            id_token,
            expires_at: unix_now() + lifetime,
        };
        login.write_credentials()?;
        Ok(login)
    }

    fn write_credentials(&self) -> std::io::Result<()> {
        let issuer = self.provider.uri();
        let file = json!({
            self.api.uri(): {
                "version": 1,
                "issuer": issuer,
                "client_id": CLIENT_ID,
                "token_endpoint": format!("{issuer}/oauth2/token"),
                "revocation_endpoint": format!("{issuer}/oauth2/revoke"),
                "id_token": self.id_token,
                "id_token_expires_at": self.expires_at,
                "refresh_token": REFRESH_TOKEN,
            }
        });
        let path = self.credentials_file();
        std::fs::write(&path, file.to_string())?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
    }

    fn credentials_file(&self) -> std::path::PathBuf {
        self.config_dir.path().join("credentials.json")
    }

    fn stored_entry(&self) -> Option<Value> {
        let content = std::fs::read_to_string(self.credentials_file()).ok()?;
        let file: Value = serde_json::from_str(&content).ok()?;
        file.get(self.api.uri()).cloned()
    }

    async fn answer_me(&self) {
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "subject": "user-1234",
                "name": "Ada Lovelace",
                "role": "admin",
            })))
            .mount(&self.api)
            .await;
    }

    async fn answer_token(&self, response: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(response)
            .mount(&self.provider)
            .await;
    }

    async fn answer_revoke(&self, status: u16) {
        Mock::given(method("POST"))
            .and(path("/oauth2/revoke"))
            .and(body_string_contains("token_type_hint=refresh_token"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&self.provider)
            .await;
    }

    async fn requests_to(&self, server: &MockServer, request_path: &str) -> usize {
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|request| request.url.path() == request_path)
            .count()
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        run(&self.api.uri(), self.config_dir.path(), args, env)
    }

    /// `text` with the random addresses of the mock servers replaced.
    fn redact(&self, text: &str) -> String {
        text.replace(&self.api.uri(), "http://127.0.0.1:<api port>")
            .replace(&self.provider.uri(), "http://127.0.0.1:<provider port>")
    }
}

/// Runs `imify` for the API at `api_url` with the file store in `config_dir`.
fn run(api_url: &str, config_dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_imify"));
    command
        .env_remove("IMIFY_OUTPUT")
        .env_remove("NO_COLOR")
        .env_remove("IMIFY_TOKEN")
        .env_remove("IMIFY_OIDC_ISSUER")
        .env_remove("IMIFY_OIDC_CLIENT_ID")
        .env("IMIFY_API_URL", api_url)
        .env("IMIFY_CREDENTIAL_STORE", "file")
        .env("IMIFY_CONFIG_DIR", config_dir);
    for (key, value) in env {
        command.env(key, value);
    }
    command.args(args).assert().get_output().clone()
}

fn envelope(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr).unwrap_or(Value::Null)
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_sends_the_stored_id_token() {
    let login = Login::stored(3600).await.unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .and(header(
            "authorization",
            format!("Bearer {}", login.id_token).as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "subject": "user-1234", "name": "Ada Lovelace", "role": "admin",
        })))
        .expect(1)
        .mount(&login.api)
        .await;

    let output = login.run(&["whoami"], &[]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(login.requests_to(&login.provider, "/oauth2/token").await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_refreshes_an_id_token_that_expires_within_60_s_and_stores_it() {
    let login = Login::stored(30).await.unwrap();
    let fresh = id_token(&login.provider.uri(), "fresh", 3600);
    login
        .answer_token(ResponseTemplate::new(200).set_body_json(json!({
            "id_token": fresh,
            "refresh_token": NEW_REFRESH_TOKEN,
            "token_type": "Bearer",
        })))
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .and(header("authorization", format!("Bearer {fresh}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "subject": "user-1234", "name": "Ada Lovelace", "role": "admin",
        })))
        .expect(1)
        .mount(&login.api)
        .await;

    let output = login.run(&["whoami"], &[]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    let stored = login.stored_entry().unwrap();
    assert_eq!(
        (
            stored["id_token"].as_str(),
            stored["refresh_token"].as_str()
        ),
        (Some(fresh.as_str()), Some(NEW_REFRESH_TOKEN))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_with_a_revoked_refresh_token_fails_with_auth_expired() {
    let login = Login::stored(30).await.unwrap();
    login
        .answer_token(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "Refresh Token has been revoked",
        })))
        .await;
    login.answer_me().await;

    let output = login.run(&["whoami", "--json"], &[]);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(login.requests_to(&login.api, "/api/v1/me").await, 0);
    insta::assert_snapshot!("whoami_auth_expired_json", text(&output.stderr));
}

#[tokio::test(flavor = "multi_thread")]
async fn imify_token_takes_precedence_over_the_stored_login() {
    let login = Login::stored(3600).await.unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .and(header(
            "authorization",
            format!("Bearer {ENV_TOKEN}").as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "subject": "user-1234", "name": "Ada Lovelace", "role": "admin",
        })))
        .expect(1)
        .mount(&login.api)
        .await;

    let output = login.run(&["whoami"], &[("IMIFY_TOKEN", ENV_TOKEN)]);

    assert!(output.status.success(), "{}", text(&output.stderr));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_stored_login_hints_at_imify_login() {
    let login = Login::stored(3600).await.unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"detail": "Invalid token"})))
        .mount(&login.api)
        .await;

    let output = login.run(&["whoami", "--json"], &[]);

    let envelope = envelope(&output);
    assert_eq!(
        (&envelope["error"]["code"], &envelope["error"]["hint"]),
        (&json!("auth_rejected"), &json!("Run `imify login`."))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_without_a_login_names_the_host() {
    let api = MockServer::start().await;
    let config_dir = tempfile::tempdir().unwrap();

    let output = run(&api.uri(), config_dir.path(), &["whoami", "--json"], &[]);

    assert_eq!(output.status.code(), Some(3));
    let envelope = envelope(&output);
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains(&api.uri())
    );
    assert_eq!(
        envelope["error"]["hint"],
        "Run `imify login`, or set `IMIFY_TOKEN` to a token."
    );
}

#[test]
fn an_unsupported_credential_store_is_a_usage_error() {
    let config_dir = tempfile::tempdir().unwrap();

    let output = run(
        "https://api.example.com",
        config_dir.path(),
        &["whoami", "--json"],
        &[("IMIFY_CREDENTIAL_STORE", "vault")],
    );

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(envelope(&output)["error"]["code"], "invalid_argument");
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_revokes_the_refresh_token_and_removes_the_login() {
    let login = Login::stored(3600).await.unwrap();
    login.answer_revoke(200).await;

    let output = login.run(&["logout", "--json"], &[]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        login.requests_to(&login.provider, "/oauth2/revoke").await,
        1
    );
    assert!(login.stored_entry().is_none());
    assert_eq!(text(&output.stderr), "");
    insta::assert_snapshot!("logout_json", login.redact(&text(&output.stdout)));
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_says_that_issued_tokens_stay_valid() {
    let login = Login::stored(3600).await.unwrap();
    login.answer_revoke(200).await;

    let output = login.run(&["logout"], &[]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("logout_human", login.redact(&text(&output.stdout)));
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_without_a_login_succeeds_and_removes_nothing() {
    let api = MockServer::start().await;
    let config_dir = tempfile::tempdir().unwrap();

    let output = run(&api.uri(), config_dir.path(), &["logout", "--json"], &[]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        (&result["removed"], &result["revoked"]),
        (&json!(false), &json!(false))
    );
    insta::assert_snapshot!(
        "logout_not_logged_in_json",
        text(&output.stdout).replace(&api.uri(), "http://127.0.0.1:<api port>")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_removes_the_login_and_warns_when_revocation_is_unsupported() {
    let login = Login::stored(3600).await.unwrap();
    login.answer_revoke(404).await;

    let output = login.run(&["logout", "--json"], &[]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        (&result["removed"], &result["revoked"]),
        (&json!(true), &json!(false))
    );
    assert!(
        text(&output.stderr)
            .starts_with("warning: The identity provider does not support revocation.")
    );
    assert!(login.stored_entry().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_removes_the_login_and_warns_when_revocation_fails() {
    let login = Login::stored(3600).await.unwrap();
    login.answer_revoke(400).await;

    let output = login.run(&["logout", "--json"], &[]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["revoked"], false);
    assert!(text(&output.stderr).starts_with("warning: Cannot revoke the refresh token"));
    assert!(login.stored_entry().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn trace_output_of_whoami_and_logout_contains_no_token() {
    let login = Login::stored(30).await.unwrap();
    let fresh = id_token(&login.provider.uri(), "fresh", 3600);
    login
        .answer_token(ResponseTemplate::new(200).set_body_json(json!({
            "id_token": fresh,
            "refresh_token": NEW_REFRESH_TOKEN,
        })))
        .await;
    login.answer_me().await;
    login.answer_revoke(200).await;

    let whoami = login.run(&["whoami", "-vv"], &[]);
    let logout = login.run(&["logout", "-vv"], &[]);

    assert!(whoami.status.success(), "{}", text(&whoami.stderr));
    assert!(logout.status.success(), "{}", text(&logout.stderr));
    let written = [&whoami, &logout]
        .map(|output| text(&output.stdout) + &text(&output.stderr))
        .concat();
    for secret in [
        login.id_token.as_str(),
        fresh.as_str(),
        REFRESH_TOKEN,
        NEW_REFRESH_TOKEN,
    ] {
        assert!(!written.contains(secret), "{secret} was written");
    }
}

#[test]
fn http_api_url_on_a_remote_host_fails_before_any_request() {
    let config_dir = tempfile::tempdir().unwrap();

    let outputs = [["whoami", "--json"], ["login", "--json"]].map(|args| {
        run(
            "http://api.example.com",
            config_dir.path(),
            &args,
            &[("IMIFY_TOKEN", ENV_TOKEN)],
        )
    });

    for output in outputs {
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(envelope(&output)["error"]["code"], "invalid_argument");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn http_api_url_on_localhost_is_accepted() {
    let login = Login::stored(3600).await.unwrap();
    login.answer_me().await;
    let api_url = login.api.uri().replace("127.0.0.1", "localhost");

    let output = run(
        &api_url,
        login.config_dir.path(),
        &["whoami"],
        &[("IMIFY_TOKEN", ENV_TOKEN)],
    );

    assert!(output.status.success(), "{}", text(&output.stderr));
}

#[tokio::test(flavor = "multi_thread")]
async fn login_without_config_json_and_overrides_hints_at_the_overrides() {
    let api = MockServer::start().await;
    let config_dir = tempfile::tempdir().unwrap();

    let output = run(&api.uri(), config_dir.path(), &["login", "--json"], &[]);

    assert_eq!(output.status.code(), Some(2));
    let envelope = envelope(&output);
    assert_eq!(
        (&envelope["error"]["code"], &envelope["error"]["hint"]),
        (
            &json!("invalid_argument"),
            &json!("Set `IMIFY_OIDC_ISSUER` and `IMIFY_OIDC_CLIENT_ID`.")
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn login_without_browser_and_without_a_terminal_is_a_usage_error() {
    let api = MockServer::start().await;
    let config_dir = tempfile::tempdir().unwrap();

    let output = run(
        &api.uri(),
        config_dir.path(),
        &["login", "--no-browser", "--json"],
        &[],
    );

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(envelope(&output)["error"]["code"], "invalid_argument");
    assert!(api.received_requests().await.unwrap().is_empty());
}

#[test]
fn login_and_logout_help_show_examples() {
    let config_dir = tempfile::tempdir().unwrap();

    for command in ["login", "logout"] {
        let output = run(
            "https://api.example.com",
            config_dir.path(),
            &[command, "--help"],
            &[],
        );
        assert!(text(&output.stdout).contains(&format!("imify {command}")));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_escape_sequence_in_an_api_error_is_not_written_raw() {
    let login = Login::stored(3600).await.unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"detail": "Invalid token\u{1b}]0;pwned\u{7}\u{1b}[2J"})),
        )
        .mount(&login.api)
        .await;

    let output = login.run(&["whoami"], &[]);

    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(3));
    assert!(!stderr.contains('\u{1b}'), "{stderr:?}");
    assert!(
        stderr.contains("Invalid token\\u{1b}]0;pwned"),
        "{stderr:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_removes_a_login_it_cannot_read_and_does_not_revoke() {
    let login = Login::stored(3600).await.unwrap();
    login.answer_revoke(200).await;
    std::fs::set_permissions(
        login.credentials_file(),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();

    let output = login.run(&["logout", "--json"], &[]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        (&result["removed"], &result["revoked"]),
        (&json!(true), &json!(false))
    );
    assert!(text(&output.stderr).starts_with("warning: Cannot read the stored login"));
    assert_eq!(
        login.requests_to(&login.provider, "/oauth2/revoke").await,
        0
    );
    assert!(!login.credentials_file().exists());
}
