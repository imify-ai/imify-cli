//! Behavior of the live view, the driver and the theme, with a fixture view on a test backend.

use std::cell::RefCell;
use std::convert::Infallible;
use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;
use std::time::Duration;

use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Constraint, Layout, Position, Rect, Size};
use ratatui::style::Color;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::LineGauge;
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc;

use super::live::{active_viewport, write_restore};
use super::theme::Theme;
use super::*;
use crate::output::CliError;

/// Renders `view` into a test backend `width` columns wide and returns the rows as text, with
/// trailing spaces removed.
pub fn render_to_text(view: &impl View, width: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, view.height())).unwrap();
    terminal.draw(|frame| view.render(frame)).unwrap();
    buffer_text(terminal.backend().buffer())
}

/// The rows of `buffer` as text, with trailing spaces removed.
pub fn buffer_text(buffer: &Buffer) -> String {
    let area = buffer.area;
    (area.top()..area.bottom())
        .map(|y| {
            let row: String = (area.left()..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            row.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A `TestBackend` the test keeps a handle to after the live view takes ownership.
#[derive(Clone)]
struct SharedBackend {
    terminal: Rc<RefCell<TestBackend>>,
    /// The number of cursor-position queries still answered; `None` answers every query.
    cursor_answers: Rc<std::cell::Cell<Option<usize>>>,
}

/// The error of a terminal that does not answer the cursor-position query.
const NO_CURSOR_REPORT: &str = "the terminal did not report the cursor position within 250 ms";

impl SharedBackend {
    fn new(width: u16, height: u16) -> Self {
        Self {
            terminal: Rc::new(RefCell::new(TestBackend::new(width, height))),
            cursor_answers: Rc::default(),
        }
    }

    /// A backend that answers `answers` cursor-position queries, then fails every query.
    fn answering_cursor_queries(answers: usize, width: u16, height: u16) -> Self {
        let backend = Self::new(width, height);
        backend.cursor_answers.set(Some(answers));
        backend
    }

    fn text(&self) -> String {
        buffer_text(self.terminal.borrow().buffer())
    }

    fn cursor_visible(&self) -> bool {
        self.terminal.borrow().cursor_visible()
    }

    fn cursor_position(&self) -> Position {
        self.terminal.borrow().cursor_position()
    }
}

fn infallible(error: Infallible) -> io::Error {
    match error {}
}

impl Backend for SharedBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.terminal.borrow_mut().draw(content).map_err(infallible)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.terminal
            .borrow_mut()
            .append_lines(n)
            .map_err(infallible)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.terminal.borrow_mut().hide_cursor().map_err(infallible)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.terminal.borrow_mut().show_cursor().map_err(infallible)
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        match self.cursor_answers.get() {
            Some(0) => return Err(io::Error::new(io::ErrorKind::TimedOut, NO_CURSOR_REPORT)),
            Some(answers) => self.cursor_answers.set(Some(answers - 1)),
            None => {}
        }
        self.terminal
            .borrow_mut()
            .get_cursor_position()
            .map_err(infallible)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.terminal
            .borrow_mut()
            .set_cursor_position(position)
            .map_err(infallible)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.terminal.borrow_mut().clear().map_err(infallible)
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.terminal
            .borrow_mut()
            .clear_region(clear_type)
            .map_err(infallible)
    }

    fn size(&self) -> io::Result<Size> {
        self.terminal.borrow().size().map_err(infallible)
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.terminal.borrow_mut().window_size().map_err(infallible)
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut *self.terminal.borrow_mut()).map_err(infallible)
    }
}

/// One file of the fixture operation finished uploading.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileUploaded {
    path: &'static str,
}

impl Render for FileUploaded {
    fn kind(&self) -> &'static str {
        "file_uploaded"
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({ "path": self.path })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "uploaded {}", self.path)
    }
}

/// What the fixture view saw, readable after the view moved into the driver.
#[derive(Debug, Default)]
struct Record {
    applied: Vec<&'static str>,
    frames: usize,
    applied_at_summary: Option<usize>,
}

/// A view with a phase line, a progress bar and the two most recent files.
struct UploadFixture {
    theme: Theme,
    total: usize,
    done: Vec<&'static str>,
    record: Rc<RefCell<Record>>,
}

impl UploadFixture {
    fn new(total: usize) -> Self {
        Self {
            theme: Theme::new(true),
            total,
            done: Vec::new(),
            record: Rc::default(),
        }
    }

    fn with_files(total: usize, paths: &[&'static str]) -> Self {
        let mut view = Self::new(total);
        for path in paths {
            view.apply(FileUploaded { path });
        }
        view
    }
}

impl View for UploadFixture {
    type Event = FileUploaded;

    fn height(&self) -> u16 {
        4
    }

    fn apply(&mut self, event: FileUploaded) {
        self.record.borrow_mut().applied.push(event.path);
        self.done.push(event.path);
    }

    fn render(&self, frame: &mut Frame) {
        self.record.borrow_mut().frames += 1;
        let [phase, bar, first, second] =
            Layout::vertical([Constraint::Length(1); 4]).areas::<4>(frame.area());
        frame.render_widget(
            Line::from(vec![
                Span::styled("Uploading", self.theme.accent),
                Span::styled(
                    format!(" {} of {} files", self.done.len(), self.total),
                    self.theme.muted,
                ),
            ]),
            phase,
        );
        #[expect(clippy::cast_precision_loss, reason = "fixture file counts are small")]
        let ratio = self.done.len() as f64 / self.total as f64;
        frame.render_widget(
            LineGauge::default()
                .ratio(ratio)
                .filled_style(self.theme.bar_filled)
                .unfilled_style(self.theme.bar_empty),
            bar,
        );
        for (row, path) in [first, second].into_iter().zip(self.done.iter().rev()) {
            frame.render_widget(
                Line::from(vec![
                    Span::styled("done ", self.theme.success),
                    Span::raw(*path),
                ]),
                row,
            );
        }
    }

    fn summary(&self) -> Text<'static> {
        self.record.borrow_mut().applied_at_summary = Some(self.done.len());
        Text::from(vec![
            Line::styled(
                format!("Uploaded {} of {} files", self.done.len(), self.total),
                self.theme.accent,
            ),
            Line::styled("dataset 42", self.theme.muted),
        ])
    }
}

const PATHS: [&str; 3] = [
    "data/chunk-000/file-000.parquet",
    "videos/observation.images.front/chunk-000/file-000.mp4",
    "meta/episodes/chunk-000/file-000.parquet",
];

fn snapshot(name: &str, text: &str) {
    insta::with_settings!({ snapshot_path => "../../tests/snapshots" }, {
        insta::assert_snapshot!(name, text);
    });
}

// Theme

#[test]
fn theme_without_color_has_no_foreground_or_background_color() {
    let theme = Theme::new(false);

    for style in theme.styles() {
        assert_eq!((style.fg, style.bg), (None, None), "{style:?}");
    }
}

#[test]
fn theme_with_color_uses_named_ansi_colors_only() {
    let theme = Theme::new(true);

    let colors = theme
        .styles()
        .into_iter()
        .flat_map(|style| [style.fg, style.bg])
        .flatten();
    for color in colors {
        assert!(
            !matches!(color, Color::Rgb(..) | Color::Indexed(_) | Color::Reset),
            "{color:?} is not a named ANSI color"
        );
    }
}

// Frames

#[test]
fn fixture_view_renders_at_80_columns() {
    let view = UploadFixture::with_files(10, &PATHS);

    snapshot("fixture_80_columns", &render_to_text(&view, 80));
}

#[test]
fn fixture_view_renders_at_40_columns() {
    let view = UploadFixture::with_files(10, &PATHS);

    snapshot("fixture_40_columns", &render_to_text(&view, 40));
}

#[test]
fn finish_leaves_the_summary_above_a_visible_cursor_and_blank_rows_below() {
    let backend = SharedBackend::new(50, 8);
    let mut live = LiveView::new(backend.clone(), UploadFixture::with_files(10, &PATHS)).unwrap();
    live.draw_if_dirty().unwrap();

    live.finish().unwrap();

    let text = backend.text();
    snapshot("finish_summary", &text);
    let rows: Vec<&str> = text.split('\n').collect();
    assert_eq!(rows.len(), 8);
    assert!(rows[2..].iter().all(|row| row.is_empty()), "{rows:?}");
    assert!(backend.cursor_visible());
    assert_eq!(backend.cursor_position(), Position::new(0, 2));
}

// Driver

fn live(
    view: UploadFixture,
) -> (
    Progress<'static, SharedBackend, UploadFixture>,
    SharedBackend,
) {
    let backend = SharedBackend::new(60, 10);
    let live = LiveView::new(backend.clone(), view).unwrap();
    (Progress::Live(live), backend)
}

async fn never_interrupted() -> io::Result<()> {
    std::future::pending().await
}

async fn interrupted_after(delay: Duration) -> io::Result<()> {
    tokio::time::sleep(delay).await;
    Ok(())
}

/// Sets its flag when dropped.
struct DropFlag(Rc<RefCell<bool>>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        *self.0.borrow_mut() = true;
    }
}

#[tokio::test(start_paused = true)]
async fn events_are_applied_in_the_order_they_were_sent() {
    let view = UploadFixture::new(3);
    let record = Rc::clone(&view.record);
    let (progress, _) = live(view);
    let (tx, rx) = mpsc::unbounded_channel();
    let operation = async move {
        for path in PATHS {
            tx.send(FileUploaded { path }).unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    };

    run(progress, rx, operation, never_interrupted())
        .await
        .unwrap();

    assert_eq!(record.borrow().applied, PATHS);
}

#[tokio::test(start_paused = true)]
async fn a_burst_of_100_events_in_one_second_draws_at_most_10_tick_frames() {
    let view = UploadFixture::new(100);
    let record = Rc::clone(&view.record);
    let (progress, _) = live(view);
    let (tx, rx) = mpsc::unbounded_channel();
    let operation = async move {
        for _ in 0..100 {
            tx.send(FileUploaded { path: PATHS[0] }).unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };

    run(progress, rx, operation, never_interrupted())
        .await
        .unwrap();

    let first_frame = 1;
    assert_eq!(record.borrow().applied.len(), 100);
    assert!(record.borrow().frames - first_frame <= 10, "{record:?}");
}

#[tokio::test(start_paused = true)]
async fn no_frame_is_drawn_on_a_tick_without_a_new_event() {
    let view = UploadFixture::new(3);
    let record = Rc::clone(&view.record);
    let (progress, _) = live(view);
    let (_tx, rx) = mpsc::unbounded_channel();

    run(
        progress,
        rx,
        tokio::time::sleep(Duration::from_secs(1)),
        never_interrupted(),
    )
    .await
    .unwrap();

    assert_eq!(record.borrow().frames, 1);
}

#[tokio::test(start_paused = true)]
async fn events_still_queued_when_the_operation_finishes_are_applied_before_the_summary() {
    let view = UploadFixture::new(3);
    let record = Rc::clone(&view.record);
    let (progress, _) = live(view);
    let (tx, rx) = mpsc::unbounded_channel();
    let operation = async move {
        for path in PATHS {
            tx.send(FileUploaded { path }).unwrap();
        }
    };

    run(progress, rx, operation, never_interrupted())
        .await
        .unwrap();

    assert_eq!(record.borrow().applied_at_summary, Some(3));
}

#[tokio::test(start_paused = true)]
async fn a_finished_operation_returns_its_output() {
    let (progress, _) = live(UploadFixture::new(3));
    let (_tx, rx) = mpsc::unbounded_channel();

    let outcome = run(progress, rx, async { 42 }, never_interrupted())
        .await
        .unwrap();

    assert_eq!(outcome, Outcome::Finished(42));
}

#[tokio::test(start_paused = true)]
async fn an_interrupt_returns_interrupted_and_drops_the_operation() {
    let (progress, _) = live(UploadFixture::new(3));
    let (_tx, rx) = mpsc::unbounded_channel();
    let dropped = Rc::new(RefCell::new(false));
    let flag = DropFlag(Rc::clone(&dropped));
    let operation = async move {
        let _flag = flag;
        std::future::pending::<()>().await;
    };

    let outcome = run(
        progress,
        rx,
        operation,
        interrupted_after(Duration::from_millis(250)),
    )
    .await
    .unwrap();

    assert_eq!(outcome, Outcome::Interrupted);
    assert!(*dropped.borrow());
}

#[tokio::test(start_paused = true)]
async fn an_interrupt_in_live_mode_writes_the_summary_and_shows_the_cursor() {
    let (progress, backend) = live(UploadFixture::new(3));
    let (_tx, rx) = mpsc::unbounded_channel();

    run(
        progress,
        rx,
        std::future::pending::<()>(),
        interrupted_after(Duration::from_millis(250)),
    )
    .await
    .unwrap();

    assert!(
        backend
            .text()
            .starts_with("Uploaded 0 of 3 files\ndataset 42")
    );
    assert!(backend.cursor_visible());
}

/// Records the events written through it as their human-readable lines, and the warnings.
#[derive(Debug, Default)]
struct RecordingRenderer {
    lines: Vec<u8>,
    warnings: Vec<String>,
}

impl Renderer for RecordingRenderer {
    fn result(&mut self, value: &dyn Render) -> io::Result<()> {
        value.human(&mut self.lines)
    }

    fn event(&mut self, event: &dyn Render) -> io::Result<()> {
        event.human(&mut self.lines)
    }

    fn error(&mut self, error: &CliError) -> io::Result<()> {
        writeln!(self.lines, "error: {}", error.message)
    }

    fn warning(&mut self, message: &str) -> io::Result<()> {
        self.warnings.push(message.to_owned());
        Ok(())
    }

    fn notice(&mut self, message: &str) -> io::Result<()> {
        writeln!(self.lines, "{message}")
    }
}

#[tokio::test(start_paused = true)]
async fn in_lines_mode_each_event_goes_through_the_renderer() {
    let mut renderer = RecordingRenderer::default();
    let (tx, rx) = mpsc::unbounded_channel();
    let operation = async move {
        for path in &PATHS[..2] {
            tx.send(FileUploaded { path }).unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    };

    run(
        Progress::<SharedBackend, UploadFixture>::Lines(&mut renderer),
        rx,
        operation,
        never_interrupted(),
    )
    .await
    .unwrap();

    assert_eq!(
        String::from_utf8(renderer.lines).unwrap(),
        "uploaded data/chunk-000/file-000.parquet\n\
         uploaded videos/observation.images.front/chunk-000/file-000.mp4\n"
    );
}

#[tokio::test(start_paused = true)]
async fn in_lines_mode_an_interrupt_returns_interrupted() {
    let mut renderer = RecordingRenderer::default();
    let (_tx, rx) = mpsc::unbounded_channel();

    let outcome = run(
        Progress::<SharedBackend, UploadFixture>::Lines(&mut renderer),
        rx,
        std::future::pending::<()>(),
        interrupted_after(Duration::from_millis(250)),
    )
    .await
    .unwrap();

    assert_eq!(outcome, Outcome::Interrupted);
    assert_eq!(String::from_utf8(renderer.lines).unwrap(), "");
}

#[tokio::test(start_paused = true)]
async fn an_error_from_the_interrupt_future_is_returned() {
    let (progress, backend) = live(UploadFixture::new(3));
    let (_tx, rx) = mpsc::unbounded_channel();
    let interrupt = async { Err(io::Error::other("no signal handler")) };

    let error = run(progress, rx, std::future::pending::<()>(), interrupt)
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), "no signal handler");
    assert!(backend.cursor_visible());
}

// Restore

#[test]
fn dropping_a_live_view_during_a_panic_shows_the_cursor_below_the_viewport() {
    let backend = SharedBackend::new(40, 10);
    let view = UploadFixture::new(3);
    let height = view.height();

    let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut live = LiveView::new(backend.clone(), view).unwrap();
        live.draw_if_dirty().unwrap();
        panic::resume_unwind(Box::new("unwinding"));
    }));

    assert!(unwound.is_err());
    assert!(backend.cursor_visible());
    assert_eq!(backend.cursor_position(), Position::new(0, height));
}

#[test]
fn panic_hook_restore_sequence_shows_the_cursor_and_ends_on_the_line_below_the_viewport() {
    let mut written = Vec::new();

    write_restore(&mut written, Rect::new(0, 2, 40, 4)).unwrap();

    // Show the cursor, move to row 6 (1-based), the viewport's last row, then a line feed.
    assert_eq!(written, b"\x1b[?25h\x1b[6;1H\r\n");
}

#[test]
fn panic_hook_restores_below_the_inline_viewport_of_the_last_frame() {
    let backend = SharedBackend::new(40, 10);
    backend
        .terminal
        .borrow_mut()
        .set_cursor_position((0, 3))
        .unwrap();
    let mut live = LiveView::new(backend, UploadFixture::new(3)).unwrap();
    live.register_for_panic_hook();

    live.draw_if_dirty().unwrap();
    let viewport = active_viewport();
    live.finish().unwrap();

    assert_eq!(viewport, Some(Rect::new(0, 3, 40, 4)));
}

// Fallback

#[test]
fn opening_on_a_backend_whose_cursor_query_fails_returns_no_live_view() {
    let backend = SharedBackend::answering_cursor_queries(0, 60, 10);
    let mut renderer = RecordingRenderer::default();

    let live = live_or_warn(LiveView::new(backend, UploadFixture::new(3)), &mut renderer).unwrap();

    assert!(live.is_none());
}

#[test]
fn a_failed_open_writes_one_warning_naming_the_error() {
    let backend = SharedBackend::answering_cursor_queries(0, 60, 10);
    let mut renderer = RecordingRenderer::default();

    live_or_warn(LiveView::new(backend, UploadFixture::new(3)), &mut renderer).unwrap();

    assert_eq!(
        renderer.warnings,
        [format!(
            "The live view cannot start: {NO_CURSOR_REPORT}. Progress is shown as lines."
        )]
    );
}

#[tokio::test(start_paused = true)]
async fn after_a_failed_open_the_operation_runs_and_each_event_goes_through_the_renderer() {
    let backend = SharedBackend::answering_cursor_queries(0, 60, 10);
    let mut renderer = RecordingRenderer::default();
    let live = live_or_warn(LiveView::new(backend, UploadFixture::new(3)), &mut renderer).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    let operation = async move {
        tx.send(FileUploaded { path: PATHS[0] }).unwrap();
        42
    };

    let outcome = run(
        Progress::new(live, &mut renderer),
        rx,
        operation,
        never_interrupted(),
    )
    .await
    .unwrap();

    assert_eq!(
        (outcome, String::from_utf8(renderer.lines).unwrap()),
        (
            Outcome::Finished(42),
            "uploaded data/chunk-000/file-000.parquet\n".to_owned()
        )
    );
}

#[tokio::test(start_paused = true)]
async fn a_cursor_query_that_fails_on_resize_returns_the_error_and_shows_the_cursor() {
    let backend = SharedBackend::answering_cursor_queries(1, 60, 10);
    let mut renderer = RecordingRenderer::default();
    let live = live_or_warn(
        LiveView::new(backend.clone(), UploadFixture::new(3)),
        &mut renderer,
    )
    .unwrap();
    backend.terminal.borrow_mut().resize(80, 12);
    let (_tx, rx) = mpsc::unbounded_channel();

    let error = run(
        Progress::new(live, &mut renderer),
        rx,
        std::future::pending::<()>(),
        never_interrupted(),
    )
    .await
    .unwrap_err();

    assert_eq!(
        (
            error.kind(),
            backend.cursor_visible(),
            renderer.warnings.len()
        ),
        (io::ErrorKind::TimedOut, true, 0)
    );
}
