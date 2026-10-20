use std::process::Output;

use assert_cmd::Command;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "tok_7f3a9c-secret";
const ACT_ID: &str = "run-3c9e1a7f";
const GROOT_ID: &str = "run-8b20d4e6";
const QUEUED_ID: &str = "run-5f61c0b2";
const PICK_DATASET: &str = "0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13";
const STACK_DATASET: &str = "5e2d7a90-1c3b-4e6f-8a7d-9b0c1d2e3f45";

/// The `imify` binary without the environment variables the CLI reads, authenticated by
/// `IMIFY_TOKEN` against `server`.
fn imify(server: &MockServer, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_imify"))
        .env_remove("IMIFY_OUTPUT")
        .env_remove("NO_COLOR")
        .env_remove("IMIFY_PROFILE")
        .env("IMIFY_CREDENTIAL_STORE", "file")
        .env(
            "IMIFY_CONFIG_DIR",
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-login"),
        )
        .env("IMIFY_NO_UPDATE_CHECK", "1")
        .env("IMIFY_TOKEN", TOKEN)
        .env("IMIFY_API_URL", server.uri())
        .args(args)
        .assert()
        .get_output()
        .clone()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The `code` of the JSON error envelope on stderr, or `None` when stderr holds none.
fn error_code(output: &Output) -> Option<String> {
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).ok()?;
    envelope["error"]["code"].as_str().map(str::to_owned)
}

fn act_body() -> serde_json::Value {
    json!({
        "id": ACT_ID,
        "version_id": "ver-41d2",
        "dataset_id": PICK_DATASET,
        "policy": "act",
        "backend": "sagemaker",
        "status": "succeeded",
        "phase": "done",
        "step": 20000,
        "total_steps": 20000,
        "error": null,
        "created_at": "2026-10-01T09:30:00Z",
        "started_at": "2026-10-01T09:31:12Z",
        "finished_at": "2026-10-01T11:02:40Z"
    })
}

fn groot_body() -> serde_json::Value {
    json!({
        "id": GROOT_ID,
        "version_id": "ver-77a0",
        "dataset_id": STACK_DATASET,
        "policy": "groot_n1.7",
        "backend": "local",
        "status": "running",
        "phase": "training",
        "step": 1200,
        "total_steps": 30000,
        "error": null,
        "created_at": "2026-10-02T08:00:00Z",
        "started_at": "2026-10-02T08:01:00Z",
        "finished_at": null
    })
}

fn queued_body() -> serde_json::Value {
    json!({
        "id": QUEUED_ID,
        "version_id": "ver-41d2",
        "dataset_id": PICK_DATASET,
        "policy": "pi0.5",
        "backend": "local",
        "status": "provisioning",
        "phase": "waiting",
        "step": 0,
        "total_steps": 10000,
        "error": null,
        "created_at": "2026-10-03T07:15:00Z",
        "started_at": null,
        "finished_at": null
    })
}

fn lineage_body() -> serde_json::Value {
    json!({
        "dataset_id": PICK_DATASET,
        "version_id": "ver-41d2",
        "version_name": "v3",
        "task_description": "Pick up the red cube",
        "embodiment_label": "so100",
        "embodiment_fingerprint": "so100-f3a9c1",
        "episode_count": 2,
        "episode_ids": ["ep-0001", "ep-0002"]
    })
}

fn checkpoints_body() -> serde_json::Value {
    json!([
        {"id": "010000", "size": 1_288_490_189_u64, "last_modified": "2026-10-01T10:15:02Z"},
        {"id": "020000", "size": 1_288_490_189_u64, "last_modified": "2026-10-01T11:01:55Z"}
    ])
}

async fn mount_json(server: &MockServer, route: &str, status: u16, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(server)
        .await;
}

async fn api_with_policies(body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    mount_json(&server, "/api/v1/training-runs", 200, body).await;
    server
}

fn policies_body() -> serde_json::Value {
    json!([act_body(), groot_body(), queued_body()])
}

/// The ids of the policies in the JSON array on stdout, or `None` when stdout holds none.
fn listed_ids(output: &Output) -> Option<Vec<String>> {
    let policies: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).ok()?;
    policies
        .iter()
        .map(|policy| policy["id"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_list_prints_the_policies_as_a_table() {
    let server = api_with_policies(policies_body()).await;

    let output = imify(&server, &["policies", "list"]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("policies_list_human", text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_list_json_prints_every_policy_in_the_policy_vocabulary() {
    let server = api_with_policies(policies_body()).await;

    let output = imify(&server, &["policies", "list", "--json"]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("policies_list_json", text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_list_without_policies_prints_no_policies() {
    let server = api_with_policies(json!([])).await;

    let output = imify(&server, &["policies", "list"]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "No policies.\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_list_json_without_policies_prints_an_empty_array() {
    let server = api_with_policies(json!([])).await;

    let output = imify(&server, &["policies", "list", "--json"]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("policies_list_empty_json", text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_list_state_keeps_only_policies_in_that_state() {
    let server = api_with_policies(policies_body()).await;

    let output = imify(
        &server,
        &["policies", "list", "--state", "training", "--json"],
    );

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(listed_ids(&output).unwrap(), [GROOT_ID]);
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_list_dataset_keeps_only_policies_trained_on_that_dataset() {
    let server = api_with_policies(policies_body()).await;

    let output = imify(
        &server,
        &["policies", "list", "--dataset", PICK_DATASET, "--json"],
    );

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(listed_ids(&output).unwrap(), [ACT_ID, QUEUED_ID]);
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_list_json_passes_an_unknown_state_through() {
    let server = api_with_policies(json!([queued_body()])).await;

    let output = imify(&server, &["policies", "list", "--json"]);

    let policies: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(policies[0]["state"], "provisioning");
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_list_with_an_unknown_state_filter_is_a_usage_error() {
    let server = api_with_policies(policies_body()).await;

    let output = imify(&server, &["policies", "list", "--state", "done", "--json"]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(error_code(&output).as_deref(), Some("invalid_argument"));
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_list_with_a_rejected_token_fails_with_auth_rejected() {
    let server = MockServer::start().await;
    mount_json(
        &server,
        "/api/v1/training-runs",
        401,
        json!({"detail": "Invalid token"}),
    )
    .await;

    let output = imify(&server, &["policies", "list", "--json"]);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(error_code(&output).as_deref(), Some("auth_rejected"));
}

/// A server that knows the policy `ACT_ID` with its lineage and `checkpoints`.
async fn api_with_act(checkpoints: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    let base = format!("/api/v1/training-runs/{ACT_ID}");
    mount_json(&server, &base, 200, act_body()).await;
    mount_json(&server, &format!("{base}/lineage"), 200, lineage_body()).await;
    mount_json(&server, &format!("{base}/checkpoints"), 200, checkpoints).await;
    server
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_show_prints_the_policy_its_lineage_and_its_checkpoints() {
    let server = api_with_act(checkpoints_body()).await;

    let output = imify(&server, &["policies", "show", ACT_ID]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("policies_show_human", text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_show_json_prints_the_policy_its_lineage_and_its_checkpoints() {
    let server = api_with_act(checkpoints_body()).await;

    let output = imify(&server, &["policies", "show", ACT_ID, "--json"]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("policies_show_json", text(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_show_without_checkpoints_says_so() {
    let server = api_with_act(json!([])).await;

    let output = imify(&server, &["policies", "show", ACT_ID]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        text(&output.stdout).ends_with("\nNo checkpoints.\n"),
        "{}",
        text(&output.stdout)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_show_of_an_unknown_policy_fails_with_not_found() {
    let server = MockServer::start().await;
    mount_json(
        &server,
        "/api/v1/training-runs/run-unknown",
        404,
        json!({"detail": "Training run not found"}),
    )
    .await;

    let output = imify(&server, &["policies", "show", "run-unknown", "--json"]);

    assert_eq!(output.status.code(), Some(4));
    assert_eq!(error_code(&output).as_deref(), Some("not_found"));
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_help_shows_examples() {
    let server = MockServer::start().await;

    let list = imify(&server, &["policies", "list", "--help"]);
    let show = imify(&server, &["policies", "show", "--help"]);

    assert!(text(&list.stdout).contains("Examples:\n  imify policies list\n"));
    assert!(text(&show.stdout).contains("Example:\n  imify policies show run-3c9e1a7f"));
}
