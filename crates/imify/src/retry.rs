//! When and how long the client waits before repeating a failed request.

use std::time::Duration;

use reqwest::Method;

use crate::Error;

/// How often and how long the client retries a failed request.
///
/// The client retries `GET`, `HEAD`, `PUT` and `DELETE` on network errors,
/// [`Error::Server`] and [`Error::RateLimited`]. It retries other methods, such as
/// `POST`, only when the connection failed or the request was rate limited, because in
/// those cases the API did not process it.
///
/// The delay before retry `n` (counting from 0) is a random duration between zero and
/// `min(max_backoff, initial_backoff * 2^n)`. A `Retry-After` delay sent with a rate
/// limit replaces it, capped at `max_backoff`.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use imify::RetryPolicy;
///
/// let mut policy = RetryPolicy::default();
/// policy.max_retries = 5;
/// policy.max_backoff = Duration::from_secs(10);
///
/// let client = imify::Client::builder("https://api.example.com")
///     .retry_policy(policy)
///     .build()?;
/// # Ok::<(), imify::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RetryPolicy {
    /// Retries after the first attempt. Default: 3.
    pub max_retries: u32,
    /// Upper bound of the delay before the first retry. Default: 500 ms.
    pub initial_backoff: Duration,
    /// Upper bound of any delay. Default: 30 s.
    pub max_backoff: Duration,
}

impl RetryPolicy {
    /// A policy that sends every request once.
    ///
    /// # Examples
    ///
    /// ```
    /// let policy = imify::RetryPolicy::none();
    /// assert_eq!(policy.max_retries, 0);
    /// ```
    #[must_use]
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            ..Self::default()
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
        }
    }
}

/// Which failures a request is repeated after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryMode {
    /// Sending the request twice has the same effect as sending it once: retried on
    /// network errors, server errors and rate limits.
    Idempotent,
    /// Sending the request twice may apply it twice: retried only on connection failures
    /// and rate limits, when the API did not process it.
    NotIdempotent,
}

impl From<&Method> for RetryMode {
    fn from(method: &Method) -> Self {
        if [Method::GET, Method::HEAD, Method::PUT, Method::DELETE].contains(method) {
            Self::Idempotent
        } else {
            Self::NotIdempotent
        }
    }
}

/// Decides whether the request that failed with `error` on retry number `attempt` (the
/// first attempt is 0) is sent again.
pub(crate) fn should_retry(
    policy: &RetryPolicy,
    mode: impl Into<RetryMode>,
    error: &Error,
    attempt: u32,
) -> bool {
    attempt < policy.max_retries && is_retryable(mode.into(), error)
}

/// Returns how long to wait before retry number `attempt`.
pub(crate) fn delay(policy: &RetryPolicy, attempt: u32, error: &Error) -> Duration {
    match error {
        Error::RateLimited {
            retry_after: Some(requested),
        } => (*requested).min(policy.max_backoff),
        _ => full_jitter(backoff_ceiling(policy, attempt)),
    }
}

fn is_retryable(mode: RetryMode, error: &Error) -> bool {
    match error {
        Error::RateLimited { .. } => true,
        Error::Network { .. } if error.is_connect_failure() => true,
        Error::Network { .. } | Error::Server { .. } => mode == RetryMode::Idempotent,
        _ => false,
    }
}

fn backoff_ceiling(policy: &RetryPolicy, attempt: u32) -> Duration {
    let factor = 2_u32.saturating_pow(attempt);
    policy
        .initial_backoff
        .saturating_mul(factor)
        .min(policy.max_backoff)
}

fn full_jitter(ceiling: Duration) -> Duration {
    let ceiling_nanos = u64::try_from(ceiling.as_nanos()).unwrap_or(u64::MAX);
    Duration::from_nanos(fastrand::u64(0..=ceiling_nanos))
}

#[cfg(test)]
mod tests;
