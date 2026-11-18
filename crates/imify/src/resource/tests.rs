use serde_json::json;
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

fn client(base_url: &str) -> Client {
    Client::builder(base_url)
        .retry_policy(crate::RetryPolicy::none())
        .build()
        .unwrap()
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct Item {
    name: String,
}

#[test]
fn last_segment_is_the_id_of_a_resource_name() {
    assert_eq!(last_segment("datasets/d1/versions/v2"), "v2");
}

#[test]
fn last_segment_of_a_bare_id_is_the_id() {
    assert_eq!(last_segment("3f2c9a"), "3f2c9a");
}

#[test]
fn url_encodes_each_segment_as_a_path_segment() {
    let client = client("https://api.example.com");

    let url = url(&client, ["datasets", "d1", "versions", "v 1/old"]).unwrap();

    assert_eq!(
        url.as_str(),
        "https://api.example.com/api/v1/datasets/d1/versions/v%201%2Fold"
    );
}

#[test]
fn url_keeps_the_path_of_the_base_url() {
    let client = client("https://example.com/platform");

    let url = url(&client, ["users", "me"]).unwrap();

    assert_eq!(url.as_str(), "https://example.com/platform/api/v1/users/me");
}

#[tokio::test]
async fn list_all_follows_next_page_token_until_it_is_empty() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/items"))
        .and(query_param("pageSize", "1000"))
        .and(query_param_is_missing("pageToken"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"items": [{"name": "items/a"}], "nextPageToken": "page 2&more"}),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/items"))
        .and(query_param("pageToken", "page 2&more"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"items": [{"name": "items/b"}], "nextPageToken": ""})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = client(&server.uri());

    let items: Vec<Item> = list_all(&client, &url(&client, ["items"]).unwrap(), "items")
        .await
        .unwrap();

    assert_eq!(
        items,
        [
            Item {
                name: "items/a".into()
            },
            Item {
                name: "items/b".into()
            }
        ]
    );
}

#[tokio::test]
async fn list_all_reads_a_page_without_the_collection_as_empty() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    let client = client(&server.uri());

    let items: Vec<Item> = list_all(&client, &url(&client, ["items"]).unwrap(), "items")
        .await
        .unwrap();

    assert_eq!(items, []);
}

#[tokio::test]
async fn list_all_fails_when_the_page_token_repeats() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/items"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"items": [], "nextPageToken": "same"})),
        )
        .mount(&server)
        .await;
    let client = client(&server.uri());

    let error = list_all::<Item>(&client, &url(&client, ["items"]).unwrap(), "items")
        .await
        .unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn list_all_fails_when_an_item_does_not_match_the_type() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": [{"id": 1}]})))
        .mount(&server)
        .await;
    let client = client(&server.uri());

    let error = list_all::<Item>(&client, &url(&client, ["items"]).unwrap(), "items")
        .await
        .unwrap_err();

    assert!(
        matches!(error, Error::UnexpectedResponse { status: 200, .. }),
        "{error:?}"
    );
}
