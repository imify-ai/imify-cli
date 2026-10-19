//! `imify auth login` against a mock API and identity provider, with a browser double that follows
//! the redirect over TCP.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read as _;
use std::net::TcpStream;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::*;
use crate::credentials::FileStore;
use crate::output::renderer::{HumanRenderer, JsonRenderer};

const CLIENT_ID: &str = "client-1";
const CODE: &str = "code_4e1d-secret";
const REFRESH_TOKEN: &str = "rt_8b2f-secret";
const METADATA_PATH: &str = "/.well-known/oauth-protected-resource/api/v1";

/// The redirect ports are fixed, so tests that listen on them run one at a time.
static REDIRECT_PORTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Bytes written to a stream, readable while the stream is in use.
#[derive(Debug, Clone, Default)]
struct Buffer(Rc<RefCell<Vec<u8>>>);

impl Buffer {
    fn text(&self) -> String {
        String::from_utf8(self.0.borrow().clone()).unwrap()
    }
}

impl io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// An unsigned ID token from `issuer` for [`CLIENT_ID`].
fn id_token(issuer: &str, nonce: Option<&str>) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let mut claims = json!({
        "iss": issuer,
        "sub": "user-1234",
        "aud": CLIENT_ID,
        "exp": unix_now() + 3600,
        "iat": unix_now(),
    });
    if let Some(nonce) = nonce {
        claims["nonce"] = json!(nonce);
    }
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    format!("{header}.{payload}.c2lnbmF0dXJl")
}

/// The `nonce` of the authorization request, once the browser double or the paste double
/// has seen it.
type Nonce = Arc<Mutex<Option<String>>>;

/// Answers the code exchange with an ID token that carries the request's `nonce`.
struct TokenEndpoint {
    issuer: String,
    nonce: Nonce,
}

impl Respond for TokenEndpoint {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let nonce = self.nonce.lock().unwrap().clone();
        ResponseTemplate::new(200).set_body_json(json!({
            "id_token": id_token(&self.issuer, nonce.as_deref()),
            "refresh_token": REFRESH_TOKEN,
            "token_type": "Bearer",
            "expires_in": 3600,
        }))
    }
}

/// The API, the identity provider and a configuration directory.
struct Hosts {
    api: MockServer,
    provider: MockServer,
    nonce: Nonce,
    config_dir: tempfile::TempDir,
}

impl Hosts {
    async fn start() -> Self {
        let hosts = Self::start_without_config_json().await;
        Mock::given(method("GET"))
            .and(path("/config.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "authority": hosts.provider.uri(),
                "clientId": CLIENT_ID,
                "apiBaseUrl": "ignored",
            })))
            .mount(&hosts.api)
            .await;
        hosts
    }

    async fn start_without_config_json() -> Self {
        let api = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "subject": "user-1234",
                "name": "Ada Lovelace",
                "role": "admin",
            })))
            .mount(&api)
            .await;
        let provider = MockServer::start().await;
        let issuer = provider.uri();
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issuer": issuer,
                "authorization_endpoint": format!("{issuer}/oauth2/authorize"),
                "token_endpoint": format!("{issuer}/oauth2/token"),
            })))
            .mount(&provider)
            .await;
        let nonce = Nonce::default();
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(TokenEndpoint {
                issuer,
                nonce: Arc::clone(&nonce),
            })
            .mount(&provider)
            .await;
        Self {
            api,
            provider,
            nonce,
            config_dir: tempfile::tempdir().unwrap(),
        }
    }

    /// The environment of a login to this API with the file store.
    fn env(&self) -> Environment {
        Environment {
            api_url: Some(self.api.uri()),
            credential_store: Some("file".to_owned()),
            config_dir: Some(self.config_dir.path().to_path_buf()),
            stdin_is_tty: true,
            ..Environment::default()
        }
    }

    fn stored(&self) -> Option<Credentials> {
        FileStore::new(self.config_dir.path(), &self.api.uri())
            .load()
            .unwrap()
    }

    async fn requested(&self, server: &MockServer, request_path: &str) -> bool {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|request| request.url.path() == request_path)
    }
}

fn query(url: &str) -> HashMap<String, String> {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

/// Follows the redirect of the authorization request with an authorization code, as the
/// identity provider would after the user signs in.
#[derive(Debug)]
struct RedirectingBrowser {
    nonce: Nonce,
    stderr: Buffer,
    stderr_when_opened: RefCell<Option<String>>,
}

impl RedirectingBrowser {
    fn new(hosts: &Hosts, stderr: &Buffer) -> Self {
        Self {
            nonce: Arc::clone(&hosts.nonce),
            stderr: stderr.clone(),
            stderr_when_opened: RefCell::default(),
        }
    }
}

impl Browser for RedirectingBrowser {
    fn open(&self, url: &str) -> io::Result<()> {
        *self.stderr_when_opened.borrow_mut() = Some(self.stderr.text());
        let query = query(url);
        *self.nonce.lock().unwrap() = Some(query["nonce"].clone());
        let port = Url::parse(&query["redirect_uri"]).unwrap().port().unwrap();
        let state = query["state"].clone();
        std::thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(
                stream,
                "GET /callback?code={CODE}&state={state} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut response = String::new();
            let _ = stream.read_to_string(&mut response);
        });
        Ok(())
    }
}

/// A browser that must not be opened.
#[derive(Debug)]
struct NoBrowser;

impl Browser for NoBrowser {
    fn open(&self, _: &str) -> io::Result<()> {
        panic!("the browser was opened");
    }
}

/// What a login wrote.
struct Output {
    result: Result<(), CliError>,
    stdout: String,
    stderr: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Human,
    Json,
    /// `--json --quiet`.
    JsonQuiet,
}

fn renderer(mode: Mode, stdout: &Buffer, stderr: &Buffer) -> Box<dyn Renderer> {
    let stdout = Box::new(stdout.clone());
    let stderr = Box::new(stderr.clone());
    match mode {
        Mode::Human => Box::new(HumanRenderer::new(stdout, stderr, false)),
        Mode::Json => Box::new(JsonRenderer::new(stdout, stderr, false)),
        Mode::JsonQuiet => Box::new(JsonRenderer::new(stdout, stderr, true)),
    }
}

/// Runs `imify auth login` with the browser double.
async fn browser_login(
    hosts: &Hosts,
    env: &Environment,
    mode: Mode,
    keychain: Keychain,
    env_token_set: bool,
) -> (Output, RedirectingBrowser) {
    let _ports = REDIRECT_PORTS.lock().await;
    let (stdout, stderr) = (Buffer::default(), Buffer::default());
    let browser = RedirectingBrowser::new(hosts, &stderr);
    let mut prompt = stderr.clone();
    let mut no_paste = || -> io::Result<String> { panic!("the login read stdin") };
    let context = LoginContext {
        browser: &browser,
        keychain,
        prompt: &mut prompt,
        read_pasted_url: &mut no_paste,
        env_token_set,
    };
    let result = login_with(
        env,
        false,
        context,
        renderer(mode, &stdout, &stderr).as_mut(),
    )
    .await;
    let output = Output {
        result,
        stdout: stdout.text(),
        stderr: stderr.text(),
    };
    (output, browser)
}

async fn login_to(hosts: &Hosts, mode: Mode) -> Output {
    browser_login(hosts, &hosts.env(), mode, Keychain::unavailable(), false)
        .await
        .0
}

/// Compares `text` with the snapshot `name` in `tests/snapshots`, where the schema tests
/// read it.
fn assert_snapshot(name: &str, text: &str) {
    insta::with_settings!({ snapshot_path => "../../../tests/snapshots", prepend_module_to_snapshot => false }, {
        insta::assert_snapshot!(name, text);
    });
}

/// `text` with the random address of the mock API replaced.
fn redact_api(text: &str, hosts: &Hosts) -> String {
    text.replace(&hosts.api.uri(), "http://127.0.0.1:<port>")
}

#[tokio::test]
async fn login_names_the_api_and_the_identity_provider_before_it_opens_the_browser() {
    let hosts = Hosts::start().await;

    let (output, browser) = browser_login(
        &hosts,
        &hosts.env(),
        Mode::Human,
        Keychain::unavailable(),
        false,
    )
    .await;

    output.result.unwrap();
    let stderr_when_opened = browser.stderr_when_opened.borrow().clone().unwrap();
    assert!(
        stderr_when_opened.starts_with(&format!(
            "Signing in to {} through 127.0.0.1. Opening the browser.\n",
            hosts.api.uri()
        )),
        "{stderr_when_opened}"
    );
    assert!(stderr_when_opened.contains(&format!("{}/oauth2/authorize?", hosts.provider.uri())));
}

#[tokio::test]
async fn login_stores_the_credentials_and_prints_the_user() {
    let hosts = Hosts::start().await;

    let output = login_to(&hosts, Mode::Human).await;

    output.result.unwrap();
    assert!(hosts.stored().unwrap().has_refresh_token());
    let path = hosts.config_dir.path().join("credentials.json");
    let human = redact_api(&output.stdout, &hosts)
        .replace(&path.display().to_string(), "<config dir>/credentials.json");
    assert_snapshot("login_human", &human);
}

#[tokio::test]
async fn login_json_prints_the_host_the_user_and_the_storage() {
    let hosts = Hosts::start().await;

    let output = login_to(&hosts, Mode::Json).await;

    output.result.unwrap();
    assert_snapshot("login_json", &redact_api(&output.stdout, &hosts));
}

#[tokio::test]
async fn login_confirms_the_account_with_the_new_id_token() {
    let hosts = Hosts::start().await;

    login_to(&hosts, Mode::Human).await.result.unwrap();

    let requests = hosts.api.received_requests().await.unwrap();
    let me = requests
        .iter()
        .find(|request| request.url.path() == "/api/v1/me")
        .unwrap();
    let authorization = me.headers.get("authorization").unwrap().to_str().unwrap();
    let stored = serde_json::to_value(hosts.stored().unwrap()).unwrap();
    assert_eq!(
        authorization,
        format!("Bearer {}", stored["id_token"].as_str().unwrap())
    );
}

#[tokio::test]
async fn login_with_both_overrides_does_not_request_config_json() {
    let hosts = Hosts::start_without_config_json().await;
    let env = Environment {
        oidc_issuer: Some(hosts.provider.uri()),
        oidc_client_id: Some(CLIENT_ID.to_owned()),
        ..hosts.env()
    };

    let (output, _) =
        browser_login(&hosts, &env, Mode::Human, Keychain::unavailable(), false).await;

    output.result.unwrap();
    assert!(!hosts.requested(&hosts.api, "/config.json").await);
}

#[tokio::test]
async fn login_with_both_overrides_does_not_request_protected_resource_metadata() {
    let hosts = Hosts::start_without_config_json().await;
    let env = Environment {
        oidc_issuer: Some(hosts.provider.uri()),
        oidc_client_id: Some(CLIENT_ID.to_owned()),
        ..hosts.env()
    };

    let (output, _) =
        browser_login(&hosts, &env, Mode::Human, Keychain::unavailable(), false).await;

    output.result.unwrap();
    assert!(!hosts.requested(&hosts.api, METADATA_PATH).await);
}

#[tokio::test]
async fn login_uses_the_issuer_and_client_id_of_the_protected_resource_metadata() {
    let hosts = Hosts::start_without_config_json().await;
    Mock::given(method("GET"))
        .and(path(METADATA_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "resource": format!("{}/api/v1", hosts.api.uri()),
            "authorization_servers": [hosts.provider.uri()],
            "imify_cli_client_id": CLIENT_ID,
        })))
        .mount(&hosts.api)
        .await;

    let output = login_to(&hosts, Mode::Human).await;

    output.result.unwrap();
    assert!(!hosts.requested(&hosts.api, "/config.json").await);
}

#[tokio::test]
async fn login_with_only_the_client_id_override_reads_the_issuer_from_config_json() {
    let hosts = Hosts::start().await;
    let env = Environment {
        oidc_client_id: Some("other-client".to_owned()),
        ..hosts.env()
    };

    let (output, browser) =
        browser_login(&hosts, &env, Mode::Human, Keychain::unavailable(), false).await;

    // The mock provider issues ID tokens for `client-1`, so the audience check fails.
    assert_eq!(output.result.unwrap_err().code, ErrorCode::LoginFailed);
    let opened = browser.stderr_when_opened.borrow().clone().unwrap();
    assert!(opened.contains("client_id=other-client"));
    assert!(opened.contains(&hosts.provider.uri()));
    assert!(hosts.requested(&hosts.api, "/config.json").await);
}

#[tokio::test]
async fn login_without_a_keychain_service_stores_the_file_and_warns_even_when_quiet() {
    let hosts = Hosts::start().await;
    let env = Environment {
        credential_store: None,
        ..hosts.env()
    };

    let (output, _) = browser_login(
        &hosts,
        &env,
        Mode::JsonQuiet,
        Keychain::unavailable(),
        false,
    )
    .await;

    output.result.unwrap();
    let path = hosts.config_dir.path().join("credentials.json");
    assert_eq!(
        output
            .stderr
            .matches(&format!(
                "warning: The login is stored unencrypted in {}",
                path.display()
            ))
            .count(),
        1,
        "{}",
        output.stderr
    );
    let result: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(
        (&result["storage"], &result["storage_fallback"]),
        (&json!("file"), &json!(true))
    );
}

#[tokio::test]
async fn login_with_a_keychain_stores_the_login_there() {
    let hosts = Hosts::start().await;
    let env = Environment {
        credential_store: Some("keychain".to_owned()),
        ..hosts.env()
    };

    let (output, _) = browser_login(&hosts, &env, Mode::Json, Keychain::in_memory(), false).await;

    output.result.unwrap();
    let result: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(result["storage"], "keychain");
    assert!(hosts.stored().is_none());
}

#[tokio::test]
async fn login_warns_when_imify_token_takes_precedence() {
    let hosts = Hosts::start().await;

    let (output, _) = browser_login(
        &hosts,
        &hosts.env(),
        Mode::Human,
        Keychain::unavailable(),
        true,
    )
    .await;

    output.result.unwrap();
    assert!(
        output
            .stderr
            .contains("warning: IMIFY_TOKEN is set and takes precedence over the stored login.")
    );
}

#[tokio::test]
async fn login_writes_no_token_code_or_secret() {
    let hosts = Hosts::start().await;

    let output = login_to(&hosts, Mode::Json).await;

    output.result.unwrap();
    let stored = serde_json::to_value(hosts.stored().unwrap()).unwrap();
    let written = format!("{}{}", output.stdout, output.stderr);
    let authorization_url = written
        .lines()
        .find(|line| line.contains("/oauth2/authorize?"))
        .unwrap();
    let query = query(authorization_url);
    for secret in [
        stored["id_token"].as_str().unwrap(),
        REFRESH_TOKEN,
        CODE,
        query["nonce"].as_str(),
        query["state"].as_str(),
    ] {
        assert_eq!(
            written.matches(secret).count(),
            usize::from(secret == query["nonce"] || secret == query["state"]),
            "{secret} appears outside the authorization URL"
        );
    }
}

/// Runs `imify auth login --no-browser`, pasting what `paste` returns for the authorization URL.
async fn paste_login(
    hosts: &Hosts,
    env: &Environment,
    paste: impl Fn(&str) -> io::Result<String>,
) -> Output {
    let (stdout, stderr) = (Buffer::default(), Buffer::default());
    let mut prompt = stderr.clone();
    let nonce = Arc::clone(&hosts.nonce);
    let written = stderr.clone();
    let mut read_pasted_url = || {
        let text = written.text();
        let url = text
            .lines()
            .find(|line| line.contains("/oauth2/authorize?"))
            .unwrap()
            .to_owned();
        *nonce.lock().unwrap() = Some(query(&url)["nonce"].clone());
        paste(&url)
    };
    let context = LoginContext {
        browser: &NoBrowser,
        keychain: Keychain::unavailable(),
        prompt: &mut prompt,
        read_pasted_url: &mut read_pasted_url,
        env_token_set: false,
    };
    let result = login_with(
        env,
        true,
        context,
        renderer(Mode::Human, &stdout, &stderr).as_mut(),
    )
    .await;
    Output {
        result,
        stdout: stdout.text(),
        stderr: stderr.text(),
    }
}

fn redirect_for(authorization_url: &str) -> String {
    let query = query(authorization_url);
    format!(
        "{}?code={CODE}&state={}",
        query["redirect_uri"], query["state"]
    )
}

#[tokio::test]
async fn login_without_browser_reads_the_pasted_redirect_url() {
    let hosts = Hosts::start().await;

    let output = paste_login(&hosts, &hosts.env(), |url| Ok(redirect_for(url))).await;

    output.result.unwrap();
    assert!(hosts.stored().is_some());
    assert!(output.stderr.starts_with(&format!(
        "Signing in to {} through 127.0.0.1. Open this URL in a browser:\n",
        hosts.api.uri()
    )));
    assert!(
        output
            .stderr
            .contains("Paste the URL from the browser's address bar: ")
    );
}

#[tokio::test]
async fn login_without_browser_rejects_a_pasted_text_that_is_not_the_redirect_url() {
    let hosts = Hosts::start().await;

    let output = paste_login(&hosts, &hosts.env(), |_| Ok("hello".to_owned())).await;

    assert_eq!(output.result.unwrap_err().code, ErrorCode::InvalidArgument);
    assert!(hosts.stored().is_none());
}

#[tokio::test]
async fn login_without_browser_and_without_a_terminal_fails_before_any_request() {
    let hosts = Hosts::start().await;
    let env = Environment {
        stdin_is_tty: false,
        ..hosts.env()
    };

    let output = paste_login(&hosts, &env, |_| panic!("stdin was read")).await;

    assert_eq!(output.result.unwrap_err().code, ErrorCode::InvalidArgument);
    assert!(hosts.api.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn logout_after_login_revokes_and_removes_the_login() {
    let hosts = Hosts::start().await;
    login_to(&hosts, Mode::Human).await.result.unwrap();
    Mock::given(method("POST"))
        .and(path("/oauth2/revoke"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&hosts.provider)
        .await;
    let (stdout, stderr) = (Buffer::default(), Buffer::default());

    logout_with(
        &hosts.env(),
        Keychain::unavailable(),
        renderer(Mode::Json, &stdout, &stderr).as_mut(),
    )
    .await
    .unwrap();

    let result: Value = serde_json::from_str(&stdout.text()).unwrap();
    assert_eq!(
        (result["removed"].clone(), result["revoked"].clone()),
        (json!(true), json!(true))
    );
    assert!(hosts.stored().is_none());
    assert_eq!(stderr.text(), "");
}

/// Trace output of the imify crates, captured as `-vv` writes it.
#[derive(Clone, Default)]
struct TraceLog(Arc<Mutex<Vec<u8>>>);

impl TraceLog {
    /// Captures the trace output on this thread until the guard is dropped.
    fn capture(&self) -> tracing::subscriber::DefaultGuard {
        use tracing_subscriber::layer::SubscriberExt as _;

        let level = tracing::level_filters::LevelFilter::TRACE;
        tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(self.clone())
                .with_max_level(level)
                .finish()
                .with(crate::output::log_filter(level)),
        )
    }

    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl io::Write for TraceLog {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TraceLog {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Asserts that `trace` contains none of the login's secrets. `stderr` holds the
/// authorization URL with `state` and `nonce`.
fn assert_no_secrets(trace: &str, stderr: &str, hosts: &Hosts) {
    let stored = serde_json::to_value(hosts.stored().unwrap()).unwrap();
    let authorization_url = stderr
        .lines()
        .find(|line| line.contains("/oauth2/authorize?"))
        .unwrap();
    let query = query(authorization_url);
    for secret in [
        stored["id_token"].as_str().unwrap(),
        REFRESH_TOKEN,
        CODE,
        query["nonce"].as_str(),
        query["state"].as_str(),
        query["code_challenge"].as_str(),
    ] {
        assert!(
            !trace.contains(secret),
            "{secret} appears in the trace output"
        );
    }
}

#[tokio::test]
async fn login_trace_output_contains_no_token_code_or_secret() {
    let hosts = Hosts::start().await;
    let trace = TraceLog::default();
    let guard = trace.capture();

    let output = login_to(&hosts, Mode::Human).await;

    drop(guard);
    output.result.unwrap();
    assert_no_secrets(&trace.text(), &output.stderr, &hosts);
}

#[tokio::test]
async fn login_without_browser_trace_output_contains_no_token_code_or_secret() {
    let hosts = Hosts::start().await;
    let trace = TraceLog::default();
    let guard = trace.capture();

    let output = paste_login(&hosts, &hosts.env(), |url| Ok(redirect_for(url))).await;

    drop(guard);
    output.result.unwrap();
    assert_no_secrets(&trace.text(), &output.stderr, &hosts);
}

#[tokio::test]
async fn an_escape_sequence_from_the_identity_provider_is_not_written_raw() {
    let hosts = Hosts::start().await;
    let output = paste_login(&hosts, &hosts.env(), |url| {
        let query = query(url);
        Ok(format!(
            "{}?error=access_denied&error_description=%1B%5D0%3Bpwned%07%1B%5B2J&state={}",
            query["redirect_uri"], query["state"]
        ))
    })
    .await;
    let error = output.result.unwrap_err();
    let stderr = Buffer::default();

    renderer(Mode::Human, &Buffer::default(), &stderr)
        .error(&error)
        .unwrap();

    assert_eq!(error.code, ErrorCode::LoginFailed);
    assert!(!stderr.text().contains('\u{1b}'), "{:?}", stderr.text());
    assert!(
        stderr.text().contains("\\u{1b}]0;pwned"),
        "{:?}",
        stderr.text()
    );
}

#[tokio::test]
async fn ctrl_c_at_the_paste_prompt_stops_the_login_as_interrupted() {
    let hosts = Hosts::start().await;

    let output = paste_login(&hosts, &hosts.env(), |_| {
        Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
    })
    .await;

    let error = output.result.unwrap_err();
    assert_eq!(
        (error.code, error.code.exit_code()),
        (ErrorCode::Interrupted, 130)
    );
    assert!(hosts.stored().is_none());
}
