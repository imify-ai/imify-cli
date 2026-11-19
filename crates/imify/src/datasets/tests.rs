use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
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
        "name": format!("datasets/{PICK_ID}"),
        "displayName": "",
        "description": "",
        "embodiment": "embodiments/f3a9c1",
        "task": "Pick up the red cube",
        "format": "v3",
        "state": "ACTIVE",
        "episodeCounts": {"ACCEPTED": 48, "PENDING_REVIEW": 2},
        "createTime": "2026-10-01T09:30:00Z",
        "updateTime": "2026-10-02T09:30:00Z",
        "etag": "W/\"1\""
    })
}

fn stack_body() -> serde_json::Value {
    json!({
        "name": format!("datasets/{STACK_ID}"),
        "embodiment": "embodiments/7c2e04",
        "task": "Stack two blocks",
        "format": "v3",
        "state": "ACTIVE",
        "episodeCounts": {"ACCEPTED": 12},
        "createTime": "2026-09-12T14:05:00Z"
    })
}

fn so100() -> serde_json::Value {
    json!({
        "name": "embodiments/f3a9c1",
        "displayName": "so100",
        "fps": 30,
        "actionDim": 6,
        "stateDim": 6,
        "actionNames": [],
        "cameras": ["wrist", "top"],
        "createTime": "2026-09-01T00:00:00Z"
    })
}

/// A server that lists `datasets` and `embodiments` in one page each.
async fn api_listing(
    datasets: &[serde_json::Value],
    embodiments: &[serde_json::Value],
) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/datasets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"datasets": datasets})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/embodiments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"embodiments": embodiments})))
        .mount(&server)
        .await;
    server
}

async fn list_from(datasets: &[serde_json::Value]) -> Result<Vec<Dataset>, Error> {
    let server = api_listing(datasets, &[so100()]).await;
    client(&server.uri()).datasets().list().await
}

#[tokio::test]
async fn list_sends_get_with_the_bearer_token() {
    let server = MockServer::start().await;
    for route in ["/api/v1/datasets", "/api/v1/embodiments"] {
        Mock::given(method("GET"))
            .and(path(route))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
    }

    client(&server.uri()).datasets().list().await.unwrap();
}

#[tokio::test]
async fn list_joins_each_dataset_with_its_embodiment() {
    let datasets = list_from(&[pick_body()]).await.unwrap();

    assert_eq!(
        datasets,
        [Dataset {
            id: PICK_ID.into(),
            task: "Pick up the red cube".into(),
            format: "v3".into(),
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
async fn list_leaves_the_label_empty_for_an_embodiment_not_listed() {
    let datasets = list_from(&[stack_body()]).await.unwrap();

    assert_eq!(
        (
            datasets[0].embodiment.as_str(),
            datasets[0].embodiment_fingerprint.as_str(),
            datasets[0].cameras.len(),
            datasets[0].action_dim,
        ),
        ("", "7c2e04", 0, 0)
    );
}

#[tokio::test]
async fn list_counts_zero_accepted_episodes_when_the_state_is_missing() {
    let mut body = pick_body();
    body["episodeCounts"] = json!({"PENDING_REVIEW": 3});

    let datasets = list_from(&[body]).await.unwrap();

    assert_eq!((datasets[0].episodes, datasets[0].accepted), (3, 0));
}

#[tokio::test]
async fn list_ignores_unknown_fields() {
    let mut body = pick_body();
    body["storageBytes"] = json!(1_048_576);

    let datasets = list_from(&[body]).await;

    assert!(datasets.is_ok(), "{datasets:?}");
}

#[tokio::test]
async fn list_returns_no_datasets_for_an_empty_page() {
    let datasets = list_from(&[]).await.unwrap();

    assert_eq!(datasets, Vec::<Dataset>::new());
}

#[tokio::test]
async fn list_keeps_the_order_of_the_response() {
    let datasets = list_from(&[stack_body(), pick_body()]).await.unwrap();

    let ids: Vec<&str> = datasets.iter().map(|dataset| dataset.id.as_str()).collect();
    assert_eq!(ids, [STACK_ID, PICK_ID]);
}

#[tokio::test]
async fn list_reads_every_page() {
    let server = api_listing(&[], &[so100()]).await;
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/datasets"))
        .and(query_param("pageToken", "next"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"datasets": [stack_body()]})))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/datasets"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"datasets": [pick_body()], "nextPageToken": "next"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/embodiments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"embodiments": []})))
        .mount(&server)
        .await;

    let datasets = client(&server.uri()).datasets().list().await.unwrap();

    let ids: Vec<&str> = datasets.iter().map(|dataset| dataset.id.as_str()).collect();
    assert_eq!(ids, [PICK_ID, STACK_ID]);
}

#[tokio::test]
async fn list_keeps_created_at_exactly_as_sent() {
    let mut body = pick_body();
    body["createTime"] = json!("2026-10-01T11:30:00.123456+02:00");

    let datasets = list_from(&[body]).await.unwrap();

    assert_eq!(datasets[0].created_at, "2026-10-01T11:30:00.123456+02:00");
}

#[tokio::test]
async fn list_requests_the_path_below_a_base_url_prefix() {
    let server = MockServer::start().await;
    for route in ["/platform/api/v1/datasets", "/platform/api/v1/embodiments"] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
    }

    let datasets = client(&format!("{}/platform", server.uri()))
        .datasets()
        .list()
        .await;

    assert!(datasets.is_ok(), "{datasets:?}");
}

#[tokio::test]
async fn list_returns_auth_with_status_401_when_the_token_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_json(
            json!({"error": {"code": 401, "status": "UNAUTHENTICATED", "message": "Invalid token"}}),
        ))
        .mount(&server)
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
async fn list_returns_unexpected_response_for_an_entry_without_name() {
    let mut body = pick_body();
    body.as_object_mut().unwrap().remove("name");

    let error = list_from(&[body]).await.unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
        "{error:?}"
    );
}
