use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::auth::StaticToken;

const TOKEN: &str = "tok_7f3a9c-secret";
const ACT_ID: &str = "run-3c9e1a7f";
const GROOT_ID: &str = "run-8b20d4e6";
const PICK_DATASET: &str = "0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13";
const STACK_DATASET: &str = "5e2d7a90-1c3b-4e6f-8a7d-9b0c1d2e3f45";

fn client(base_url: &str) -> Client {
    Client::builder(base_url)
        .token_source(StaticToken::new(TOKEN))
        .retry_policy(crate::RetryPolicy::none())
        .build()
        .unwrap()
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
        "artifact_path": "policies/run-3c9e1a7f/",
        "artifact_uri": "s3://bucket/policies/run-3c9e1a7f/",
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
        "artifact_path": null,
        "error": null,
        "created_at": "2026-10-02T08:00:00Z",
        "started_at": "2026-10-02T08:01:00Z",
        "finished_at": null
    })
}

fn with(mut body: serde_json::Value, field: &str, value: serde_json::Value) -> serde_json::Value {
    body[field] = value;
    body
}

fn run_url(id: &str, suffix: &str) -> String {
    format!("/api/v1/training-runs/{id}{suffix}")
}

async fn api_answering(route: &str, response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

async fn list_from(body: serde_json::Value, filter: &PolicyFilter) -> Result<Vec<Policy>, Error> {
    let server = api_answering(
        "/api/v1/training-runs",
        ResponseTemplate::new(200).set_body_json(body),
    )
    .await;
    client(&server.uri()).policies().list(filter).await
}

async fn one_from(body: serde_json::Value) -> Policy {
    let mut policies = list_from(json!([body]), &PolicyFilter::new())
        .await
        .unwrap();
    policies.remove(0)
}

fn ids(policies: &[Policy]) -> Vec<&str> {
    policies.iter().map(|policy| policy.id.as_str()).collect()
}

fn not_found(detail: &str) -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_json(json!({ "detail": detail }))
}

fn assert_not_found(error: &Error, expected: &str) {
    assert!(
        matches!(error, Error::NotFound { message } if message == expected),
        "{error:?}"
    );
}

#[tokio::test]
async fn list_sends_get_with_the_bearer_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/training-runs"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;

    client(&server.uri())
        .policies()
        .list(&PolicyFilter::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn list_decodes_every_exposed_field_from_its_wire_name() {
    let policy = one_from(act_body()).await;

    assert_eq!(
        policy,
        Policy {
            id: PolicyId::new(ACT_ID),
            policy_type: "act".into(),
            state: PolicyState::Ready,
            dataset_id: PICK_DATASET.into(),
            dataset_version_id: "ver-41d2".into(),
            phase: "done".into(),
            step: 20000,
            total_steps: 20000,
            error: None,
            created_at: "2026-10-01T09:30:00Z".into(),
            started_at: Some("2026-10-01T09:31:12Z".into()),
            finished_at: Some("2026-10-01T11:02:40Z".into()),
        }
    );
}

#[tokio::test]
async fn list_maps_each_known_status_to_its_state() {
    let cases = [
        ("queued", PolicyState::Queued),
        ("running", PolicyState::Training),
        ("succeeded", PolicyState::Ready),
        ("failed", PolicyState::Failed),
    ];
    for (status, expected) in cases {
        let policy = one_from(with(act_body(), "status", json!(status))).await;

        assert_eq!(policy.state, expected, "status {status}");
    }
}

#[tokio::test]
async fn list_keeps_an_unknown_status_as_other() {
    let policy = one_from(with(act_body(), "status", json!("cancelled"))).await;

    assert_eq!(policy.state, PolicyState::Other("cancelled".into()));
    assert_eq!(policy.state.as_str(), "cancelled");
}

#[tokio::test]
async fn list_keeps_an_unknown_policy_type_as_sent() {
    let policy = one_from(with(act_body(), "policy", json!("smolvla"))).await;

    assert_eq!(policy.policy_type, "smolvla");
}

#[tokio::test]
async fn list_ignores_unknown_fields() {
    let body = with(act_body(), "priority", json!(3));

    let policies = list_from(json!([body]), &PolicyFilter::new()).await;

    assert!(policies.is_ok(), "{policies:?}");
}

#[tokio::test]
async fn list_decodes_null_times_and_error_as_none() {
    let mut body = act_body();
    body["started_at"] = json!(null);
    body["finished_at"] = json!(null);
    body["error"] = json!(null);

    let policy = one_from(body).await;

    assert_eq!(
        (policy.started_at, policy.finished_at, policy.error),
        (None, None, None)
    );
}

#[tokio::test]
async fn list_keeps_the_order_of_the_response() {
    let policies = list_from(json!([groot_body(), act_body()]), &PolicyFilter::new())
        .await
        .unwrap();

    assert_eq!(ids(&policies), [GROOT_ID, ACT_ID]);
}

#[tokio::test]
async fn list_with_a_state_filter_returns_only_policies_in_that_state() {
    let filter = PolicyFilter::new().state(PolicyState::Training);

    let policies = list_from(json!([act_body(), groot_body()]), &filter)
        .await
        .unwrap();

    assert_eq!(ids(&policies), [GROOT_ID]);
}

#[tokio::test]
async fn list_with_a_dataset_filter_returns_only_policies_of_that_dataset() {
    let filter = PolicyFilter::new().dataset_id(PICK_DATASET);

    let policies = list_from(json!([act_body(), groot_body()]), &filter)
        .await
        .unwrap();

    assert_eq!(ids(&policies), [ACT_ID]);
}

#[tokio::test]
async fn list_with_both_filters_returns_only_policies_matching_both() {
    let failed_pick = with(
        with(groot_body(), "dataset_id", json!(PICK_DATASET)),
        "status",
        json!("failed"),
    );
    let filter = PolicyFilter::new()
        .state(PolicyState::Ready)
        .dataset_id(PICK_DATASET);

    let policies = list_from(json!([failed_pick, groot_body(), act_body()]), &filter)
        .await
        .unwrap();

    assert_eq!(ids(&policies), [ACT_ID]);
}

#[tokio::test]
async fn get_requests_the_policy_by_id() {
    let server = api_answering(
        &run_url(ACT_ID, ""),
        ResponseTemplate::new(200).set_body_json(act_body()),
    )
    .await;

    let policy = client(&server.uri())
        .policies()
        .get(&PolicyId::new(ACT_ID))
        .await
        .unwrap();

    assert_eq!(policy.id, PolicyId::new(ACT_ID));
}

#[tokio::test]
async fn get_returns_not_found_for_an_unknown_id() {
    let server = api_answering(
        &run_url("run-missing", ""),
        not_found("Unknown training run"),
    )
    .await;

    let error = client(&server.uri())
        .policies()
        .get(&PolicyId::new("run-missing"))
        .await
        .unwrap_err();

    assert_not_found(&error, "Unknown training run");
}

#[tokio::test]
async fn get_sends_an_id_with_a_slash_as_one_encoded_segment() {
    let server = api_answering(
        "/api/v1/training-runs/run%2F1",
        ResponseTemplate::new(200).set_body_json(act_body()),
    )
    .await;

    let policy = client(&server.uri())
        .policies()
        .get(&PolicyId::new("run/1"))
        .await;

    assert!(policy.is_ok(), "{policy:?}");
}

#[tokio::test]
async fn lineage_decodes_the_dataset_version_dataset_task_embodiment_and_episodes() {
    let server = api_answering(
        &run_url(ACT_ID, "/lineage"),
        ResponseTemplate::new(200).set_body_json(json!({
            "run_id": ACT_ID,
            "policy": "act",
            "backend": "sagemaker",
            "status": "succeeded",
            "version_id": "ver-41d2",
            "version_name": "v2-accepted",
            "dataset_id": PICK_DATASET,
            "task_description": "Pick up the red cube",
            "embodiment_label": "so100",
            "embodiment_fingerprint": "f3a9c1",
            "episode_count": 2,
            "episode_ids": ["ep-0001", "ep-0002"]
        })),
    )
    .await;

    let lineage = client(&server.uri())
        .policies()
        .lineage(&PolicyId::new(ACT_ID))
        .await
        .unwrap();

    assert_eq!(
        lineage,
        Lineage {
            dataset_id: PICK_DATASET.into(),
            dataset_version_id: "ver-41d2".into(),
            dataset_version_name: "v2-accepted".into(),
            task: "Pick up the red cube".into(),
            embodiment: "so100".into(),
            embodiment_fingerprint: "f3a9c1".into(),
            episodes: 2,
            episode_ids: vec!["ep-0001".into(), "ep-0002".into()],
        }
    );
}

#[tokio::test]
async fn checkpoints_decodes_each_checkpoint_and_keeps_the_order_oldest_first() {
    let server = api_answering(
        &run_url(ACT_ID, "/checkpoints"),
        ResponseTemplate::new(200).set_body_json(json!([
            {"id": "010000", "size": 207_618_048, "last_modified": "2026-10-01T10:15:00Z"},
            {"id": "020000", "size": 207_618_112, "last_modified": "2026-10-01T11:00:00Z"}
        ])),
    )
    .await;

    let checkpoints = client(&server.uri())
        .policies()
        .checkpoints(&PolicyId::new(ACT_ID))
        .await
        .unwrap();

    assert_eq!(
        checkpoints,
        [
            Checkpoint {
                id: "010000".into(),
                size: 207_618_048,
                last_modified: "2026-10-01T10:15:00Z".into(),
            },
            Checkpoint {
                id: "020000".into(),
                size: 207_618_112,
                last_modified: "2026-10-01T11:00:00Z".into(),
            },
        ]
    );
}

#[tokio::test]
async fn checkpoints_returns_not_found_for_an_unknown_id() {
    let server = api_answering(
        &run_url("run-missing", "/checkpoints"),
        not_found("Unknown training run"),
    )
    .await;

    let error = client(&server.uri())
        .policies()
        .checkpoints(&PolicyId::new("run-missing"))
        .await
        .unwrap_err();

    assert_not_found(&error, "Unknown training run");
}

#[tokio::test]
async fn log_returns_the_text_body_unchanged() {
    let text = "step 100 loss 0.412\nstep 200 loss 0.371\n";
    let server = api_answering(
        &run_url(ACT_ID, "/log"),
        ResponseTemplate::new(200).set_body_raw(text, "text/plain; charset=utf-8"),
    )
    .await;

    let log = client(&server.uri())
        .policies()
        .log(&PolicyId::new(ACT_ID))
        .await
        .unwrap();

    assert_eq!(log, text);
}

#[tokio::test]
async fn log_returns_not_found_when_the_log_is_missing() {
    let server = api_answering(
        &run_url(ACT_ID, "/log"),
        not_found("Unknown training run, or no log yet"),
    )
    .await;

    let error = client(&server.uri())
        .policies()
        .log(&PolicyId::new(ACT_ID))
        .await
        .unwrap_err();

    assert_not_found(&error, "Unknown training run, or no log yet");
}

#[tokio::test]
async fn log_replaces_invalid_utf8_with_replacement_characters() {
    let server = api_answering(
        &run_url(ACT_ID, "/log"),
        ResponseTemplate::new(200).set_body_raw(b"loss \xff0.4".to_vec(), "text/plain"),
    )
    .await;

    let log = client(&server.uri())
        .policies()
        .log(&PolicyId::new(ACT_ID))
        .await
        .unwrap();

    assert_eq!(log, "loss \u{fffd}0.4");
}

#[tokio::test]
async fn list_requests_the_path_below_a_base_url_prefix() {
    let server = api_answering(
        "/platform/api/v1/training-runs",
        ResponseTemplate::new(200).set_body_json(json!([])),
    )
    .await;

    let policies = client(&format!("{}/platform", server.uri()))
        .policies()
        .list(&PolicyFilter::new())
        .await;

    assert!(policies.is_ok(), "{policies:?}");
}

#[tokio::test]
async fn get_requests_the_path_below_a_base_url_prefix() {
    let server = api_answering(
        &format!("/platform{}", run_url(ACT_ID, "")),
        ResponseTemplate::new(200).set_body_json(act_body()),
    )
    .await;

    let policy = client(&format!("{}/platform", server.uri()))
        .policies()
        .get(&PolicyId::new(ACT_ID))
        .await;

    assert!(policy.is_ok(), "{policy:?}");
}

#[tokio::test]
async fn log_returns_auth_with_status_401_when_the_token_is_rejected() {
    let server = api_answering(
        &run_url(ACT_ID, "/log"),
        ResponseTemplate::new(401).set_body_json(json!({"detail": "Invalid token"})),
    )
    .await;

    let error = client(&server.uri())
        .policies()
        .log(&PolicyId::new(ACT_ID))
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            Error::Auth {
                status: Some(401),
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn list_returns_auth_with_status_401_when_the_token_is_rejected() {
    let server = api_answering(
        "/api/v1/training-runs",
        ResponseTemplate::new(401).set_body_json(json!({"detail": "Invalid token"})),
    )
    .await;

    let error = client(&server.uri())
        .policies()
        .list(&PolicyFilter::new())
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            Error::Auth {
                status: Some(401),
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn list_returns_unexpected_response_for_an_entry_without_status() {
    let mut body = act_body();
    body.as_object_mut().unwrap().remove("status");

    let error = list_from(json!([body]), &PolicyFilter::new())
        .await
        .unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
        "{error:?}"
    );
}

#[test]
fn state_names_use_the_policy_vocabulary() {
    let names: Vec<String> = [
        PolicyState::Queued,
        PolicyState::Training,
        PolicyState::Ready,
        PolicyState::Failed,
    ]
    .iter()
    .map(ToString::to_string)
    .collect();

    assert_eq!(names, ["queued", "training", "ready", "failed"]);
}
