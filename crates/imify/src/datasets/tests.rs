use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::auth::StaticToken;

const TOKEN: &str = "tok_7f3a9c-secret";
const PICK_ID: &str = "0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13";
const STACK_ID: &str = "5e2d7a90-1c3b-4e6f-8a7d-9b0c1d2e3f45";

fn client(base_url: &str) -> Client {
    Client::builder(base_url)
        .token_source(StaticToken::new(TOKEN))
        .retry_policy(crate::RetryPolicy::none())
        .build()
        .unwrap()
}

fn pick_body() -> serde_json::Value {
    json!({
        "id": PICK_ID,
        "task_description": "Pick up the red cube",
        "dataset_format": "lerobot_v3",
        "embodiment_label": "so100",
        "embodiment_fingerprint": "f3a9c1",
        "cameras": ["wrist", "top"],
        "action_dim": 6,
        "created_at": "2026-10-01T09:30:00Z",
        "episode_count": 50,
        "accepted_count": 48
    })
}

fn stack_body() -> serde_json::Value {
    json!({
        "id": STACK_ID,
        "task_description": "Stack two blocks",
        "dataset_format": "lerobot_v3",
        "embodiment_label": "koch",
        "embodiment_fingerprint": "7c2e04",
        "cameras": ["top"],
        "action_dim": 6,
        "created_at": "2026-09-12T14:05:00Z",
        "episode_count": 12,
        "accepted_count": 12
    })
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

async fn list_from(body: serde_json::Value) -> Result<Vec<Dataset>, Error> {
    let server = api_answering(
        "/api/v1/datasets",
        ResponseTemplate::new(200).set_body_json(body),
    )
    .await;
    client(&server.uri()).datasets().list().await
}

#[tokio::test]
async fn list_sends_get_with_the_bearer_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/datasets"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;

    client(&server.uri()).datasets().list().await.unwrap();
}

#[tokio::test]
async fn list_decodes_every_field_from_its_wire_name() {
    let datasets = list_from(json!([pick_body()])).await.unwrap();

    assert_eq!(
        datasets,
        [Dataset {
            id: PICK_ID.into(),
            task: "Pick up the red cube".into(),
            format: "lerobot_v3".into(),
            embodiment: "so100".into(),
            embodiment_fingerprint: "f3a9c1".into(),
            cameras: vec!["wrist".into(), "top".into()],
            action_dim: 6,
            created_at: "2026-10-01T09:30:00Z".into(),
            episodes: 50,
            accepted: 48,
        }]
    );
}

#[tokio::test]
async fn list_ignores_unknown_fields() {
    let mut body = pick_body();
    body["storage_bytes"] = json!(1_048_576);

    let datasets = list_from(json!([body])).await;

    assert!(datasets.is_ok(), "{datasets:?}");
}

#[tokio::test]
async fn list_returns_no_datasets_for_an_empty_array() {
    let datasets = list_from(json!([])).await.unwrap();

    assert_eq!(datasets, Vec::<Dataset>::new());
}

#[tokio::test]
async fn list_keeps_the_order_of_the_response() {
    let datasets = list_from(json!([stack_body(), pick_body()])).await.unwrap();

    let ids: Vec<&str> = datasets.iter().map(|dataset| dataset.id.as_str()).collect();
    assert_eq!(ids, [STACK_ID, PICK_ID]);
}

#[tokio::test]
async fn list_keeps_created_at_exactly_as_sent() {
    let mut body = pick_body();
    body["created_at"] = json!("2026-10-01T11:30:00.123456+02:00");

    let datasets = list_from(json!([body])).await.unwrap();

    assert_eq!(datasets[0].created_at, "2026-10-01T11:30:00.123456+02:00");
}

#[tokio::test]
async fn list_requests_the_path_below_a_base_url_prefix() {
    let server = api_answering(
        "/platform/api/v1/datasets",
        ResponseTemplate::new(200).set_body_json(json!([])),
    )
    .await;

    let datasets = client(&format!("{}/platform", server.uri()))
        .datasets()
        .list()
        .await;

    assert!(datasets.is_ok(), "{datasets:?}");
}

#[tokio::test]
async fn list_returns_auth_with_status_401_when_the_token_is_rejected() {
    let server = api_answering(
        "/api/v1/datasets",
        ResponseTemplate::new(401).set_body_json(json!({"detail": "Invalid token"})),
    )
    .await;

    let error = client(&server.uri()).datasets().list().await.unwrap_err();

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
async fn list_returns_unexpected_response_for_an_object_body() {
    let error = list_from(json!({"items": [pick_body()]}))
        .await
        .unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn list_returns_unexpected_response_for_an_entry_without_episode_count() {
    let mut body = pick_body();
    body.as_object_mut().unwrap().remove("episode_count");

    let error = list_from(json!([body])).await.unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
        "{error:?}"
    );
}
