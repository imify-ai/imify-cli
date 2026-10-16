use std::io;
use std::time::Duration;

use reqwest::Method;

use super::*;
use crate::error::ConnectFailed;

fn policy() -> RetryPolicy {
    RetryPolicy {
        max_retries: 3,
        initial_backoff: Duration::from_millis(100),
        max_backoff: Duration::from_secs(2),
    }
}

fn connect_error() -> Error {
    Error::Network {
        source: Box::new(ConnectFailed(Box::new(io::Error::from(
            io::ErrorKind::ConnectionRefused,
        )))),
    }
}

fn read_timeout() -> Error {
    Error::Network {
        source: Box::new(io::Error::from(io::ErrorKind::TimedOut)),
    }
}

fn server_error() -> Error {
    Error::Server {
        status: 503,
        message: "Service Unavailable".into(),
    }
}

fn rate_limited() -> Error {
    Error::RateLimited { retry_after: None }
}

fn retries_first_attempt(method: &Method, error: &Error) -> bool {
    should_retry(&policy(), method, error, 0)
}

#[test]
fn get_is_retried_on_connect_error() {
    assert!(retries_first_attempt(&Method::GET, &connect_error()));
}

#[test]
fn get_is_retried_on_read_timeout() {
    assert!(retries_first_attempt(&Method::GET, &read_timeout()));
}

#[test]
fn get_is_retried_on_server_error() {
    assert!(retries_first_attempt(&Method::GET, &server_error()));
}

#[test]
fn get_is_retried_on_rate_limit() {
    assert!(retries_first_attempt(
        &Method::GET,
        &Error::RateLimited {
            retry_after: Some(Duration::from_secs(1))
        }
    ));
}

#[test]
fn get_is_retried_on_edge_block() {
    assert!(retries_first_attempt(&Method::GET, &rate_limited()));
}

#[test]
fn idempotent_methods_are_retried_on_server_error() {
    for method in [Method::HEAD, Method::PUT, Method::DELETE] {
        assert!(retries_first_attempt(&method, &server_error()), "{method}");
    }
}

#[test]
fn get_is_not_retried_on_client_errors() {
    let client_errors = [
        Error::Validation {
            message: "Bad Request".into(),
            fields: Vec::new(),
        },
        Error::Auth {
            status: Some(401),
            message: "Not authenticated".into(),
        },
        Error::Auth {
            status: Some(403),
            message: "Not enough permissions".into(),
        },
        Error::NotFound {
            message: "Not Found".into(),
        },
        Error::Conflict {
            message: "Conflict".into(),
        },
    ];

    for error in &client_errors {
        assert!(!retries_first_attempt(&Method::GET, error), "{error}");
    }
}

#[test]
fn post_is_retried_on_connect_error() {
    assert!(retries_first_attempt(&Method::POST, &connect_error()));
}

#[test]
fn post_is_retried_on_rate_limit() {
    assert!(retries_first_attempt(&Method::POST, &rate_limited()));
}

#[test]
fn post_is_not_retried_on_server_error() {
    assert!(!retries_first_attempt(&Method::POST, &server_error()));
}

#[test]
fn post_is_not_retried_on_read_timeout() {
    assert!(!retries_first_attempt(&Method::POST, &read_timeout()));
}

#[test]
fn no_retry_happens_after_max_retries() {
    assert!(!should_retry(&policy(), &Method::GET, &server_error(), 3));
}

#[test]
fn last_allowed_retry_happens() {
    assert!(should_retry(&policy(), &Method::GET, &server_error(), 2));
}

#[test]
fn backoff_ceiling_doubles_with_each_attempt() {
    let ceilings: Vec<_> = (0..4)
        .map(|attempt| backoff_ceiling(&policy(), attempt))
        .collect();

    assert_eq!(
        ceilings,
        [
            Duration::from_millis(100),
            Duration::from_millis(200),
            Duration::from_millis(400),
            Duration::from_millis(800),
        ]
    );
}

#[test]
fn backoff_ceiling_is_capped_at_max_backoff() {
    assert_eq!(backoff_ceiling(&policy(), 40), Duration::from_secs(2));
}

#[test]
fn delay_never_exceeds_max_backoff() {
    for attempt in 0..64 {
        assert!(delay(&policy(), attempt, &server_error()) <= Duration::from_secs(2));
    }
}

#[test]
fn retry_after_sets_the_delay() {
    let error = Error::RateLimited {
        retry_after: Some(Duration::from_secs(1)),
    };

    assert_eq!(delay(&policy(), 0, &error), Duration::from_secs(1));
}

#[test]
fn retry_after_is_capped_at_max_backoff() {
    let error = Error::RateLimited {
        retry_after: Some(Duration::from_secs(120)),
    };

    assert_eq!(delay(&policy(), 0, &error), Duration::from_secs(2));
}

#[test]
fn none_policy_never_retries() {
    assert!(!should_retry(
        &RetryPolicy::none(),
        &Method::GET,
        &server_error(),
        0
    ));
}

#[test]
fn default_policy_retries_three_times() {
    let policy = RetryPolicy::default();

    assert_eq!(
        (
            policy.max_retries,
            policy.initial_backoff,
            policy.max_backoff
        ),
        (3, Duration::from_millis(500), Duration::from_secs(30))
    );
}
