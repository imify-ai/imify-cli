use std::net::TcpListener as StdListener;

use super::*;
use crate::auth::testing::free_port;

async fn get(port: u16, path_and_query: &str) -> reqwest::Response {
    reqwest::get(format!("http://127.0.0.1:{port}{path_and_query}"))
        .await
        .unwrap()
}

#[tokio::test]
async fn bind_uses_the_first_free_port() {
    let port = free_port();

    let listener = RedirectListener::bind(&[port]).await.unwrap();

    assert_eq!(listener.port(), port);
}

#[tokio::test]
async fn bind_takes_the_next_port_when_the_first_is_in_use() {
    let taken = StdListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let taken_port = taken.local_addr().unwrap().port();
    let free = free_port();

    let listener = RedirectListener::bind(&[taken_port, free]).await.unwrap();

    assert_eq!(listener.port(), free);
}

#[tokio::test]
async fn bind_fails_with_ports_in_use_when_every_port_is_taken() {
    let first = StdListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let second = StdListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let ports = [
        first.local_addr().unwrap().port(),
        second.local_addr().unwrap().port(),
    ];

    let error = RedirectListener::bind(&ports).await.unwrap_err();

    assert!(
        matches!(
            error,
            Error::Login {
                reason: LoginFailure::PortsInUse,
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn wait_returns_the_code_and_state_of_the_redirect() {
    let listener = RedirectListener::bind(&[free_port()]).await.unwrap();
    let port = listener.port();
    let waiting = tokio::spawn(listener.wait(Duration::from_secs(10)));

    let response = get(port, "/callback?code=c-1&state=s-1").await;

    assert_eq!(response.status(), 200);
    let callback = waiting.await.unwrap().unwrap();
    assert!(
        matches!(callback, Callback::Code { ref code, ref state } if code == "c-1" && state == "s-1")
    );
}

#[tokio::test]
async fn wait_answers_other_paths_with_404_and_keeps_waiting() {
    let listener = RedirectListener::bind(&[free_port()]).await.unwrap();
    let port = listener.port();
    let waiting = tokio::spawn(listener.wait(Duration::from_secs(10)));

    let favicon = get(port, "/favicon.ico").await;
    get(port, "/callback?code=c-1&state=s-1").await;

    assert_eq!(favicon.status(), 404);
    assert!(matches!(
        waiting.await.unwrap().unwrap(),
        Callback::Code { .. }
    ));
}

#[tokio::test]
async fn wait_is_not_blocked_by_a_connection_that_sends_nothing() {
    let listener = RedirectListener::bind(&[free_port()]).await.unwrap();
    let port = listener.port();
    let waiting = tokio::spawn(listener.wait(Duration::from_secs(5)));

    let _silent = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    get(port, "/callback?code=c-1&state=s-1").await;

    assert!(waiting.await.unwrap().is_ok());
}

#[tokio::test]
async fn wait_returns_the_provider_error_of_the_redirect() {
    let listener = RedirectListener::bind(&[free_port()]).await.unwrap();
    let port = listener.port();
    let waiting = tokio::spawn(listener.wait(Duration::from_secs(10)));

    get(port, "/callback?error=access_denied&state=s-1").await;

    assert!(matches!(
        waiting.await.unwrap().unwrap(),
        Callback::Error { ref error, .. } if error == "access_denied"
    ));
}

#[tokio::test]
async fn wait_fails_with_timed_out_without_a_redirect() {
    let listener = RedirectListener::bind(&[free_port()]).await.unwrap();

    let error = listener.wait(Duration::from_millis(50)).await.unwrap_err();

    assert!(
        matches!(
            error,
            Error::Login {
                reason: LoginFailure::TimedOut,
                ..
            }
        ),
        "{error:?}"
    );
}

#[test]
fn callback_debug_redacts_the_code_and_state() {
    let callback = Callback::Code {
        code: "code-secret".into(),
        state: "state-secret".into(),
    };

    let debug = format!("{callback:?}");

    assert!(!debug.contains("code-secret"), "{debug}");
    assert!(!debug.contains("state-secret"), "{debug}");
}
