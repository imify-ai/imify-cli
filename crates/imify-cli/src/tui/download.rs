//! The live view of `imify policies download`: the upload view's layout, fed with download
//! events.

use ratatui::Frame;
use ratatui::text::Text;

use super::theme::Theme;
use super::upload::UploadView;
use super::view::View;
use crate::commands::datasets::upload::{UploadProgress, UploadStep};
use crate::commands::policies::download::{DownloadProgress, DownloadStep};

/// The live view of a download: phase, total progress with rate and time left, file count and
/// the files in flight.
#[derive(Debug)]
pub struct DownloadView(UploadView);

impl DownloadView {
    /// A view of the download of `name` into the directory `target`, `total_bytes` bytes in
    /// total.
    pub fn new(
        name: impl Into<String>,
        target: impl Into<String>,
        total_bytes: u64,
        theme: Theme,
    ) -> Self {
        Self(UploadView::download(name, target, total_bytes, theme))
    }
}

impl View for DownloadView {
    type Event = DownloadProgress;

    fn height(&self) -> u16 {
        self.0.height()
    }

    fn apply(&mut self, event: DownloadProgress) {
        let at = event.at;
        let step = match event.step {
            DownloadStep::Resumed {
                files_done,
                bytes_done,
            } => return self.0.resumed(at, files_done, bytes_done),
            DownloadStep::Finished => return self.0.end(at, true),
            DownloadStep::Failed => return self.0.end(at, false),
            DownloadStep::ManifestRefreshed => return,
            DownloadStep::FileStarted { path, size } => UploadStep::FileStarted { path, size },
            DownloadStep::FileProgress {
                path,
                bytes_received,
            } => UploadStep::FileProgress {
                path,
                bytes_sent: bytes_received,
            },
            DownloadStep::FileRetrying {
                path,
                attempt,
                reason,
            } => UploadStep::FileRetrying {
                path,
                attempt,
                reason,
            },
            DownloadStep::FileFinished { path, size } => UploadStep::FileFinished { path, size },
            DownloadStep::RateLimited { pause } => UploadStep::RateLimited { pause },
        };
        self.0.apply(UploadProgress { at, step });
    }

    fn render(&self, frame: &mut Frame) {
        self.0.render(frame);
    }

    fn summary(&self) -> Text<'static> {
        self.0.summary()
    }
}

#[cfg(test)]
mod tests;
