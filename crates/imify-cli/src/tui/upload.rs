//! The live view of `imify datasets upload`, also used by `imify policies download`.

use std::collections::VecDeque;
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span, Text};

use super::format;
use super::theme::Theme;
use super::view::View;
use crate::commands::datasets::upload::{UploadProgress, UploadStep};

/// Rows of the view: phase, progress bar, file count, then one row per file in flight.
const HEIGHT: u16 = 7;
const FILE_ROWS: usize = 4;
/// Time span of the events the transfer rate is measured over.
const RATE_WINDOW: Duration = Duration::from_secs(5);
const BAR_MIN_WIDTH: usize = 10;
const BAR_MAX_WIDTH: usize = 30;
/// Columns a file row keeps for the path before it drops the sizes.
const PATH_MIN_WIDTH: usize = 20;
const GAP: &str = "  ";
const INDENT: &str = "  ";

/// What the view shows: an upload into an ingest session, or a download into a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Transfer {
    Upload,
    /// A download into the directory `target`.
    Download {
        target: String,
    },
}

/// What the transfer is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    CreatingSession,
    Transferring,
    Completing,
    Finished { at: Duration },
    Failed,
}

/// A rate limit pause: `pause` long, ending at `until` since the start of the upload.
#[derive(Debug, Clone, Copy)]
struct Pause {
    pause: Duration,
    until: Duration,
}

#[derive(Debug, Clone)]
struct FileInFlight {
    path: String,
    size: u64,
    sent: u64,
}

/// The live view of an upload or a download: phase, total progress with rate and time left,
/// file count and the files in flight.
#[derive(Debug)]
pub struct UploadView {
    theme: Theme,
    transfer: Transfer,
    name: String,
    /// `None` when the number of files is not known before the transfer.
    total_files: Option<usize>,
    total_bytes: u64,
    session: Option<String>,
    phase: Phase,
    pause: Option<Pause>,
    in_flight: Vec<FileInFlight>,
    finished_files: usize,
    finished_bytes: u64,
    retries: u32,
    /// `(event time, bytes done)` of the events within [`RATE_WINDOW`] of the latest one.
    samples: VecDeque<(Duration, u64)>,
}

impl UploadView {
    /// A view of the upload of `name`: `total_files` files of `total_bytes` bytes in total.
    pub fn new(
        name: impl Into<String>,
        total_files: usize,
        total_bytes: u64,
        theme: Theme,
    ) -> Self {
        Self {
            theme,
            transfer: Transfer::Upload,
            name: name.into(),
            total_files: Some(total_files),
            total_bytes,
            session: None,
            phase: Phase::CreatingSession,
            pause: None,
            in_flight: Vec::new(),
            finished_files: 0,
            finished_bytes: 0,
            retries: 0,
            samples: VecDeque::new(),
        }
    }

    /// A view of the download of `name` into the directory `target`: `total_bytes` bytes in an
    /// unknown number of files.
    pub fn download(
        name: impl Into<String>,
        target: impl Into<String>,
        total_bytes: u64,
        theme: Theme,
    ) -> Self {
        Self {
            transfer: Transfer::Download {
                target: target.into(),
            },
            total_files: None,
            phase: Phase::Transferring,
            ..Self::new(name, 0, total_bytes, theme)
        }
    }

    /// Counts `files_done` files of `bytes_done` bytes as finished before the transfer.
    pub fn resumed(&mut self, at: Duration, files_done: usize, bytes_done: u64) {
        self.finished_files += files_done;
        self.finished_bytes += bytes_done;
        self.record_sample(at);
    }

    /// Ends the transfer: finished at `at` when `succeeded`, failed otherwise.
    pub fn end(&mut self, at: Duration, succeeded: bool) {
        self.phase = if succeeded {
            Phase::Finished { at }
        } else {
            Phase::Failed
        };
    }

    /// Bytes of finished files plus bytes sent of the files in flight.
    pub fn bytes_done(&self) -> u64 {
        self.finished_bytes + self.in_flight.iter().map(|file| file.sent).sum::<u64>()
    }

    #[cfg(test)]
    pub fn retries(&self) -> u32 {
        self.retries
    }

    #[cfg(test)]
    pub fn files_in_flight(&self) -> Vec<&str> {
        self.in_flight
            .iter()
            .map(|file| file.path.as_str())
            .collect()
    }

    /// Bytes per second between the first and the last event within [`RATE_WINDOW`]; 0 when
    /// they are at the same time or no bytes were sent between them.
    pub fn rate(&self) -> f64 {
        let (Some(&(first_at, first_done)), Some(&(last_at, last_done))) =
            (self.samples.front(), self.samples.back())
        else {
            return 0.0;
        };
        let seconds = last_at.saturating_sub(first_at).as_secs_f64();
        if seconds == 0.0 {
            return 0.0;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a rate is shown with one decimal"
        )]
        let bytes = last_done.saturating_sub(first_done) as f64;
        bytes / seconds
    }

    fn file_mut(&mut self, path: &str) -> Option<&mut FileInFlight> {
        self.in_flight.iter_mut().find(|file| file.path == path)
    }

    fn record_sample(&mut self, at: Duration) {
        let done = self.bytes_done();
        self.samples.push_back((at, done));
        while self
            .samples
            .front()
            .is_some_and(|&(sample_at, _)| sample_at + RATE_WINDOW < at)
        {
            self.samples.pop_front();
        }
    }

    fn phase_line(&self) -> Line<'static> {
        let label = match (self.phase, self.pause) {
            (Phase::Transferring, Some(pause)) => {
                format!("Paused (rate limited, {})", format::duration(pause.pause))
            }
            (Phase::CreatingSession, _) => "Creating session".to_owned(),
            (Phase::Transferring, None) => match self.transfer {
                Transfer::Upload => "Uploading".to_owned(),
                Transfer::Download { .. } => "Downloading".to_owned(),
            },
            (Phase::Completing | Phase::Finished { .. }, _) => "Completing".to_owned(),
            (Phase::Failed, _) => "Failed".to_owned(),
        };
        let mut spans = vec![
            Span::styled(label, self.theme.accent),
            Span::raw(format!("{GAP}{}", self.name)),
        ];
        if let Some(session) = &self.session {
            spans.push(Span::styled(
                format!("{GAP}session {session}"),
                self.theme.muted,
            ));
        }
        Line::from(spans)
    }

    fn progress_line(&self, width: usize) -> Line<'static> {
        let done = self.bytes_done();
        let rate = self.rate();
        let mut parts = vec![
            format!(
                "{} / {}",
                format::bytes(done),
                format::bytes(self.total_bytes)
            ),
            format!("{}%", percent(done, self.total_bytes)),
        ];
        if rate > 0.0 {
            parts.push(format::rate(rate));
            #[expect(
                clippy::cast_precision_loss,
                reason = "time left is shown in whole seconds"
            )]
            let remaining = self.total_bytes.saturating_sub(done) as f64;
            if remaining > 0.0
                && let Ok(left) = Duration::try_from_secs_f64(remaining / rate)
            {
                parts.push(format!("{} left", format::duration(left)));
            }
        }
        let mut text = String::new();
        for part in parts {
            let separator = if text.is_empty() { "" } else { GAP };
            if text.chars().count() + separator.len() + part.chars().count() > width {
                break;
            }
            text.push_str(separator);
            text.push_str(&part);
        }

        let bar_width = width
            .saturating_sub(text.chars().count() + "[] ".len() + 1)
            .min(BAR_MAX_WIDTH);
        if bar_width < BAR_MIN_WIDTH {
            return Line::raw(text);
        }
        let filled = filled_cells(done, self.total_bytes, bar_width);
        Line::from(vec![
            Span::raw("["),
            Span::styled("█".repeat(filled), self.theme.bar_filled),
            Span::styled("░".repeat(bar_width - filled), self.theme.bar_empty),
            Span::raw("]  "),
            Span::raw(text),
        ])
    }

    fn count_line(&self) -> Line<'static> {
        let files = match self.total_files {
            Some(total) => format!("{}/{total} files", self.finished_files),
            None => files(self.finished_files),
        };
        let text = match self.retries {
            0 => files,
            1 => format!("{files}{GAP}1 retry"),
            n => format!("{files}{GAP}{n} retries"),
        };
        Line::styled(text, self.theme.muted)
    }

    fn file_lines(&self, width: usize) -> Vec<Line<'static>> {
        let shown = if self.in_flight.len() > FILE_ROWS {
            FILE_ROWS - 1
        } else {
            self.in_flight.len()
        };
        let mut lines: Vec<Line<'static>> = self.in_flight[..shown]
            .iter()
            .map(|file| file_line(file, width))
            .collect();
        if shown < self.in_flight.len() {
            lines.push(Line::styled(
                format!("{INDENT}+{} more", self.in_flight.len() - shown),
                self.theme.muted,
            ));
        }
        lines
    }
}

impl UploadView {
    /// The summary line of a download into `target`.
    fn download_summary(&self, target: &str) -> Line<'static> {
        let done = files(self.finished_files);
        match self.phase {
            Phase::Finished { at } => Line::styled(
                format!(
                    "Downloaded {done} ({}) in {} to {target}.",
                    format::bytes(self.finished_bytes),
                    format::duration(at),
                ),
                self.theme.success,
            ),
            Phase::Failed => Line::styled(format!("Failed after {done}."), self.theme.error),
            Phase::CreatingSession | Phase::Transferring | Phase::Completing => Line::styled(
                format!("Interrupted after {done}. Run the same command again to continue."),
                self.theme.warning,
            ),
        }
    }
}

/// `done` as a whole percentage of `total`, rounded down; 100 when `total` is 0.
fn percent(done: u64, total: u64) -> u64 {
    if total == 0 {
        return 100;
    }
    let percent = u128::from(done.min(total)) * 100 / u128::from(total);
    u64::try_from(percent).unwrap_or(100)
}

/// The filled cells of a bar `width` cells wide for `done` of `total`.
fn filled_cells(done: u64, total: u64, width: usize) -> usize {
    if total == 0 {
        return width;
    }
    let cells = u128::from(done.min(total)) * width as u128 / u128::from(total);
    usize::try_from(cells).unwrap_or(width)
}

/// `  <path>  <sent> / <size>`, the path truncated in the middle to fit `width`. When the path
/// would get fewer than [`PATH_MIN_WIDTH`] columns, the percentage replaces the sizes.
fn file_line(file: &FileInFlight, width: usize) -> Line<'static> {
    let path_width =
        |progress: &str| width.saturating_sub(INDENT.len() + GAP.len() + progress.chars().count());
    let mut progress = format!(
        "{} / {}",
        format::bytes(file.sent),
        format::bytes(file.size)
    );
    if path_width(&progress) < PATH_MIN_WIDTH {
        progress = format!("{}%", percent(file.sent, file.size));
    }
    let path_width = path_width(&progress);
    let path = format::truncate_middle(&file.path, path_width);
    let padding = path_width.saturating_sub(path.chars().count());
    Line::raw(format!(
        "{INDENT}{path}{}{GAP}{progress}",
        " ".repeat(padding)
    ))
}

/// `n` followed by `file` or `files`.
fn files(n: usize) -> String {
    if n == 1 {
        "1 file".to_owned()
    } else {
        format!("{n} files")
    }
}

impl View for UploadView {
    type Event = UploadProgress;

    fn height(&self) -> u16 {
        HEIGHT
    }

    fn apply(&mut self, event: UploadProgress) {
        if self.pause.is_some_and(|pause| event.at >= pause.until) {
            self.pause = None;
        }
        match event.step {
            UploadStep::SessionCreated { session } => {
                self.session = Some(session);
                self.phase = Phase::Transferring;
            }
            UploadStep::SessionResumed {
                session,
                files_done,
                bytes_done,
            } => {
                self.session = Some(session);
                self.phase = Phase::Transferring;
                self.finished_files += files_done;
                self.finished_bytes += bytes_done;
            }
            UploadStep::FileStarted { path, size } => self.in_flight.push(FileInFlight {
                path,
                size,
                sent: 0,
            }),
            UploadStep::FileProgress { path, bytes_sent } => {
                if let Some(file) = self.file_mut(&path) {
                    file.sent = bytes_sent;
                }
            }
            UploadStep::FileRetrying { path, .. } => {
                self.retries += 1;
                if let Some(file) = self.file_mut(&path) {
                    file.sent = 0;
                }
            }
            UploadStep::FileFinished { path, size } => {
                self.in_flight.retain(|file| file.path != path);
                self.finished_files += 1;
                self.finished_bytes += size;
            }
            UploadStep::RateLimited { pause } => {
                self.pause = Some(Pause {
                    pause,
                    until: event.at + pause,
                });
            }
            UploadStep::Completing => self.phase = Phase::Completing,
            UploadStep::Finished => self.phase = Phase::Finished { at: event.at },
            UploadStep::Failed => self.phase = Phase::Failed,
        }
        self.record_sample(event.at);
    }

    fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        let width = usize::from(area.width);
        let rows = Layout::vertical([Constraint::Length(1); HEIGHT as usize]).split(area);
        let mut lines = vec![
            self.phase_line(),
            self.progress_line(width),
            self.count_line(),
        ];
        lines.extend(self.file_lines(width));
        for (line, row) in lines.into_iter().zip(rows.iter()) {
            frame.render_widget(line, *row);
        }
    }

    fn summary(&self) -> Text<'static> {
        if let Transfer::Download { target } = &self.transfer {
            return Text::from(self.download_summary(target));
        }
        let total_files = self.total_files.unwrap_or_default();
        let line = match self.phase {
            Phase::Finished { at } => Line::styled(
                format!(
                    "Uploaded {} ({}) in {} to ingest session {}.",
                    files(self.finished_files),
                    format::bytes(self.finished_bytes),
                    format::duration(at),
                    self.session.as_deref().unwrap_or_default()
                ),
                self.theme.success,
            ),
            Phase::Failed => Line::styled(
                format!(
                    "Failed after {} of {}.",
                    self.finished_files,
                    files(total_files)
                ),
                self.theme.error,
            ),
            Phase::CreatingSession | Phase::Transferring | Phase::Completing => {
                let interrupted = format!(
                    "Interrupted after {} of {}.",
                    self.finished_files,
                    files(total_files)
                );
                let text = match &self.session {
                    Some(session) => {
                        format!("{interrupted} Ingest session {session} stays open.")
                    }
                    None => interrupted,
                };
                Line::styled(text, self.theme.warning)
            }
        };
        Text::from(line)
    }
}

#[cfg(test)]
mod tests;
