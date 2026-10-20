//! Spacing of request starts shared by the workers of an upload or a download.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::time::{Instant, sleep_until};

/// Spaces request starts by a minimum interval and holds all starts during a pause.
#[derive(Debug)]
pub(crate) struct Pacer {
    interval: Duration,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    next_start: Instant,
    paused_until: Instant,
}

impl Pacer {
    /// Creates a pacer that lets one request start per `interval`. `Duration::ZERO`
    /// spaces nothing; pauses still apply.
    pub(crate) fn new(interval: Duration) -> Self {
        let now = Instant::now();
        Self {
            interval,
            state: Mutex::new(State {
                next_start: now,
                paused_until: now,
            }),
        }
    }

    /// Waits until the caller may start a request.
    pub(crate) async fn ready(&self) {
        loop {
            let start = self.reserve();
            if start > Instant::now() {
                sleep_until(start).await;
            }
            if Instant::now() >= self.state().paused_until {
                return;
            }
        }
    }

    /// Holds every request start for `duration` from now.
    pub(crate) fn pause(&self, duration: Duration) {
        let until = Instant::now() + duration;
        let mut state = self.state();
        state.paused_until = state.paused_until.max(until);
    }

    fn reserve(&self) -> Instant {
        let mut state = self.state();
        let start = Instant::now().max(state.next_start).max(state.paused_until);
        state.next_start = start + self.interval;
        start
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests;
