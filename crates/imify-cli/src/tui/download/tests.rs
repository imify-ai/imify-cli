//! Frames and summaries of the download view, driven by recorded events.

use std::time::Duration;

use ratatui::text::Text;

use super::*;
use crate::tui::tests::render_to_text;

const MIB: u64 = 1024 * 1024;
const TOTAL_BYTES: u64 = 1229 * MIB;
const MODEL: &str = "pretrained_model/model.safetensors";
const OPTIMIZER: &str = "training_state/optimizer_state.safetensors";
const CONFIG: &str = "pretrained_model/config.json";

fn view() -> DownloadView {
    DownloadView::new(
        "run-3c9e1a7f 020000",
        "run-3c9e1a7f-020000",
        TOTAL_BYTES,
        Theme::new(false),
    )
}

fn at(seconds: f64, step: DownloadStep) -> DownloadProgress {
    DownloadProgress {
        at: Duration::from_secs_f64(seconds),
        step,
    }
}

fn started(seconds: f64, path: &str, size: u64) -> DownloadProgress {
    at(
        seconds,
        DownloadStep::FileStarted {
            path: path.to_owned(),
            size,
        },
    )
}

fn received(seconds: f64, path: &str, bytes_received: u64) -> DownloadProgress {
    at(
        seconds,
        DownloadStep::FileProgress {
            path: path.to_owned(),
            bytes_received,
        },
    )
}

fn finished(seconds: f64, path: &str, size: u64) -> DownloadProgress {
    at(
        seconds,
        DownloadStep::FileFinished {
            path: path.to_owned(),
            size,
        },
    )
}

fn text(summary: &Text<'_>) -> String {
    summary
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn snapshot(name: &str, text: &str) {
    insta::with_settings!({ snapshot_path => "../../../tests/snapshots" }, {
        insta::assert_snapshot!(name, text);
    });
}

/// One file finished after a resume of one file, two files in flight.
fn downloading() -> DownloadView {
    let mut view = view();
    for event in [
        at(
            0.2,
            DownloadStep::Resumed {
                files_done: 1,
                bytes_done: 4096,
            },
        ),
        started(0.5, CONFIG, 2048),
        finished(0.6, CONFIG, 2048),
        started(0.6, MODEL, 800 * MIB),
        started(0.6, OPTIMIZER, 429 * MIB),
        received(2.0, MODEL, 100 * MIB),
        received(4.0, MODEL, 300 * MIB),
        received(4.0, OPTIMIZER, 120 * MIB),
    ] {
        view.apply(event);
    }
    view
}

#[test]
fn download_frame_at_80_columns() {
    snapshot(
        "download_downloading_80",
        &render_to_text(&downloading(), 80),
    );
}

#[test]
fn download_frame_at_40_columns() {
    snapshot(
        "download_downloading_40",
        &render_to_text(&downloading(), 40),
    );
}

#[test]
fn summary_after_success_names_files_size_duration_and_directory() {
    let mut view = downloading();
    for event in [
        finished(9.0, MODEL, 800 * MIB),
        finished(9.5, OPTIMIZER, 429 * MIB),
        at(9.5, DownloadStep::Finished),
    ] {
        view.apply(event);
    }

    assert_eq!(
        text(&view.summary()),
        "Downloaded 4 files (1.2 GiB) in 9s to run-3c9e1a7f-020000."
    );
}

#[test]
fn summary_after_a_failure_counts_the_finished_files() {
    let mut view = downloading();
    view.apply(at(5.0, DownloadStep::Failed));

    assert_eq!(text(&view.summary()), "Failed after 2 files.");
}

#[test]
fn summary_after_an_interrupt_says_how_to_continue() {
    assert_eq!(
        text(&downloading().summary()),
        "Interrupted after 2 files. Run the same command again to continue."
    );
}
