//! Live progress of long-running commands: an inline viewport on stderr, or plain event lines.
//!
//! [`run`] drives an operation and its progress events. With [`Progress::Live`], a [`View`]
//! draws the events into an inline viewport at most 10 times per second and leaves its summary
//! in the scrollback. With [`Progress::Lines`], each event goes through the [`Renderer`].
//!
//! A live view that cannot open, for example because the terminal does not report the cursor
//! position within [`cursor::QUERY_TIMEOUT`], falls back to lines: [`live_or_warn`] writes one
//! warning and [`Progress::new`] selects [`Progress::Lines`].

pub mod cursor;
pub mod format;
pub mod live;
pub mod theme;
pub mod upload;
pub mod view;

#[cfg(test)]
mod tests;

use std::future::Future;
use std::io;
use std::pin::pin;
use std::time::Duration;

use ratatui::backend::Backend;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{self, MissedTickBehavior};

use self::live::LiveView;
use self::view::View;
use crate::output::renderer::{Render, Renderer};

/// The interval between frames of a live view.
pub const TICK: Duration = Duration::from_millis(100);

/// Where [`run`] reports progress events.
pub enum Progress<'r, B: Backend<Error = io::Error>, V: View> {
    /// Draws the events into an inline viewport.
    Live(LiveView<B, V>),
    /// Writes each event as a plain line through the renderer: text on stderr, NDJSON with
    /// `--json`, nothing with `--quiet`.
    Lines(&'r mut dyn Renderer),
}

impl<B: Backend<Error = io::Error>, V: View> std::fmt::Debug for Progress<'_, B, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Live(live) => f.debug_tuple("Live").field(live).finish(),
            Self::Lines(_) => f.debug_tuple("Lines").finish_non_exhaustive(),
        }
    }
}

impl<'r, B: Backend<Error = io::Error>, V: View> Progress<'r, B, V> {
    /// [`Progress::Live`] for `Some`, [`Progress::Lines`] through `renderer` for `None`.
    pub fn new(live: Option<LiveView<B, V>>, renderer: &'r mut dyn Renderer) -> Self {
        match live {
            Some(live) => Self::Live(live),
            None => Self::Lines(renderer),
        }
    }
}

/// Returns the opened live view, or writes one warning naming `error` through `renderer` and
/// returns `None`, so that the progress is shown as lines.
///
/// ```text
/// warning: The live view cannot start: the terminal did not report the cursor position within 250 ms. Progress is shown as lines.
/// ```
///
/// # Errors
///
/// Returns the error of writing the warning.
pub fn live_or_warn<B: Backend<Error = io::Error>, V: View>(
    opened: io::Result<LiveView<B, V>>,
    renderer: &mut dyn Renderer,
) -> io::Result<Option<LiveView<B, V>>> {
    match opened {
        Ok(live) => Ok(Some(live)),
        Err(error) => {
            renderer.warning(&format!(
                "The live view cannot start: {error}. Progress is shown as lines."
            ))?;
            Ok(None)
        }
    }
}

impl<B, V> Progress<'_, B, V>
where
    B: Backend<Error = io::Error>,
    V: View,
    V::Event: Render,
{
    fn apply(&mut self, event: V::Event) -> io::Result<()> {
        match self {
            Self::Live(live) => {
                live.apply(event);
                Ok(())
            }
            Self::Lines(renderer) => renderer.event(&event),
        }
    }

    fn draw(&mut self) -> io::Result<()> {
        match self {
            Self::Live(live) => live.draw_if_dirty(),
            Self::Lines(_) => Ok(()),
        }
    }

    fn finish(self) -> io::Result<()> {
        match self {
            Self::Live(live) => live.finish().map(drop),
            Self::Lines(_) => Ok(()),
        }
    }
}

/// How [`run`] ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    /// The operation finished with this output.
    Finished(T),
    /// The interrupt resolved first and the operation was dropped.
    Interrupted,
}

/// Runs `operation` until it finishes or `interrupt` resolves, and reports the events from
/// `events` in the order they were sent.
///
/// A live view draws a first frame at start, then at most one frame per [`TICK`] and only when
/// an event arrived since the last frame. On interrupt, `operation` is dropped, which cancels
/// its work. Events still queued at the end are applied. In both cases the live view ends with
/// its summary.
///
/// # Errors
///
/// Returns the error of the interrupt future, of the terminal or of the renderer. The terminal
/// is restored before the error is returned.
pub async fn run<B, V, T>(
    mut progress: Progress<'_, B, V>,
    mut events: UnboundedReceiver<V::Event>,
    operation: impl Future<Output = T>,
    interrupt: impl Future<Output = io::Result<()>>,
) -> io::Result<Outcome<T>>
where
    B: Backend<Error = io::Error>,
    V: View,
    V::Event: Render,
{
    // Boxed so that it can be dropped before the summary is written.
    let mut operation = Box::pin(operation);
    let mut interrupt = pin!(interrupt);
    let mut tick = time::interval(TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut events_open = true;

    progress.draw()?;
    let outcome = loop {
        tokio::select! {
            biased;
            result = &mut interrupt => {
                result?;
                break Outcome::Interrupted;
            }
            output = &mut operation => break Outcome::Finished(output),
            _ = tick.tick() => progress.draw()?,
            event = events.recv(), if events_open => match event {
                Some(event) => progress.apply(event)?,
                None => events_open = false,
            },
        }
    };
    drop(operation);

    while let Ok(event) = events.try_recv() {
        progress.apply(event)?;
    }
    progress.finish()?;
    Ok(outcome)
}

/// Resolves on the first interrupt signal (Ctrl-C).
///
/// # Errors
///
/// Returns the error of installing the signal handler.
pub async fn ctrl_c() -> io::Result<()> {
    tokio::signal::ctrl_c().await
}
