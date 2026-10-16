//! Behavior, frames and summaries of the upload view, driven by recorded events.

use std::time::Duration;

use ratatui::text::Text;

use super::*;
use crate::commands::datasets::upload::{UploadProgress, UploadStep};
use crate::tui::tests::render_to_text;

const MIB: u64 = 1024 * 1024;
const TOTAL_FILES: usize = 8;
const TOTAL_BYTES: u64 = 2048 + 1410 * MIB;

fn view() -> UploadView {
    UploadView::new("pick-cube", TOTAL_FILES, TOTAL_BYTES, Theme::new(false))
}

fn at(seconds: f64, step: UploadStep) -> UploadProgress {
    UploadProgress {
        at: Duration::from_secs_f64(seconds),
        step,
    }
}

fn session_created(seconds: f64) -> UploadProgress {
    at(
        seconds,
        UploadStep::SessionCreated {
            session: "3f2c9a".to_owned(),
        },
    )
}

fn started(seconds: f64, path: &str, size: u64) -> UploadProgress {
    at(
        seconds,
        UploadStep::FileStarted {
            path: path.to_owned(),
            size,
        },
    )
}

fn sent(seconds: f64, path: &str, bytes_sent: u64) -> UploadProgress {
    at(
        seconds,
        UploadStep::FileProgress {
            path: path.to_owned(),
            bytes_sent,
        },
    )
}

fn retrying(seconds: f64, path: &str) -> UploadProgress {
    at(
        seconds,
        UploadStep::FileRetrying {
            path: path.to_owned(),
            attempt: 1,
            reason: "server error 503: Service Unavailable".to_owned(),
        },
    )
}

fn finished(seconds: f64, path: &str, size: u64) -> UploadProgress {
    at(
        seconds,
        UploadStep::FileFinished {
            path: path.to_owned(),
            size,
        },
    )
}

fn apply_all(view: &mut UploadView, events: impl IntoIterator<Item = UploadProgress>) {
    for event in events {
        view.apply(event);
    }
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

const DATA_0: &str = "data/chunk-000/file-000.parquet";
const DATA_1: &str = "data/chunk-000/file-001.parquet";
const VIDEO_2: &str = "videos/observation.images.front/chunk-000/episode_000002.mp4";

/// Five files finished, three in flight, one retry, 18 MiB/s over the last seconds.
fn uploading() -> UploadView {
    let mut view = view();
    apply_all(
        &mut view,
        [
            session_created(0.5),
            started(1.0, "meta/info.json", 2048),
            finished(1.2, "meta/info.json", 2048),
        ],
    );
    for (index, path) in [
        "meta/tasks.parquet",
        "meta/stats.json",
        "meta/episodes.parquet",
        "data/a.parquet",
    ]
    .into_iter()
    .enumerate()
    {
        #[expect(clippy::cast_precision_loss, reason = "small test indexes")]
        let seconds = 2.0 + index as f64;
        view.apply(started(seconds, path, 150 * MIB));
        view.apply(finished(seconds + 0.5, path, 150 * MIB));
    }
    apply_all(
        &mut view,
        [
            started(10.0, DATA_0, 300 * MIB),
            started(10.0, DATA_1, 300 * MIB),
            started(10.0, VIDEO_2, 210 * MIB),
            sent(11.0, DATA_0, 20 * MIB),
            retrying(11.5, DATA_1),
            sent(15.0, DATA_0, 90 * MIB),
            sent(15.0, DATA_1, 30 * MIB),
            sent(15.0, VIDEO_2, 40 * MIB),
        ],
    );
    view
}

// State

#[test]
fn bytes_done_are_finished_sizes_plus_bytes_sent_of_files_in_flight() {
    let mut view = view();

    apply_all(
        &mut view,
        [
            started(0.0, DATA_0, 100),
            finished(1.0, DATA_0, 100),
            started(1.0, DATA_1, 50),
            sent(2.0, DATA_1, 20),
        ],
    );

    assert_eq!(view.bytes_done(), 120);
}

#[test]
fn retrying_resets_the_bytes_of_the_file_and_counts_the_retry() {
    let mut view = view();

    apply_all(
        &mut view,
        [
            started(0.0, DATA_0, 100),
            sent(1.0, DATA_0, 60),
            retrying(2.0, DATA_0),
        ],
    );

    assert_eq!((view.bytes_done(), view.retries()), (0, 1));
}

#[test]
fn finished_file_leaves_the_rows_and_adds_its_size() {
    let mut view = view();

    apply_all(
        &mut view,
        [
            started(0.0, DATA_0, 100),
            started(0.0, DATA_1, 50),
            finished(1.0, DATA_0, 100),
        ],
    );

    assert_eq!(view.files_in_flight(), [DATA_1]);
    assert_eq!(view.bytes_done(), 100);
}

#[test]
fn throughput_is_measured_over_the_last_5_seconds_of_event_times() {
    let mut view = view();

    apply_all(
        &mut view,
        [
            started(0.0, DATA_0, 1000 * MIB),
            sent(1.0, DATA_0, 100 * MIB),
            sent(10.0, DATA_0, 110 * MIB),
            sent(15.0, DATA_0, 160 * MIB),
        ],
    );

    // 50 MiB between 10 s and 15 s; the sample at 1 s is outside the window.
    assert_eq!(view.rate(), 10.0 * 1024.0 * 1024.0);
}

#[test]
fn no_time_left_is_shown_while_the_rate_is_0() {
    let mut view = view();

    apply_all(
        &mut view,
        [session_created(0.5), started(1.0, DATA_0, 100 * MIB)],
    );

    assert!(!render_to_text(&view, 80).contains("left"));
}

#[test]
fn rate_limited_shows_the_pause_until_the_next_event_after_the_pause_ends() {
    let mut view = view();
    apply_all(
        &mut view,
        [
            session_created(0.5),
            at(
                1.0,
                UploadStep::RateLimited {
                    pause: Duration::from_secs(3),
                },
            ),
        ],
    );
    let paused = render_to_text(&view, 80);
    view.apply(sent(2.0, DATA_0, 10));
    let during_pause = render_to_text(&view, 80);
    view.apply(started(4.0, DATA_1, 10));
    let after_pause = render_to_text(&view, 80);

    assert!(paused.starts_with("Paused (rate limited, 3s)"), "{paused}");
    assert!(during_pause.starts_with("Paused"), "{during_pause}");
    assert!(after_pause.starts_with("Uploading"), "{after_pause}");
}

#[test]
fn more_files_in_flight_than_rows_show_the_first_rows_and_a_count_of_the_rest() {
    let mut view = view();

    apply_all(
        &mut view,
        (0..6).map(|index| started(1.0, &format!("data/file-{index}.parquet"), 10)),
    );

    let frame = render_to_text(&view, 80);
    assert!(frame.contains("data/file-2.parquet"), "{frame}");
    assert!(!frame.contains("data/file-3.parquet"), "{frame}");
    assert!(frame.ends_with("+3 more"), "{frame}");
}

// Frames

#[test]
fn creating_session_frame() {
    snapshot("upload_creating_session_80", &render_to_text(&view(), 80));
}

#[test]
fn uploading_frame_with_three_files_in_flight_and_one_retry_at_80_columns() {
    snapshot("upload_uploading_80", &render_to_text(&uploading(), 80));
}

#[test]
fn uploading_frame_with_three_files_in_flight_and_one_retry_at_40_columns() {
    snapshot("upload_uploading_40", &render_to_text(&uploading(), 40));
}

#[test]
fn rate_limited_frame() {
    let mut view = uploading();
    view.apply(at(
        16.0,
        UploadStep::RateLimited {
            pause: Duration::from_secs(3),
        },
    ));

    snapshot("upload_rate_limited_80", &render_to_text(&view, 80));
}

#[test]
fn completing_frame_at_80_and_40_columns() {
    let mut view = uploading();
    apply_all(
        &mut view,
        [
            finished(20.0, DATA_0, 300 * MIB),
            finished(21.0, DATA_1, 300 * MIB),
            finished(22.0, VIDEO_2, 210 * MIB),
            at(22.5, UploadStep::Completing),
        ],
    );

    snapshot("upload_completing_80", &render_to_text(&view, 80));
    snapshot("upload_completing_40", &render_to_text(&view, 40));
}

// Summaries

#[test]
fn summary_after_success_names_files_size_duration_and_session() {
    let mut view = view();
    apply_all(
        &mut view,
        [
            session_created(0.5),
            started(1.0, DATA_0, TOTAL_BYTES),
            finished(70.0, DATA_0, TOTAL_BYTES),
            at(71.0, UploadStep::Completing),
            at(72.4, UploadStep::Finished),
        ],
    );

    snapshot("upload_summary_success", &text(&view.summary()));
}

#[test]
fn summary_after_an_interrupt_says_the_session_stays_open() {
    snapshot("upload_summary_interrupted", &text(&uploading().summary()));
}

#[test]
fn summary_after_an_interrupt_before_the_session_exists_names_no_session() {
    assert_eq!(text(&view().summary()), "Interrupted after 0 of 8 files.");
}

#[test]
fn summary_after_a_failure_counts_the_finished_files() {
    let mut view = uploading();
    view.apply(at(16.0, UploadStep::Failed));

    snapshot("upload_summary_failed", &text(&view.summary()));
}

// Resume

fn session_resumed(seconds: f64, files_done: usize, bytes_done: u64) -> UploadProgress {
    at(
        seconds,
        UploadStep::SessionResumed {
            session: "3f2c9a".to_owned(),
            files_done,
            bytes_done,
        },
    )
}

#[test]
fn resumed_session_counts_the_files_and_bytes_already_uploaded_as_done() {
    let mut view = view();

    view.apply(session_resumed(0.5, 5, 600 * MIB));

    let frame = render_to_text(&view, 80);
    assert_eq!(view.bytes_done(), 600 * MIB);
    assert!(frame.contains("5/8 files"), "{frame}");
    assert!(
        frame.starts_with("Uploading  pick-cube  session 3f2c9a"),
        "{frame}"
    );
}

#[test]
fn bytes_already_uploaded_do_not_count_toward_the_rate() {
    let mut view = view();

    apply_all(
        &mut view,
        [
            session_resumed(0.0, 5, 600 * MIB),
            started(1.0, DATA_0, 300 * MIB),
            sent(2.0, DATA_0, 20 * MIB),
        ],
    );

    // 20 MiB between the resume at 0 s and 2 s.
    assert_eq!(view.rate(), 10.0 * 1024.0 * 1024.0);
}

#[test]
fn resumed_frame_with_one_file_in_flight() {
    let mut view = view();
    apply_all(
        &mut view,
        [
            session_resumed(0.5, 5, 600 * MIB),
            started(1.0, DATA_0, 300 * MIB),
            sent(3.0, DATA_0, 60 * MIB),
        ],
    );

    snapshot("upload_resumed_80", &render_to_text(&view, 80));
}

#[test]
fn summary_after_a_resumed_upload_counts_the_files_of_the_whole_session() {
    let mut view = view();
    apply_all(
        &mut view,
        [
            session_resumed(0.5, 7, TOTAL_BYTES - 300 * MIB),
            started(1.0, DATA_0, 300 * MIB),
            finished(10.0, DATA_0, 300 * MIB),
            at(10.5, UploadStep::Completing),
            at(11.0, UploadStep::Finished),
        ],
    );

    assert_eq!(
        text(&view.summary()),
        "Uploaded 8 files (1.4 GiB) in 11s to ingest session 3f2c9a."
    );
}
