use tokio::time::sleep;

use super::*;

const LIMIT: Duration = Duration::from_millis(100);

/// Takes `chunks` chunks with `gap` between them, then waits `response_after` for the
/// response, like an HTTP stack sending a body.
async fn send_body(
    activity: &Activity,
    chunks: u32,
    gap: Duration,
    response_after: Duration,
) -> Result<(), Error> {
    for _ in 0..chunks {
        sleep(gap).await;
        activity.touch();
    }
    sleep(response_after).await;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn body_that_keeps_moving_is_not_cut() {
    let activity = Activity::new();

    let result = watch(
        LIMIT,
        &activity,
        send_body(&activity, 25, Duration::from_millis(40), Duration::ZERO),
    )
    .await;

    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test(start_paused = true)]
async fn body_that_stalls_fails_with_retryable_network_error() {
    let activity = Activity::new();

    let error = watch(
        LIMIT,
        &activity,
        send_body(&activity, 2, Duration::from_millis(150), Duration::ZERO),
    )
    .await
    .unwrap_err();

    assert!(crate::retry::should_retry(
        &crate::RetryPolicy::default(),
        crate::retry::RetryMode::Idempotent,
        &error,
        0
    ));
}

#[tokio::test(start_paused = true)]
async fn missing_response_after_the_last_chunk_fails_with_network_error() {
    let activity = Activity::new();

    let error = watch(
        LIMIT,
        &activity,
        send_body(
            &activity,
            3,
            Duration::from_millis(10),
            Duration::from_millis(150),
        ),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, Error::Network { .. }));
}
