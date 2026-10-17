use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Output;

use assert_cmd::Command;
use imify_test_support::{DatasetBuilder, Defect, Format, SyntheticDataset};
use serde_json::json;
use wiremock::matchers::{header, method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const TOKEN: &str = "tok_7f3a9c-secret";

/// The `imify` binary without the environment variables the CLI reads. Logins are read from
/// a file in a directory that does not exist, never from the keychain.
fn imify_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_imify"));
    command
        .env_remove("IMIFY_OUTPUT")
        .env_remove("NO_COLOR")
        .env_remove("IMIFY_TOKEN")
        .env_remove("IMIFY_API_URL")
        .env_remove("IMIFY_PROFILE")
        .env_remove("IMIFY_OIDC_ISSUER")
        .env_remove("IMIFY_OIDC_CLIENT_ID")
        .env("IMIFY_CREDENTIAL_STORE", "file")
        .env("IMIFY_CONFIG_DIR", no_login_dir());
    command
}

fn imify(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = imify_command();
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
fn top_level_login_logout_and_whoami_are_usage_errors() {
    for command in ["login", "logout", "whoami"] {
        let output = imify(&[command, "--json"], &[("IMIFY_TOKEN", TOKEN)]);

        assert_eq!(output.status.code(), Some(2), "imify {command}");
        assert_eq!(text(&output.stdout), "", "imify {command}");
    }
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
    let mut args = vec!["auth", "whoami"];
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

    let output = imify(
        &["auth", "whoami", "--json"],
        &[("IMIFY_API_URL", &server.uri())],
    );

    assert_eq!(output.status.code(), Some(3));
    assert!(server.received_requests().await.unwrap().is_empty());
    insta::assert_snapshot!(
        "whoami_auth_required_json",
        text(&output.stderr).replace(&server.uri(), "http://127.0.0.1:<port>")
    );
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

/// Without a token and a login, no request is sent, so the default host is never contacted.
#[test]
fn whoami_without_an_api_url_targets_the_default_host() {
    let output = imify(&["auth", "whoami", "--json"], &[]);

    assert_eq!(output.status.code(), Some(3));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "auth_required");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("https://platform.imify.ai")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_api_url_flag_takes_precedence_over_imify_api_url() {
    let server = api_with_user().await;

    let output = imify(
        &["auth", "whoami", "--json", "--api-url", &server.uri()],
        &[
            ("IMIFY_TOKEN", TOKEN),
            ("IMIFY_API_URL", "https://unused.invalid"),
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
}

#[test]
fn an_invalid_api_url_flag_is_a_usage_error_naming_the_flag() {
    let output = imify(
        &[
            "auth",
            "whoami",
            "--json",
            "--api-url",
            "http://api.example.com",
        ],
        &[("IMIFY_TOKEN", TOKEN)],
    );

    assert_eq!(output.status.code(), Some(2));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("--api-url is invalid")
    );
}

#[test]
fn whoami_with_a_non_http_imify_api_url_is_a_usage_error() {
    let output = imify(
        &["auth", "whoami", "--json"],
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

const PICK_ID: &str = "0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13";
const STACK_ID: &str = "5e2d7a90-1c3b-4e6f-8a7d-9b0c1d2e3f45";
const POUR_ID: &str = "9a4c1e7b-3f2d-4b8a-a6c5-0d1e2f3a4b5c";

fn dataset_body(
    id: &str,
    task: &str,
    format: &str,
    embodiment: &str,
    episodes: u64,
    accepted: u64,
) -> serde_json::Value {
    json!({
        "id": id,
        "task_description": task,
        "dataset_format": format,
        "embodiment_label": embodiment,
        "embodiment_fingerprint": format!("{embodiment}-f3a9c1"),
        "cameras": ["wrist", "top"],
        "action_dim": 6,
        "created_at": "2026-10-01T09:30:00Z",
        "episode_count": episodes,
        "accepted_count": accepted
    })
}

fn datasets_body() -> serde_json::Value {
    json!([
        dataset_body(STACK_ID, "Stack two blocks", "lerobot_v3", "koch", 12, 12),
        dataset_body(
            PICK_ID,
            "Pick up the red cube",
            "lerobot_v3",
            "so100",
            50,
            48
        ),
        dataset_body(POUR_ID, "Pour water", "lerobot_v2_1", "so100", 9, 7),
    ])
}

async fn api_with_datasets(status: u16, body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/datasets"))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(&server)
        .await;
    server
}

fn datasets_ls(server: &MockServer, extra_args: &[&str]) -> Output {
    let mut args = vec!["datasets", "ls"];
    args.extend_from_slice(extra_args);
    imify(
        &args,
        &[("IMIFY_TOKEN", TOKEN), ("IMIFY_API_URL", &server.uri())],
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_ls_prints_the_datasets_as_a_table() {
    let server = api_with_datasets(200, datasets_body()).await;

    let output = datasets_ls(&server, &[]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("datasets_ls_human", text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_ls_json_prints_every_field_as_an_array() {
    let server = api_with_datasets(200, datasets_body()).await;

    let output = datasets_ls(&server, &["--json"]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("datasets_ls_json", text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_ls_without_datasets_prints_no_datasets() {
    let server = api_with_datasets(200, json!([])).await;

    let output = datasets_ls(&server, &[]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("datasets_ls_empty_human", text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_ls_json_without_datasets_prints_an_empty_array() {
    let server = api_with_datasets(200, json!([])).await;

    let output = datasets_ls(&server, &["--json"]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("datasets_ls_empty_json", text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_ls_json_filters_and_sorts_before_printing() {
    let server = api_with_datasets(200, datasets_body()).await;

    let output = datasets_ls(
        &server,
        &["--json", "--format", "lerobot_v3", "--sort", "episodes"],
    );

    assert!(output.status.success(), "{}", text(&output.stderr));
    let datasets: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    let ids: Vec<&str> = datasets
        .iter()
        .map(|dataset| dataset["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [STACK_ID, PICK_ID]);
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_ls_without_imify_token_fails_with_auth_required_and_sends_no_request() {
    let server = api_with_datasets(200, datasets_body()).await;

    let output = imify(
        &["datasets", "ls", "--json"],
        &[("IMIFY_API_URL", &server.uri())],
    );

    assert_eq!(output.status.code(), Some(3));
    assert!(server.received_requests().await.unwrap().is_empty());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "auth_required");
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_ls_with_a_rejected_token_fails_with_auth_rejected() {
    let server = api_with_datasets(401, json!({"detail": "Invalid token"})).await;

    let output = datasets_ls(&server, &["--json"]);

    assert_eq!(output.status.code(), Some(3));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "auth_rejected");
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_ls_with_a_body_that_is_not_a_list_of_datasets_fails_with_internal() {
    let server = api_with_datasets(200, json!({"items": []})).await;

    let output = datasets_ls(&server, &["--json"]);

    assert_eq!(output.status.code(), Some(1));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "internal");
}

#[test]
fn datasets_ls_with_an_unknown_sort_field_is_a_usage_error() {
    let output = imify(&["datasets", "ls", "--json", "--sort", "size"], &[]);

    assert_eq!(output.status.code(), Some(2));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "invalid_argument");
}

#[test]
fn datasets_ls_help_shows_examples() {
    let output = imify(&["datasets", "ls", "--help"], &[]);

    assert!(output.status.success());
    assert!(text(&output.stdout).contains("Examples:\n  imify datasets ls\n"));
}

/// Runs `imify datasets validate <path>` in the root of `dataset`, so output names no
/// temporary directory.
fn validate_path(dataset: &SyntheticDataset, path: &str, extra_args: &[&str]) -> Output {
    let mut args = vec!["datasets", "validate", path];
    args.extend_from_slice(extra_args);
    imify_command()
        .current_dir(dataset.root())
        .args(args)
        .assert()
        .get_output()
        .clone()
}

fn validate(dataset: &SyntheticDataset, extra_args: &[&str]) -> Output {
    validate_path(dataset, ".", extra_args)
}

fn dataset(format: Format, defects: &[Defect]) -> std::io::Result<SyntheticDataset> {
    defects
        .iter()
        .fold(DatasetBuilder::new(format), |builder, defect| {
            builder.defect(defect.clone())
        })
        .build()
}

fn invalid_dataset() -> std::io::Result<SyntheticDataset> {
    dataset(
        Format::V2_1,
        &[
            Defect::EpisodeLength,
            Defect::StrayFile("notes.txt".to_owned()),
        ],
    )
}

#[test]
fn datasets_validate_prints_the_report_of_a_valid_dataset() {
    let output = validate(&dataset(Format::V3_0, &[]).unwrap(), &[]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(text(&output.stderr), "");
    insta::assert_snapshot!("datasets_validate_valid_human", text(&output.stdout));
}

#[test]
fn datasets_validate_json_prints_the_report_of_a_valid_dataset() {
    let output = validate(&dataset(Format::V3_0, &[]).unwrap(), &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    insta::assert_snapshot!("datasets_validate_valid_json", text(&output.stdout));
}

#[test]
fn datasets_validate_with_warnings_only_exits_0() {
    let defects = [
        Defect::StrayFile("notes.txt".to_owned()),
        Defect::MissingVideo,
    ];

    let output = validate(&dataset(Format::V2_1, &defects).unwrap(), &[]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    insta::assert_snapshot!("datasets_validate_warnings_human", text(&output.stdout));
}

#[test]
fn datasets_validate_prints_the_report_and_fails_with_dataset_invalid() {
    let output = validate(&invalid_dataset().unwrap(), &[]);

    assert_eq!(output.status.code(), Some(5));
    insta::assert_snapshot!("datasets_validate_invalid_human", text(&output.stdout));
    insta::assert_snapshot!(
        "datasets_validate_invalid_human_stderr",
        text(&output.stderr)
    );
}

#[test]
fn datasets_validate_json_writes_the_report_to_stdout_and_the_envelope_to_stderr() {
    let output = validate(&invalid_dataset().unwrap(), &["--json"]);

    assert_eq!(output.status.code(), Some(5));
    insta::assert_snapshot!("datasets_validate_invalid_json", text(&output.stdout));
    insta::assert_snapshot!(
        "datasets_validate_invalid_envelope_json",
        text(&output.stderr)
    );
}

#[test]
fn datasets_validate_of_a_missing_directory_is_a_usage_error_naming_meta_info_json() {
    let output = validate_path(
        &dataset(Format::V2_1, &[]).unwrap(),
        "no-such-dataset",
        &["--json"],
    );

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(text(&output.stdout), "");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "invalid_argument");
    assert!(
        envelope["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("meta/info.json")
    );
}
const SESSION_ID: &str = "3f2c9a";
const TASK: &str = "Pick up the red cube";
const SESSIONS: &str = "/api/v1/ingest/sessions";
const FILES: &str = "/api/v1/ingest/sessions/3f2c9a/files/";
const COMPLETE: &str = "/api/v1/ingest/sessions/3f2c9a/complete";

fn session_body(status: &str) -> serde_json::Value {
    json!({
        "id": SESSION_ID,
        "name": "pick-cube",
        "dataset_format": null,
        "robot_type": null,
        "fps": null,
        "total_episodes": null,
        "status": status,
        "task_description": TASK,
        "created_at": "2026-10-16T09:30:00Z"
    })
}

fn completed_session_body() -> serde_json::Value {
    let mut body = session_body("ready");
    body["dataset_format"] = json!("lerobot_v3");
    body["robot_type"] = json!("so100");
    body["fps"] = json!(30.0);
    body["total_episodes"] = json!(2);
    body
}

/// Answers a file `PUT` the way the API does: with the stored path and size.
struct Store;

impl Respond for Store {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let stored = request.url.path().strip_prefix(FILES).unwrap_or_default();
        ResponseTemplate::new(200).set_body_json(json!({
            "path": stored,
            "size": request.body.len(),
        }))
    }
}

/// An ingest API that creates session `3f2c9a`, stores every file and answers `complete` with
/// `complete`.
async fn ingest_api_completing_with(complete: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    mount_ingest_api(&server, complete).await;
    server
}

/// Mounts the mocks of [`ingest_api_completing_with`] on `server`.
async fn mount_ingest_api(server: &MockServer, complete: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(SESSIONS))
        .respond_with(ResponseTemplate::new(201).set_body_json(session_body("uploading")))
        .mount(server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(format!("^{FILES}")))
        .respond_with(Store)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(COMPLETE))
        .respond_with(complete)
        .mount(server)
        .await;
}

fn completed() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(completed_session_body())
}

async fn ingest_api() -> MockServer {
    ingest_api_completing_with(completed()).await
}

/// A synthetic dataset in a directory named `pick-cube`, next to a home directory for the
/// upload journal; both removed on drop.
struct PickCube {
    _parent: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    files: Vec<PathBuf>,
}

fn pick_cube(defects: &[Defect]) -> std::io::Result<PickCube> {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("pick-cube");
    let builder = defects.iter().fold(
        DatasetBuilder::new(Format::V3_0).episodes(2),
        |builder, defect| builder.defect(defect.clone()),
    );
    let files = builder.write_to(&root)?;
    Ok(PickCube {
        home: parent.path().join("home"),
        _parent: parent,
        root,
        files,
    })
}

impl PickCube {
    /// The upload journal directory with `HOME` and `XDG_STATE_HOME` set by [`upload_task`].
    fn journal_dir(&self) -> PathBuf {
        self.home.join(".local/state/imify/uploads")
    }

    /// The files of the dataset as the API names them: relative, with `/` separators.
    fn upload_paths(&self) -> BTreeSet<String> {
        self.files
            .iter()
            .map(|file| file.to_string_lossy().into_owned())
            .collect()
    }
}

/// Runs `imify datasets upload . --task <TASK>` in the dataset root, with the token and the
/// URL of `server` when one is given.
fn upload(dataset: &PickCube, server: Option<&MockServer>, extra_args: &[&str]) -> Output {
    upload_task(dataset, server, TASK, extra_args)
}

/// Runs `imify datasets upload . --task <task>` like [`upload`]. `HOME` is a directory next to
/// the dataset and `XDG_STATE_HOME` is unset, so the journal is in [`PickCube::journal_dir`].
fn upload_task(
    dataset: &PickCube,
    server: Option<&MockServer>,
    task: &str,
    extra_args: &[&str],
) -> Output {
    let mut command = imify_command();
    if let Some(server) = server {
        command
            .env("IMIFY_TOKEN", TOKEN)
            .env("IMIFY_API_URL", server.uri());
    }
    command
        .env("HOME", &dataset.home)
        .env_remove("XDG_STATE_HOME")
        .current_dir(&dataset.root)
        .args(["datasets", "upload", ".", "--task", task])
        .args(extra_args)
        .assert()
        .get_output()
        .clone()
}

/// The file a transfer event line is about: the JSON `path`, or the second word of a text line.
fn transfer_path(line: &str) -> &str {
    match line.split_once("\"path\":\"") {
        Some((_, rest)) => rest.split('"').next().unwrap_or_default(),
        None => line.split(' ').nth(1).unwrap_or_default(),
    }
}

/// `text` with the lines from the first line containing `first` up to, not including, the line
/// containing `end` ordered by file, so that the output of concurrent transfers compares
/// stably. The events of one file keep their order.
fn with_sorted_transfers(text: &str, first: &str, end: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    let position = |marker: &str| {
        lines
            .iter()
            .position(|line| line.contains(marker))
            .unwrap_or(lines.len())
    };
    let (start, stop) = (position(first), position(end));
    lines[start..stop.max(start)].sort_by_key(|line| transfer_path(line));
    let mut sorted = lines.join("\n");
    sorted.push('\n');
    sorted
}

fn json_lines(stdout: &[u8]) -> serde_json::Result<Vec<serde_json::Value>> {
    text(stdout).lines().map(serde_json::from_str).collect()
}

fn types(events: &[serde_json::Value]) -> Vec<&str> {
    events
        .iter()
        .map(|event| event["type"].as_str().unwrap_or_default())
        .collect()
}

/// The `code` of the error envelope on stderr.
fn error_code(output: &Output) -> Option<String> {
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).ok()?;
    envelope["error"]["code"].as_str().map(str::to_owned)
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_json_writes_the_event_stream_and_the_session() {
    let server = ingest_api().await;
    let dataset = pick_cube(&[]).unwrap();

    let output = upload(&dataset, Some(&server), &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    insta::assert_snapshot!(
        "datasets_upload_json",
        with_sorted_transfers(&text(&output.stdout), "\"file_", "\"completing\"")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_json_reports_each_file_as_started_then_finished() {
    let server = ingest_api().await;
    let dataset = pick_cube(&[]).unwrap();

    let output = upload(&dataset, Some(&server), &["--json"]);

    let events = json_lines(&output.stdout).unwrap();
    for file in &dataset.files {
        let path = file.to_str().unwrap();
        let kinds: Vec<&str> = events
            .iter()
            .filter(|event| event["path"] == path)
            .map(|event| event["type"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["file_started", "file_finished"], "{path}");
    }
    assert_eq!(
        types(&events)[..3],
        ["validated", "planned", "session_created"]
    );
    assert_eq!(types(&events)[events.len() - 2..], ["completing", "result"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_creates_the_session_sends_meta_info_json_first_then_every_file_then_completes()
 {
    let server = ingest_api().await;
    let dataset = pick_cube(&[]).unwrap();

    let output = upload(&dataset, Some(&server), &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let requests = server.received_requests().await.unwrap();
    let (first, rest) = requests.split_first().unwrap();
    let (last, puts) = rest.split_last().unwrap();
    assert_eq!(
        (first.method.as_str(), first.url.path()),
        ("POST", SESSIONS)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&first.body).unwrap(),
        json!({"name": "pick-cube", "task_description": TASK})
    );
    assert_eq!((last.method.as_str(), last.url.path()), ("POST", COMPLETE));
    let mut sent: Vec<&str> = puts
        .iter()
        .map(|request| request.url.path().strip_prefix(FILES).unwrap())
        .collect();
    assert_eq!(sent[0], "meta/info.json");
    sent.sort_unstable();
    let mut planned: Vec<&str> = dataset
        .files
        .iter()
        .map(|file| file.to_str().unwrap())
        .collect();
    planned.sort_unstable();
    assert_eq!(sent, planned);
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_writes_events_to_stderr_and_the_session_to_stdout() {
    let server = ingest_api().await;
    let dataset = pick_cube(&[]).unwrap();

    let output = upload(&dataset, Some(&server), &[]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    insta::assert_snapshot!("datasets_upload_human", text(&output.stdout));
    insta::assert_snapshot!(
        "datasets_upload_human_stderr",
        with_sorted_transfers(&text(&output.stderr), "Uploading ", "Completing")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_quiet_json_writes_only_the_result_line() {
    let server = ingest_api().await;
    let dataset = pick_cube(&[]).unwrap();

    let output = upload(&dataset, Some(&server), &["--quiet", "--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(types(&json_lines(&output.stdout).unwrap()), ["result"]);
}

#[test]
fn datasets_upload_dry_run_needs_no_token_and_no_api_url() {
    let dataset = pick_cube(&[]).unwrap();

    let output = upload(&dataset, None, &["--dry-run", "--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    insta::assert_snapshot!("datasets_upload_dry_run_json", text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_dry_run_sends_no_request() {
    let server = ingest_api().await;
    let dataset = pick_cube(&[]).unwrap();

    let output = upload(&dataset, Some(&server), &["--dry-run"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(server.received_requests().await.unwrap().is_empty());
    insta::assert_snapshot!("datasets_upload_dry_run_human", text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_of_a_dataset_with_an_error_fails_with_dataset_invalid_and_sends_no_request()
 {
    let server = ingest_api().await;
    let dataset = pick_cube(&[Defect::MissingDataFile]).unwrap();

    let output = upload(&dataset, Some(&server), &["--json"]);

    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output).as_deref(), Some("dataset_invalid"));
    let events = json_lines(&output.stdout).unwrap();
    assert_eq!(types(&events), ["validated"]);
    assert_eq!(events[0]["errors"], 1);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_of_a_dataset_with_warnings_only_uploads_and_lists_the_warnings() {
    let server = ingest_api().await;
    let dataset = pick_cube(&[Defect::StrayFile("notes.txt".to_owned())]).unwrap();

    let output = upload(&dataset, Some(&server), &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let events = json_lines(&output.stdout).unwrap();
    assert_eq!(events[0]["type"], "validated");
    assert_eq!(events[0]["warnings"], 1);
    assert_eq!(events[0]["findings"][0]["code"], "unexpected_entry");
    assert_eq!(events[1]["skipped"], json!(["notes.txt"]));
    assert_eq!(
        events[1]["skipped_entries"],
        json!([{"path": "notes.txt", "reason": "outside_dataset"}])
    );
    assert_eq!(events.last().unwrap()["type"], "result");
}

/// Adds `videos/extra.mp4` as a symbolic link to a file outside the dataset, kept alive by
/// the returned directory.
fn link_outside(dataset: &PickCube) -> std::io::Result<tempfile::TempDir> {
    let outside = tempfile::tempdir()?;
    std::fs::write(
        outside.path().join("private.bin"),
        b"not part of the dataset",
    )?;
    std::os::unix::fs::symlink(
        outside.path().join("private.bin"),
        dataset.root.join("videos/extra.mp4"),
    )?;
    Ok(outside)
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_skips_a_symbolic_link_and_names_it_in_the_planned_event() {
    let server = ingest_api().await;
    let dataset = pick_cube(&[]).unwrap();
    let _outside = link_outside(&dataset).unwrap();

    let output = upload(&dataset, Some(&server), &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let events = json_lines(&output.stdout).unwrap();
    assert_eq!(
        events[1]["skipped_entries"],
        json!([{"path": "videos/extra.mp4", "reason": "symbolic_link"}])
    );
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|request| !request.url.path().contains("extra.mp4"))
    );
}

#[test]
fn datasets_upload_dry_run_names_a_skipped_symbolic_link() {
    let dataset = pick_cube(&[]).unwrap();
    let _outside = link_outside(&dataset).unwrap();

    let output = upload(&dataset, None, &["--dry-run"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr)
            .contains("Skipped videos/extra.mp4: symbolic links are not uploaded.\n"),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_rejected_by_complete_fails_with_dataset_invalid_and_the_api_message() {
    let server = ingest_api_completing_with(
        ResponseTemplate::new(400).set_body_json(json!({"detail": "No episodes found in data/"})),
    )
    .await;
    let dataset = pick_cube(&[]).unwrap();

    let output = upload(&dataset, Some(&server), &["--json"]);

    assert_eq!(output.status.code(), Some(5));
    insta::assert_snapshot!("datasets_upload_rejected_json", text(&output.stderr));
}

#[test]
fn datasets_upload_without_task_is_a_usage_error() {
    let output = imify(&["datasets", "upload", ".", "--json"], &[]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn datasets_upload_with_an_empty_or_blank_task_is_a_usage_error() {
    for task in ["", "  "] {
        let output = imify(&["datasets", "upload", ".", "--task", task, "--json"], &[]);

        assert_eq!(output.status.code(), Some(2), "{task:?}");
        assert_eq!(error_code(&output).as_deref(), Some("invalid_argument"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_without_imify_token_fails_with_auth_required_before_reading_the_dataset() {
    let server = ingest_api().await;

    let output = imify(
        &[
            "datasets",
            "upload",
            "no-such-dataset",
            "--task",
            TASK,
            "--json",
        ],
        &[("IMIFY_API_URL", &server.uri())],
    );

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(error_code(&output).as_deref(), Some("auth_required"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn datasets_upload_of_a_missing_directory_is_a_usage_error() {
    let dataset = pick_cube(&[]).unwrap();

    let output = imify_command()
        .current_dir(&dataset.root)
        .args(["datasets", "upload", "no-such-dataset", "--task", TASK])
        .args(["--dry-run", "--json"])
        .assert()
        .get_output()
        .clone();

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(error_code(&output).as_deref(), Some("invalid_argument"));
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_retries_a_file_answered_with_503_and_reports_the_retry() {
    let server = ingest_api().await;
    let dataset = pick_cube(&[]).unwrap();
    let retried = dataset
        .files
        .iter()
        .find(|file| file.starts_with("data"))
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    Mock::given(method("PUT"))
        .and(path(format!("{FILES}{retried}")))
        .respond_with(
            ResponseTemplate::new(503).set_body_json(json!({"detail": "Service Unavailable"})),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;

    let output = upload(&dataset, Some(&server), &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let events = json_lines(&output.stdout).unwrap();
    let retry_events: Vec<&serde_json::Value> = events
        .iter()
        .filter(|event| event["type"] == "file_retrying")
        .collect();
    assert_eq!(retry_events.len(), 1);
    assert_eq!(retry_events[0]["path"], retried.as_str());
    assert_eq!(retry_events[0]["attempt"], 1);
}

/// The files the requests store in session `3f2c9a`, in request order.
fn stored_paths(requests: &[Request]) -> Vec<String> {
    requests
        .iter()
        .filter(|request| request.method.as_str() == "PUT")
        .filter_map(|request| request.url.path().strip_prefix(FILES))
        .map(str::to_owned)
        .collect()
}

fn session_creations(requests: &[Request]) -> usize {
    requests
        .iter()
        .filter(|request| request.method.as_str() == "POST" && request.url.path() == SESSIONS)
        .count()
}

/// The `path` of every event of type `kind`.
fn event_paths(events: &[serde_json::Value], kind: &str) -> BTreeSet<String> {
    events
        .iter()
        .filter(|event| event["type"] == kind)
        .filter_map(|event| event["path"].as_str())
        .map(str::to_owned)
        .collect()
}

/// Runs an upload that stores every file and is rejected at `complete`, which leaves a
/// journal of every file. Returns the server with its requests cleared and `complete`
/// answering with the completed session.
async fn upload_rejected_at_complete(dataset: &PickCube) -> MockServer {
    let server = ingest_api_completing_with(
        ResponseTemplate::new(400).set_body_json(json!({"detail": "No episodes found in data/"})),
    )
    .await;
    let output = upload(dataset, Some(&server), &["--json"]);
    assert_eq!(output.status.code(), Some(5), "{}", text(&output.stderr));
    server.reset().await;
    mount_ingest_api(&server, completed()).await;
    server
}

/// The data file [`upload_failing_at_a_data_file`] rejects.
const REJECTED: &str = "data/chunk-000/file-000.parquet";

/// Runs an upload whose first data file is rejected, which leaves a journal of the files
/// stored before. Returns the server, its `complete` and file mocks unchanged, and the files
/// the first run reported as stored.
async fn upload_failing_at_a_data_file(
    dataset: &PickCube,
) -> serde_json::Result<(MockServer, BTreeSet<String>)> {
    let server = ingest_api().await;
    Mock::given(method("PUT"))
        .and(path(format!("{FILES}{REJECTED}")))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({"detail": "Unsupported file"})),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    let output = upload(dataset, Some(&server), &["--json"]);
    assert_eq!(output.status.code(), Some(5), "{}", text(&output.stderr));
    let stored = event_paths(&json_lines(&output.stdout)?, "file_finished");
    assert!(stored.contains("meta/info.json"), "{stored:?}");
    assert!(!stored.contains(REJECTED), "{stored:?}");
    Ok((server, stored))
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_after_a_failed_upload_resumes_the_session_and_sends_only_the_remaining_files()
 {
    let dataset = pick_cube(&[]).unwrap();
    let (server, stored) = upload_failing_at_a_data_file(&dataset).await.unwrap();
    let first_run = server.received_requests().await.unwrap().len();

    let output = upload(&dataset, Some(&server), &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let requests = server.received_requests().await.unwrap()[first_run..].to_vec();
    let sent: BTreeSet<String> = stored_paths(&requests).into_iter().collect();
    let remaining: BTreeSet<String> = dataset
        .upload_paths()
        .difference(&stored)
        .cloned()
        .collect();
    assert_eq!(sent, remaining);
    assert_eq!(session_creations(&requests), 0);
    let events = json_lines(&output.stdout).unwrap();
    let resumed = events
        .iter()
        .find(|event| event["type"] == "session_resumed")
        .unwrap();
    assert_eq!(resumed["session_id"], SESSION_ID);
    assert_eq!(resumed["files_done"], stored.len());
    let result = events.last().unwrap();
    assert_eq!(
        (&result["session_id"], &result["resumed"]),
        (&json!(SESSION_ID), &json!(true))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_json_of_a_resumed_upload_reports_the_files_already_stored() {
    let dataset = pick_cube(&[]).unwrap();
    let server = upload_rejected_at_complete(&dataset).await;

    let output = upload(&dataset, Some(&server), &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    insta::assert_snapshot!("datasets_upload_resumed_json", text(&output.stdout));
    assert_eq!(
        stored_paths(&server.received_requests().await.unwrap()),
        Vec::<String>::new()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_removes_the_journal_once_the_session_is_complete() {
    let dataset = pick_cube(&[]).unwrap();
    let (server, _) = upload_failing_at_a_data_file(&dataset).await.unwrap();
    let journals_before = std::fs::read_dir(dataset.journal_dir()).unwrap().count();

    let output = upload(&dataset, Some(&server), &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(journals_before, 1);
    assert_eq!(std::fs::read_dir(dataset.journal_dir()).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_resume_without_an_interrupted_upload_fails_with_not_found_and_sends_no_request()
 {
    let server = ingest_api().await;
    let dataset = pick_cube(&[]).unwrap();

    let output = upload(&dataset, Some(&server), &["--resume", "--json"]);

    assert_eq!(output.status.code(), Some(4), "{}", text(&output.stderr));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "not_found");
    assert_eq!(envelope["error"]["hint"], "Run without `--resume`.");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_with_another_task_than_the_interrupted_upload_fails_with_conflict() {
    let dataset = pick_cube(&[]).unwrap();
    let (server, _) = upload_failing_at_a_data_file(&dataset).await.unwrap();
    let first_run = server.received_requests().await.unwrap().len();

    let output = upload_task(&dataset, Some(&server), "Stack the blocks", &["--json"]);

    assert_eq!(output.status.code(), Some(6), "{}", text(&output.stderr));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "conflict");
    assert_eq!(envelope["error"]["hint"], "Run again with `--restart`.");
    assert_eq!(server.received_requests().await.unwrap().len(), first_run);
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_restart_creates_a_new_session_and_sends_every_file() {
    let dataset = pick_cube(&[]).unwrap();
    let (server, _) = upload_failing_at_a_data_file(&dataset).await.unwrap();
    let first_run = server.received_requests().await.unwrap().len();

    let output = upload(&dataset, Some(&server), &["--restart", "--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let requests = server.received_requests().await.unwrap()[first_run..].to_vec();
    assert_eq!(session_creations(&requests), 1);
    let sent: BTreeSet<String> = stored_paths(&requests).into_iter().collect();
    assert_eq!(sent, dataset.upload_paths());
    let events = json_lines(&output.stdout).unwrap();
    assert!(types(&events).contains(&"session_created"), "{events:?}");
    assert!(!types(&events).contains(&"session_resumed"), "{events:?}");
    assert_eq!(events.last().unwrap()["resumed"], false);
}

#[test]
fn datasets_upload_with_resume_and_restart_is_a_usage_error() {
    let output = imify(
        &[
            "datasets",
            "upload",
            ".",
            "--task",
            TASK,
            "--resume",
            "--restart",
            "--json",
        ],
        &[],
    );

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(error_code(&output).as_deref(), Some("invalid_argument"));
}

#[test]
fn datasets_upload_help_shows_examples() {
    let output = imify(&["datasets", "upload", "--help"], &[]);

    assert!(output.status.success());
    assert!(text(&output.stdout).contains(
        "Examples:\n  imify datasets upload ./pick-cube --task \"Pick up the red cube\"\n"
    ));
}

/// An unsigned JWT for `user-1234` that expires at 2100-01-01T00:00:00Z.
const VALID_JWT: &str =
    "eyJhbGciOiJub25lIn0.eyJzdWIiOiJ1c2VyLTEyMzQiLCJleHAiOjQxMDI0NDQ4MDB9.c2lnbmF0dXJl";
/// An unsigned JWT for `user-1234` that expired at 2001-09-09T01:46:40Z.
const EXPIRED_JWT: &str =
    "eyJhbGciOiJub25lIn0.eyJzdWIiOiJ1c2VyLTEyMzQiLCJleHAiOjEwMDAwMDAwMDB9.c2lnbmF0dXJl";

fn health_ok() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"status": "ok"}))
}

fn ready(status: u16, storage: &str) -> ResponseTemplate {
    let overall = if storage == "ok" { "ok" } else { "degraded" };
    ResponseTemplate::new(status)
        .set_body_json(json!({"status": overall, "db": "ok", "storage": storage}))
}

fn me_ok() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(user_body())
}

/// The `config.json` of a host whose identity provider is `https://login.example.com`.
fn config_json_ok() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "authority": "https://login.example.com",
        "clientId": "client-1",
    }))
}

/// An API that answers the four requests doctor sends.
async fn doctor_api(
    health: ResponseTemplate,
    readiness: ResponseTemplate,
    me: ResponseTemplate,
) -> MockServer {
    let server = MockServer::start().await;
    for (route, response) in [
        ("/api/health", health),
        ("/api/health/ready", readiness),
        ("/api/v1/me", me),
        ("/config.json", config_json_ok()),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(response)
            .mount(&server)
            .await;
    }
    server
}

async fn healthy_api() -> MockServer {
    doctor_api(health_ok(), ready(200, "ok"), me_ok()).await
}

fn doctor(api_url: Option<&str>, token: Option<&str>, extra_args: &[&str]) -> Output {
    let mut args = vec!["doctor"];
    args.extend_from_slice(extra_args);
    let mut env = Vec::new();
    if let Some(api_url) = api_url {
        env.push(("IMIFY_API_URL", api_url));
    }
    if let Some(token) = token {
        env.push(("IMIFY_TOKEN", token));
    }
    imify(&args, &env)
}

/// The configuration directory of [`imify_command`].
fn no_login_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("no-login")
}

/// Replaces the parts of a doctor report that vary between runs, machines and releases: the
/// mock server's port, the configuration directory, the latency, the platform and the CLI
/// version.
fn normalize_doctor_text(text: &str, server_uri: &str) -> String {
    let platform = format!("({} {})", std::env::consts::OS, std::env::consts::ARCH);
    let version = format!("imify-cli {} (", env!("CARGO_PKG_VERSION"));
    let mut normalized = text
        .replace(server_uri, "http://127.0.0.1:<port>")
        .replace(&no_login_dir().display().to_string(), "<config dir>")
        .replace(&version, "imify-cli <version> (")
        .replace(&platform, "(<os> <arch>)");
    let label = "Reachable in ";
    if let Some(start) = normalized.find(label).map(|index| index + label.len()) {
        let digits = normalized[start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .count();
        normalized.replace_range(start..start + digits, "<n>");
    }
    normalized
}

/// The JSON report with `latency_ms` and `clock_skew_ms` set to 0 and `cli_version` set to
/// `<version>` when present, and the varying text replaced as in [`normalize_doctor_text`].
/// `null` when stdout is not JSON.
fn normalize_doctor_json(stdout: &[u8], server_uri: &str) -> String {
    let mut report: serde_json::Value = serde_json::from_slice(stdout).unwrap_or_default();
    for field in ["latency_ms", "clock_skew_ms"] {
        if !report[field].is_null() {
            report[field] = json!(0);
        }
    }
    if !report["cli_version"].is_null() {
        report["cli_version"] = json!("<version>");
    }
    report["platform"] = json!({"os": "<os>", "arch": "<arch>"});
    normalize_doctor_text(&report.to_string(), server_uri)
}

/// `(name, status)` of every check in a JSON report; empty when stdout is not a report.
fn doctor_statuses(stdout: &[u8]) -> Vec<(String, String)> {
    let report: serde_json::Value = serde_json::from_slice(stdout).unwrap_or_default();
    let field =
        |check: &serde_json::Value, name: &str| check[name].as_str().unwrap_or_default().to_owned();
    report["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|check| (field(check, "name"), field(check, "status")))
        .collect()
}

fn statuses(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(name, status)| ((*name).to_owned(), (*status).to_owned()))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_every_check_passing_prints_the_report_and_exits_0() {
    let server = healthy_api().await;

    let output = doctor(Some(&server.uri()), Some(VALID_JWT), &[]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(text(&output.stderr), "");
    insta::assert_snapshot!(
        "doctor_ok_human",
        normalize_doctor_text(&text(&output.stdout), &server.uri())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_json_with_every_check_passing_prints_one_document() {
    let server = healthy_api().await;

    let output = doctor(Some(&server.uri()), Some(VALID_JWT), &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    insta::assert_snapshot!(
        "doctor_ok_json",
        normalize_doctor_json(&output.stdout, &server.uri())
    );
}

#[test]
fn doctor_with_an_invalid_api_url_skips_the_api_checks_and_fails_with_invalid_argument() {
    let output = doctor(Some("ftp://api.example.com"), Some(VALID_JWT), &["--json"]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(error_code(&output).as_deref(), Some("invalid_argument"));
    assert_eq!(
        doctor_statuses(&output.stdout),
        statuses(&[
            ("api_url", "fail"),
            ("health", "skip"),
            ("readiness", "skip"),
            ("clock", "skip"),
            ("token", "ok"),
            ("token_expiry", "ok"),
            ("account", "skip"),
            ("oidc", "skip"),
        ])
    );
    insta::assert_snapshot!(
        "doctor_invalid_api_url_json",
        normalize_doctor_json(&output.stdout, "http://unused")
    );
}

#[test]
fn doctor_against_a_host_that_is_not_listening_fails_with_network() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();

    let output = doctor(
        Some(&format!("http://127.0.0.1:{port}")),
        Some(TOKEN),
        &["--json"],
    );

    assert_eq!(output.status.code(), Some(7));
    assert_eq!(error_code(&output).as_deref(), Some("network"));
    assert_eq!(
        doctor_statuses(&output.stdout),
        statuses(&[
            ("api_url", "ok"),
            ("health", "fail"),
            ("readiness", "skip"),
            ("clock", "skip"),
            ("token", "ok"),
            ("token_expiry", "skip"),
            ("account", "skip"),
            ("oidc", "skip"),
        ])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_health_answering_503_fails_with_server() {
    let server = doctor_api(ResponseTemplate::new(503), ready(200, "ok"), me_ok()).await;

    let output = doctor(Some(&server.uri()), Some(TOKEN), &["--json"]);

    assert_eq!(output.status.code(), Some(7));
    assert_eq!(error_code(&output).as_deref(), Some("server"));
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_unready_storage_fails_the_readiness_check_naming_storage() {
    let server = doctor_api(health_ok(), ready(503, "unreachable"), me_ok()).await;

    let output = doctor(Some(&server.uri()), Some(TOKEN), &["--json"]);

    assert_eq!(output.status.code(), Some(7));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let readiness = &report["checks"][2];
    assert_eq!(
        (&readiness["name"], &readiness["status"], &readiness["code"]),
        (&json!("readiness"), &json!("fail"), &json!("server"))
    );
    assert!(
        readiness["message"]
            .as_str()
            .unwrap()
            .contains("storage unreachable")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_a_server_clock_far_behind_warns_and_exits_0() {
    let server = doctor_api(
        health_ok().insert_header("date", "Sun, 06 Nov 1994 08:49:37 GMT"),
        ready(200, "ok"),
        me_ok(),
    )
    .await;

    let output = doctor(Some(&server.uri()), Some(TOKEN), &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["checks"][3]["status"], "warn");
    assert!(report["clock_skew_ms"].as_i64().unwrap() < -60_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_without_imify_token_fails_with_auth_required_and_does_not_call_me() {
    let server = healthy_api().await;

    let output = doctor(Some(&server.uri()), None, &["--json"]);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(error_code(&output).as_deref(), Some("auth_required"));
    let statuses = doctor_statuses(&output.stdout);
    assert_eq!(statuses[4], ("token".to_owned(), "fail".to_owned()));
    assert_eq!(statuses[6], ("account".to_owned(), "skip".to_owned()));
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|request| request.url.path() != "/api/v1/me")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_an_expired_jwt_fails_with_auth_rejected_and_still_checks_the_account() {
    let server = healthy_api().await;

    let output = doctor(Some(&server.uri()), Some(EXPIRED_JWT), &["--json"]);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(error_code(&output).as_deref(), Some("auth_rejected"));
    let statuses = doctor_statuses(&output.stdout);
    assert_eq!(statuses[5], ("token_expiry".to_owned(), "fail".to_owned()));
    assert_eq!(statuses[6], ("account".to_owned(), "ok".to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_a_token_the_api_rejects_fails_with_auth_rejected() {
    let server = doctor_api(
        health_ok(),
        ready(200, "ok"),
        ResponseTemplate::new(401).set_body_json(json!({"detail": "Invalid token"})),
    )
    .await;

    let output = doctor(Some(&server.uri()), Some(TOKEN), &["--json"]);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(error_code(&output).as_deref(), Some("auth_rejected"));
    assert_eq!(
        doctor_statuses(&output.stdout)[6],
        ("account".to_owned(), "fail".to_owned())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_several_failures_names_them_all_and_carries_the_first_code() {
    let server = doctor_api(ResponseTemplate::new(503), ready(200, "ok"), me_ok()).await;

    let output = doctor(Some(&server.uri()), None, &["--json"]);

    assert_eq!(output.status.code(), Some(7));
    insta::assert_snapshot!("doctor_failed_envelope_json", text(&output.stderr));
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_a_failing_check_writes_the_report_to_stdout_and_the_error_to_stderr() {
    let server = healthy_api().await;

    let output = doctor(Some(&server.uri()), None, &[]);

    assert_eq!(output.status.code(), Some(3));
    insta::assert_snapshot!(
        "doctor_token_missing_human",
        normalize_doctor_text(&text(&output.stdout), &server.uri())
    );
    insta::assert_snapshot!("doctor_token_missing_human_stderr", text(&output.stderr));
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_trace_output_and_json_report_contain_no_part_of_the_token() {
    let server = healthy_api().await;

    let traced = doctor(Some(&server.uri()), Some(VALID_JWT), &["-vv"]);
    let json = doctor(Some(&server.uri()), Some(VALID_JWT), &["--json"]);

    assert_eq!(traced.status.code(), Some(0), "{}", text(&traced.stderr));
    for output in [&traced, &json] {
        let all = format!("{}{}", text(&output.stdout), text(&output.stderr));
        for part in VALID_JWT.split('.') {
            assert!(!all.contains(part), "{all}");
        }
    }
}

const UNSET_TOKEN_HINT: &str =
    "Unset `IMIFY_TOKEN` to use the stored login, or set it to a valid, unexpired token.";

/// A configuration directory whose credentials file holds a login for `api_url`.
fn config_dir_with_login(api_url: &str) -> std::io::Result<tempfile::TempDir> {
    use std::os::unix::fs::PermissionsExt as _;

    let config_dir = tempfile::tempdir()?;
    let issuer = "http://127.0.0.1:1";
    let file = json!({
        api_url: {
            "version": 1,
            "issuer": issuer,
            "client_id": "client-1",
            "token_endpoint": format!("{issuer}/oauth2/token"),
            "revocation_endpoint": format!("{issuer}/oauth2/revoke"),
            "id_token": VALID_JWT,
            "id_token_expires_at": 4_102_444_800_u64,
            "refresh_token": "rt_5b1e-secret",
        }
    });
    let path = config_dir.path().join("credentials.json");
    std::fs::write(&path, file.to_string())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(config_dir)
}

/// `imify doctor --json` for `server` with `IMIFY_TOKEN` set to `token` and the login stored
/// in `config_dir`.
fn doctor_with_stored_login(
    server: &MockServer,
    token: &str,
    config_dir: &tempfile::TempDir,
) -> Output {
    imify(
        &["doctor", "--json"],
        &[
            ("IMIFY_API_URL", &server.uri()),
            ("IMIFY_TOKEN", token),
            (
                "IMIFY_CONFIG_DIR",
                config_dir.path().to_str().unwrap_or_default(),
            ),
        ],
    )
}

/// The hint of the check named `name` in a JSON report.
fn check_hint(stdout: &[u8], name: &str) -> serde_json::Value {
    let report: serde_json::Value = serde_json::from_slice(stdout).unwrap_or_default();
    report["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|check| check["name"] == name)
        .map(|check| check["hint"].clone())
        .unwrap_or_default()
}

fn error_hint(output: &Output) -> serde_json::Value {
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap_or_default();
    envelope["error"]["hint"].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_a_rejected_imify_token_and_a_stored_login_hints_at_unsetting_it() {
    let server = doctor_api(
        health_ok(),
        ready(200, "ok"),
        ResponseTemplate::new(401).set_body_json(json!({"detail": "Invalid token"})),
    )
    .await;
    let config_dir = config_dir_with_login(&server.uri()).unwrap();

    let output = doctor_with_stored_login(&server, TOKEN, &config_dir);

    assert_eq!(error_code(&output).as_deref(), Some("auth_rejected"));
    assert_eq!(
        (check_hint(&output.stdout, "account"), error_hint(&output)),
        (json!(UNSET_TOKEN_HINT), json!(UNSET_TOKEN_HINT))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_an_expired_imify_token_and_a_stored_login_hints_at_unsetting_it() {
    let server = healthy_api().await;
    let config_dir = config_dir_with_login(&server.uri()).unwrap();

    let output = doctor_with_stored_login(&server, EXPIRED_JWT, &config_dir);

    assert_eq!(
        check_hint(&output.stdout, "token_expiry"),
        json!(UNSET_TOKEN_HINT)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_an_expired_imify_token_and_no_stored_login_hints_at_a_valid_token() {
    let server = healthy_api().await;

    let output = doctor(Some(&server.uri()), Some(EXPIRED_JWT), &["--json"]);

    assert_eq!(
        check_hint(&output.stdout, "token_expiry"),
        json!("Set `IMIFY_TOKEN` to a valid, unexpired token.")
    );
}

/// The check named `name` in a JSON report; `null` when it is missing.
fn check_named(stdout: &[u8], name: &str) -> serde_json::Value {
    let report: serde_json::Value = serde_json::from_slice(stdout).unwrap_or_default();
    report["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|check| check["name"] == name)
        .cloned()
        .unwrap_or_default()
}

async fn requests_to(server: &MockServer, request_path: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.url.path() == request_path)
        .count()
}

#[test]
fn doctor_with_an_http_api_url_on_a_remote_host_fails_api_url_with_invalid_argument() {
    let output = doctor(Some("http://api.example.com"), Some(VALID_JWT), &["--json"]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        (
            &check_named(&output.stdout, "api_url")["status"],
            &check_named(&output.stdout, "api_url")["code"]
        ),
        (&json!("fail"), &json!("invalid_argument"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_reports_the_https_issuer_the_host_publishes_and_passes_oidc() {
    let server = healthy_api().await;

    let output = doctor(Some(&server.uri()), Some(VALID_JWT), &["--json"]);

    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        (
            &check_named(&output.stdout, "oidc")["status"],
            &report["oidc"]
        ),
        (
            &json!("ok"),
            &json!({
                "issuer": "https://login.example.com",
                "client_id": "client-1",
                "issuer_https": true,
            })
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_both_oidc_overrides_reports_them_without_requesting_config_json() {
    let server = healthy_api().await;

    let output = imify(
        &["doctor", "--json"],
        &[
            ("IMIFY_API_URL", &server.uri()),
            ("IMIFY_TOKEN", VALID_JWT),
            ("IMIFY_OIDC_ISSUER", "https://sso.example.org"),
            ("IMIFY_OIDC_CLIENT_ID", "client-override"),
        ],
    );

    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        (&report["oidc"]["issuer"], &report["oidc"]["client_id"]),
        (&json!("https://sso.example.org"), &json!("client-override"))
    );
    assert_eq!(requests_to(&server, "/config.json").await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_against_a_host_without_config_json_warns_on_oidc_and_exits_0() {
    let server = MockServer::start().await;
    for (route, response) in [
        ("/api/health", health_ok()),
        ("/api/health/ready", ready(200, "ok")),
        ("/api/v1/me", me_ok()),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(response)
            .mount(&server)
            .await;
    }

    let output = doctor(Some(&server.uri()), Some(VALID_JWT), &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let oidc = check_named(&output.stdout, "oidc");
    assert_eq!(
        (&oidc["status"], &oidc["code"], &oidc["hint"]),
        (
            &json!("warn"),
            &json!(null),
            &json!("Set `IMIFY_OIDC_ISSUER` and `IMIFY_OIDC_CLIENT_ID`.")
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_failing_health_skips_oidc_without_requesting_config_json() {
    let server = doctor_api(ResponseTemplate::new(503), ready(200, "ok"), me_ok()).await;

    let output = doctor(Some(&server.uri()), Some(VALID_JWT), &["--json"]);

    assert_eq!(check_named(&output.stdout, "oidc")["status"], "skip");
    assert_eq!(requests_to(&server, "/config.json").await, 0);
}

#[test]
fn doctor_help_shows_the_api_url_and_profile_examples() {
    let output = imify(&["doctor", "--help"], &[]);

    assert!(output.status.success());
    assert!(text(&output.stdout).contains(
        "Examples:\n  imify doctor --api-url https://api.example.com\n  imify doctor --profile staging\n"
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_reports_an_api_url_from_the_config_file_with_its_source() {
    let server = healthy_api().await;
    let config_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!("api_url = \"{}\"\n", server.uri()),
    )
    .unwrap();

    let output = imify(
        &["doctor", "--json"],
        &[
            ("IMIFY_TOKEN", VALID_JWT),
            ("IMIFY_CONFIG_DIR", config_dir.path().to_str().unwrap()),
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        (&report["api_url"], &report["api_url_source"]),
        (&json!(server.uri()), &json!("config"))
    );
}

/// A configuration directory whose `config.toml` sets the top-level `api_url` to `top` and
/// the `api_url` of the profile `staging` to `staging`, and has the profile `empty` without
/// keys.
fn profiles_config_dir(top: &str, staging: &str) -> std::io::Result<tempfile::TempDir> {
    let config_dir = tempfile::tempdir()?;
    std::fs::write(
        config_dir.path().join("config.toml"),
        format!(
            "api_url = \"{top}\"\n\n[profiles.staging]\napi_url = \"{staging}\"\n\n[profiles.empty]\n"
        ),
    )?;
    Ok(config_dir)
}

/// `imify doctor` with `args`, a valid `IMIFY_TOKEN`, the config file in `config_dir` and the
/// variables in `env`.
fn doctor_in(config_dir: &tempfile::TempDir, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command_args = vec!["doctor"];
    command_args.extend_from_slice(args);
    let config_path = config_dir.path().to_str().unwrap_or_default();
    let mut command_env = vec![
        ("IMIFY_TOKEN", VALID_JWT),
        ("IMIFY_CONFIG_DIR", config_path),
    ];
    command_env.extend_from_slice(env);
    imify(&command_args, &command_env)
}

/// `active_profile`, `api_url_source` and `api_url_profile` of a JSON report.
fn profile_fields(stdout: &[u8]) -> (serde_json::Value, serde_json::Value, serde_json::Value) {
    let report: serde_json::Value = serde_json::from_slice(stdout).unwrap_or_default();
    (
        report["active_profile"].clone(),
        report["api_url_source"].clone(),
        report["api_url_profile"].clone(),
    )
}

/// The JSON report normalized as in [`normalize_doctor_json`], with `config_dir` replaced by
/// `<config dir>`.
fn normalize_doctor_json_in(
    stdout: &[u8],
    config_dir: &tempfile::TempDir,
    server_uri: &str,
) -> String {
    normalize_doctor_json(stdout, server_uri)
        .replace(&config_dir.path().display().to_string(), "<config dir>")
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_without_a_profile_reports_default_as_the_active_and_the_supplying_profile() {
    let server = healthy_api().await;
    let config_dir = profiles_config_dir(&server.uri(), "https://staging.example.com").unwrap();

    let output = doctor_in(&config_dir, &["--json"], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(
        profile_fields(&output.stdout),
        (json!("default"), json!("config"), json!("default"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_the_profile_flag_reports_the_profile_that_supplies_the_api_url() {
    let server = healthy_api().await;
    let config_dir = profiles_config_dir("https://top.example.com", &server.uri()).unwrap();

    let output = doctor_in(&config_dir, &["--json", "--profile", "staging"], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["api_url"], json!(server.uri()));
    assert_eq!(
        profile_fields(&output.stdout),
        (json!("staging"), json!("config"), json!("staging"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_imify_profile_reports_the_profile_as_active() {
    let server = healthy_api().await;
    let config_dir = profiles_config_dir("https://top.example.com", &server.uri()).unwrap();

    let output = doctor_in(&config_dir, &["--json"], &[("IMIFY_PROFILE", "staging")]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(profile_fields(&output.stdout).0, json!("staging"));
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_a_profile_without_api_url_reports_default_as_the_supplying_profile() {
    let server = healthy_api().await;
    let config_dir = profiles_config_dir(&server.uri(), "https://staging.example.com").unwrap();

    let output = doctor_in(&config_dir, &["--json", "--profile", "empty"], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["api_url"], json!(server.uri()));
    assert_eq!(
        profile_fields(&output.stdout),
        (json!("empty"), json!("config"), json!("default"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_a_profile_without_api_url_names_the_default_profile_in_the_api_url_check() {
    let server = healthy_api().await;
    let config_dir = profiles_config_dir(&server.uri(), "https://staging.example.com").unwrap();

    let output = doctor_in(&config_dir, &["--json", "--profile", "empty"], &[]);

    assert_eq!(
        check_named(&output.stdout, "api_url")["message"],
        json!(format!("{} (config.toml, profile default)", server.uri()))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_imify_api_url_and_a_profile_reports_no_supplying_profile() {
    let server = healthy_api().await;
    let config_dir =
        profiles_config_dir("https://top.example.com", "https://staging.example.com").unwrap();

    let output = doctor_in(
        &config_dir,
        &["--json", "--profile", "staging"],
        &[("IMIFY_API_URL", &server.uri())],
    );

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(
        profile_fields(&output.stdout),
        (json!("staging"), json!("env"), json!(null))
    );
}

#[test]
fn doctor_with_an_unknown_profile_fails_api_url_naming_it_and_skips_health() {
    let config_dir =
        profiles_config_dir("https://top.example.com", "https://staging.example.com").unwrap();

    let output = doctor_in(&config_dir, &["--json", "--profile", "nope"], &[]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(profile_fields(&output.stdout).0, json!("nope"));
    let api_url = check_named(&output.stdout, "api_url");
    assert_eq!(
        (&api_url["status"], &api_url["code"]),
        (&json!("fail"), &json!("invalid_argument"))
    );
    assert!(
        api_url["message"]
            .as_str()
            .unwrap_or_default()
            .contains("'nope'"),
        "{api_url}"
    );
    assert_eq!(check_named(&output.stdout, "health")["status"], "skip");
}

#[test]
fn doctor_with_an_invalid_profile_name_reports_no_active_profile_and_fails_api_url() {
    let config_dir =
        profiles_config_dir("https://top.example.com", "https://staging.example.com").unwrap();

    let output = doctor_in(&config_dir, &["--json", "--profile", "a.b"], &[]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(profile_fields(&output.stdout).0, json!(null));
    let api_url = check_named(&output.stdout, "api_url");
    assert_eq!(
        (&api_url["status"], &api_url["code"]),
        (&json!("fail"), &json!("invalid_argument"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_with_a_named_profile_prints_the_profile_line_and_the_supplying_profile() {
    let server = healthy_api().await;
    let config_dir = profiles_config_dir("https://top.example.com", &server.uri()).unwrap();

    let output = doctor_in(&config_dir, &["--profile", "staging"], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    insta::assert_snapshot!(
        "doctor_profile_human",
        normalize_doctor_text(&text(&output.stdout), &server.uri())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_json_with_a_named_profile_prints_one_document() {
    let server = healthy_api().await;
    let config_dir = profiles_config_dir("https://top.example.com", &server.uri()).unwrap();

    let output = doctor_in(&config_dir, &["--json", "--profile", "staging"], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    insta::assert_snapshot!(
        "doctor_profile_json",
        normalize_doctor_json_in(&output.stdout, &config_dir, &server.uri())
    );
}

/// External text that clears the screen, sets the window title, reverses the text after it
/// and starts a line that looks like a finding.
const HOSTILE: &str = "x\u{1b}[2J\u{1b}]0;pwned\u{7}\u{202e}\nerror  forged";

/// Asserts that `output` shows [`HOSTILE`] escaped: no raw control character other than a line
/// feed, no right-to-left override and no line that the external text started.
fn assert_escaped(output: &str) {
    let raw: Vec<char> = output
        .chars()
        .filter(|&c| (c.is_control() && c != '\n') || c == '\u{202e}')
        .collect();
    assert!(raw.is_empty(), "raw characters {raw:?} in {output:?}");
    assert!(output.contains("\\u{1b}[2J"), "{output}");
    assert!(
        !output.lines().any(|line| line.starts_with("error  forged")),
        "{output}"
    );
}

#[test]
fn datasets_validate_escapes_a_file_name_and_an_info_json_string() {
    let dataset = dataset(
        Format::V2_1,
        &[
            Defect::StrayFile(HOSTILE.to_owned()),
            Defect::CodebaseVersion(HOSTILE.to_owned()),
        ],
    )
    .unwrap();

    let output = validate(&dataset, &[]);

    assert_eq!(output.status.code(), Some(5));
    assert_escaped(&text(&output.stdout));
}

#[test]
fn datasets_validate_escapes_a_parquet_value() {
    let dataset = dataset(Format::V3_0, &[Defect::TextTimestamps(HOSTILE.to_owned())]).unwrap();

    let output = validate(&dataset, &[]);

    assert_eq!(output.status.code(), Some(5));
    assert_escaped(&text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_ls_escapes_a_server_dataset_name() {
    let body = json!([dataset_body(PICK_ID, HOSTILE, "lerobot_v3", "so100", 1, 1)]);
    let server = api_with_datasets(200, body).await;

    let output = datasets_ls(&server, &[]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_escaped(&text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn datasets_upload_escapes_session_fields_and_file_names() {
    let server = ingest_api().await;
    let dataset = pick_cube(&[Defect::StrayFile(HOSTILE.to_owned())]).unwrap();
    let mut session = completed_session_body();
    session["robot_type"] = json!(HOSTILE);
    Mock::given(method("POST"))
        .and(path(COMPLETE))
        .respond_with(ResponseTemplate::new(200).set_body_json(session))
        .with_priority(1)
        .mount(&server)
        .await;

    let output = upload(&dataset, Some(&server), &[]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_escaped(&text(&output.stdout));
    assert_escaped(&text(&output.stderr));
}

#[test]
fn datasets_upload_dry_run_escapes_the_session_name_taken_from_the_directory_name() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join(HOSTILE);
    DatasetBuilder::new(Format::V3_0).write_to(&root).unwrap();

    let output = imify_command()
        .env("HOME", parent.path().join("home"))
        .current_dir(&root)
        .args(["datasets", "upload", ".", "--task", TASK, "--dry-run"])
        .assert()
        .get_output()
        .clone();

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_escaped(&text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_escapes_readiness_and_account_fields() {
    let readiness = ResponseTemplate::new(200)
        .set_body_json(json!({"status": "ok", "db": HOSTILE, "storage": "ok"}));
    let me = ResponseTemplate::new(200)
        .set_body_json(json!({"subject": "user-1234", "name": HOSTILE, "role": "admin"}));
    let server = doctor_api(health_ok(), readiness, me).await;

    let output = doctor(Some(&server.uri()), Some(VALID_JWT), &[]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_escaped(&text(&output.stdout));
}
