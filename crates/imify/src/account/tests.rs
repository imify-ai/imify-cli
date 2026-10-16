use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::auth::StaticToken;

const TOKEN: &str = "tok_7f3a9c-secret";

fn client(base_url: &str) -> Client {
    Client::builder(base_url)
        .token_source(StaticToken::new(TOKEN))
        .retry_policy(crate::RetryPolicy::none())
        .build()
        .unwrap()
}

fn user_body() -> serde_json::Value {
    json!({"subject": "user-1234", "name": "Ada Lovelace", "role": "admin"})
}

async fn mount_me(server: &MockServer, route: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(response)
        .mount(server)
        .await;
}

#[tokio::test]
async fn me_sends_get_with_the_bearer_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(user_body()))
        .expect(1)
        .mount(&server)
        .await;

    client(&server.uri()).me().await.unwrap();
}

#[tokio::test]
async fn me_decodes_the_user_and_ignores_unknown_fields() {
    let server = MockServer::start().await;
    let mut body = user_body();
    body["organization"] = json!("robotics-lab");
    mount_me(
        &server,
        "/api/v1/me",
        ResponseTemplate::new(200).set_body_json(body),
    )
    .await;

    let user = client(&server.uri()).me().await.unwrap();

    assert_eq!(
        user,
        User {
            subject: "user-1234".into(),
            name: "Ada Lovelace".into(),
            role: "admin".into(),
        }
    );
}

#[tokio::test]
async fn me_requests_the_path_below_a_base_url_prefix() {
    let server = MockServer::start().await;
    mount_me(
        &server,
        "/platform/api/v1/me",
        ResponseTemplate::new(200).set_body_json(user_body()),
    )
    .await;

    let user = client(&format!("{}/platform", server.uri())).me().await;

    assert!(user.is_ok(), "{user:?}");
}

#[tokio::test]
async fn me_returns_auth_with_status_401_that_does_not_contain_the_token() {
    let server = MockServer::start().await;
    mount_me(
        &server,
        "/api/v1/me",
        ResponseTemplate::new(401).set_body_json(json!({"detail": "Invalid token"})),
    )
    .await;

    let error = client(&server.uri()).me().await.unwrap_err();

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
    assert!(!error.to_string().contains(TOKEN));
}
