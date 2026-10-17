//! The inline viewport a view draws into, and the restore of the terminal when it ends.

use std::io::{self, BufWriter, Stderr, Write};
use std::panic;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, Once, PoisonError, TryLockError};

use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::crossterm::cursor::{MoveTo, Show};
use ratatui::crossterm::{queue, style::Print};
use ratatui::layout::{Position, Rect, Size};
use ratatui::text::Text;
use ratatui::widgets::Widget;
use ratatui::{Terminal, TerminalOptions, Viewport};

use super::cursor::{self, DevTty};
use super::view::View;

/// The backend of a live view on the process's stderr.
///
/// Writes through crossterm. The cursor position is queried on the controlling terminal
/// with [`cursor::query_position`] and [`cursor::QUERY_TIMEOUT`], so the query never writes to
/// stdout and fails with `TimedOut` when the terminal does not answer.
pub struct StderrBackend {
    inner: CrosstermBackend<BufWriter<Stderr>>,
}

impl std::fmt::Debug for StderrBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StderrBackend").finish_non_exhaustive()
    }
}

impl StderrBackend {
    fn new() -> Self {
        Self {
            inner: CrosstermBackend::new(BufWriter::new(io::stderr())),
        }
    }
}

impl Backend for StderrBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        // The terminal reports the position after it processed the output written before.
        Backend::flush(&mut self.inner)?;
        let mut tty = DevTty::open()?;
        cursor::query_position(&mut tty, cursor::QUERY_TIMEOUT)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

/// A view drawn into an inline viewport on `B`.
///
/// The cursor is hidden while the view is active. [`LiveView::finish`] replaces the viewport
/// with the view's summary. When a `LiveView` is dropped without `finish`, including during a
/// panic, it shows the cursor and moves it to the line below the viewport.
pub struct LiveView<B: Backend<Error = io::Error>, V: View> {
    screen: Screen<B>,
    view: V,
    dirty: bool,
}

impl<B: Backend<Error = io::Error>, V: View> std::fmt::Debug for LiveView<B, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveView")
            .field("dirty", &self.dirty)
            .finish_non_exhaustive()
    }
}

impl<V: View> LiveView<StderrBackend, V> {
    /// Opens the viewport on stderr at the cursor's row.
    ///
    /// Also installs a panic hook that, while a live view is active, shows the cursor and moves
    /// it below the viewport before the panic message is printed.
    ///
    /// # Errors
    ///
    /// Returns the error of querying or writing the terminal: `TimedOut` when the terminal does
    /// not report the cursor position within [`cursor::QUERY_TIMEOUT`].
    pub fn stderr(view: V) -> io::Result<Self> {
        install_panic_hook();
        let mut live = Self::new(StderrBackend::new(), view)?;
        live.register_for_panic_hook();
        Ok(live)
    }
}

impl<B: Backend<Error = io::Error>, V: View> LiveView<B, V> {
    /// Opens the viewport on `backend` at the cursor's row, [`View::height`] rows high.
    ///
    /// # Errors
    ///
    /// Returns the error of the backend.
    pub fn new(backend: B, view: V) -> io::Result<Self> {
        let options = TerminalOptions {
            viewport: Viewport::Inline(view.height()),
        };
        let mut terminal = Terminal::with_options(backend, options)?;
        terminal.hide_cursor()?;
        Ok(Self {
            screen: Screen {
                terminal,
                registration: None,
                ended: false,
            },
            view,
            dirty: true,
        })
    }

    /// Makes this view the one the panic hook restores.
    pub(super) fn register_for_panic_hook(&mut self) {
        let viewport = self.screen.area();
        self.screen.registration = Some(Registration::register(viewport));
    }

    /// Applies `event` to the view. The change is drawn by the next [`LiveView::draw_if_dirty`].
    pub fn apply(&mut self, event: V::Event) {
        self.view.apply(event);
        self.dirty = true;
    }

    /// Draws the view when its state changed since the last frame. The first call always draws.
    ///
    /// # Errors
    ///
    /// Returns the error of the backend.
    pub fn draw_if_dirty(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let view = &self.view;
        self.screen.terminal.draw(|frame| view.render(frame))?;
        let viewport = self.screen.area();
        if let Some(registration) = &self.screen.registration {
            registration.update(viewport);
        }
        self.dirty = false;
        Ok(())
    }

    /// Replaces the viewport with [`View::summary`] and leaves the cursor visible on the line
    /// below the summary. Returns the view.
    ///
    /// # Errors
    ///
    /// Returns the error of the backend. The terminal is then restored as on drop.
    pub fn finish(mut self) -> io::Result<V> {
        self.screen.end(&self.view.summary())?;
        Ok(self.view)
    }
}

/// The terminal of a live view. Restores the cursor on drop unless the view ended.
struct Screen<B: Backend<Error = io::Error>> {
    terminal: Terminal<B>,
    registration: Option<Registration>,
    ended: bool,
}

impl<B: Backend<Error = io::Error>> Screen<B> {
    fn area(&mut self) -> Rect {
        self.terminal.get_frame().area()
    }

    fn end(&mut self, summary: &Text<'_>) -> io::Result<()> {
        let height = u16::try_from(summary.height()).unwrap_or(u16::MAX);
        if height > 0 {
            self.terminal
                .insert_before(height, |buffer| summary.render(buffer.area, buffer))?;
        }
        self.terminal.clear()?;
        self.terminal
            .draw(|frame| frame.set_cursor_position(frame.area().as_position()))?;
        if let Some(registration) = self.registration.take() {
            registration.release();
        }
        self.ended = true;
        Ok(())
    }

    /// Shows the cursor and moves it to the first column of the line below the viewport.
    fn restore(&mut self) -> io::Result<()> {
        let area = self.area();
        self.terminal.show_cursor()?;
        self.terminal
            .set_cursor_position((0, area.bottom().saturating_sub(1)))?;
        // A line feed on the last row scrolls the screen, so the line below always exists.
        self.terminal.backend_mut().append_lines(1)?;
        let last_screen_row = self.terminal.size()?.height.saturating_sub(1);
        self.terminal
            .set_cursor_position((0, area.bottom().min(last_screen_row)))?;
        self.terminal.backend_mut().flush()
    }
}

impl<B: Backend<Error = io::Error>> Drop for Screen<B> {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        let restored_by_panic_hook = self
            .registration
            .take()
            .is_some_and(|registration| !registration.release());
        if !restored_by_panic_hook {
            // A drop cannot return an error, and no other channel is left to report it on.
            let _ = self.restore();
        }
    }
}

/// The active live view on stderr and its viewport, for the panic hook.
static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(0);
static PANIC_HOOK: Once = Once::new();

#[derive(Clone, Copy)]
struct Active {
    id: u64,
    viewport: Rect,
}

fn active() -> MutexGuard<'static, Option<Active>> {
    ACTIVE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The viewport the panic hook restores, if a live view is registered.
#[cfg(test)]
pub(super) fn active_viewport() -> Option<Rect> {
    active().map(|active| active.viewport)
}

/// Marks a live view on stderr as the active one for the panic hook.
struct Registration {
    id: u64,
}

impl Registration {
    fn register(viewport: Rect) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        *active() = Some(Active { id, viewport });
        Self { id }
    }

    fn update(&self, viewport: Rect) {
        if let Some(active) = active().as_mut().filter(|active| active.id == self.id) {
            active.viewport = viewport;
        }
    }

    /// Deactivates the view. Returns `false` when the panic hook already restored the terminal.
    fn release(self) -> bool {
        let mut active = active();
        let owned = active.is_some_and(|active| active.id == self.id);
        if owned {
            *active = None;
        }
        owned
    }
}

fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            // `try_lock`: the panicking thread may hold the lock.
            let active = match ACTIVE.try_lock() {
                Ok(mut active) => active.take(),
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner().take(),
                Err(TryLockError::WouldBlock) => None,
            };
            if let Some(active) = active {
                // The panic message follows; a failed restore must not hide it.
                let _ = write_restore(&mut io::stderr(), active.viewport);
            }
            previous(info);
        }));
    });
}

/// Writes the sequence that shows the cursor and moves it to the line below `viewport`.
pub fn write_restore(w: &mut impl Write, viewport: Rect) -> io::Result<()> {
    queue!(
        w,
        Show,
        MoveTo(0, viewport.bottom().saturating_sub(1)),
        Print("\r\n")
    )?;
    w.flush()
}
