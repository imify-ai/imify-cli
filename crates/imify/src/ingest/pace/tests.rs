use std::sync::Arc;

use futures_util::future::join_all;

use super::*;

const INTERVAL: Duration = Duration::from_millis(100);

async fn start_times(pacer: Arc<Pacer>, callers: usize) -> Vec<Duration> {
    let origin = Instant::now();
    let mut times: Vec<Duration> = join_all((0..callers).map(|_| {
        let pacer = Arc::clone(&pacer);
        async move {
            pacer.ready().await;
            origin.elapsed()
        }
    }))
    .await;
    times.sort();
    times
}

#[tokio::test(start_paused = true)]
async fn request_starts_are_at_least_the_interval_apart() {
    let times = start_times(Arc::new(Pacer::new(INTERVAL)), 4).await;

    assert!(
        times
            .windows(2)
            .all(|pair| pair[1].saturating_sub(pair[0]) >= INTERVAL),
        "{times:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn zero_interval_does_not_delay() {
    let times = start_times(Arc::new(Pacer::new(Duration::ZERO)), 4).await;

    assert!(times.iter().all(Duration::is_zero), "{times:?}");
}

#[tokio::test(start_paused = true)]
async fn pause_holds_every_caller_until_it_ends() {
    let pacer = Arc::new(Pacer::new(Duration::ZERO));
    pacer.pause(Duration::from_secs(2));

    let times = start_times(pacer, 3).await;

    assert!(
        times.iter().all(|time| *time >= Duration::from_secs(2)),
        "{times:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn pause_holds_a_caller_that_is_already_waiting() {
    let pacer = Arc::new(Pacer::new(Duration::from_secs(1)));
    pacer.ready().await;
    let origin = Instant::now();
    let waiting = {
        let pacer = Arc::clone(&pacer);
        tokio::spawn(async move {
            pacer.ready().await;
            origin.elapsed()
        })
    };
    tokio::time::sleep(Duration::from_millis(10)).await;

    pacer.pause(Duration::from_secs(3));

    assert!(waiting.await.unwrap() >= Duration::from_millis(3010));
}
