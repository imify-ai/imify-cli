//! `imify policies download`: downloads a checkpoint of a policy into a directory.

use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use imify::download::{DownloadEvent, DownloadSource, DownloadSummary, Downloader};
use imify::policies::{Checkpoint, PolicyId};
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::cli::PoliciesDownloadArgs;
use crate::commands::client;
use crate::output::renderer::{RESULT_EVENT, Render, Renderer};
use crate::output::sanitize::printable;
use crate::output::{CliError, Environment, ErrorCode, Settings};
use crate::tui::download::DownloadView;
use crate::tui::live::{LiveView, StderrBackend};
use crate::tui::theme::Theme;
use crate::tui::{self, Outcome, Progress, format};

/// The checkpoint argument that selects the checkpoint with the highest step.
const LATEST: &str = "latest";

/// Width of the key column of the human result.
const KEY_WIDTH: usize = 12;

/// `imify policies download <policy> <checkpoint>`: resolves the checkpoint, downloads its
/// files into the target directory and writes the result.
///
/// With `latest`, writes the `checkpoint_resolved` event first. The progress goes to the live
/// view, or as events through `renderer` without it.
///
/// # Errors
///
/// Returns `not_found` when the policy, the checkpoint or any checkpoint is missing,
/// `invalid_argument` when the default target directory would not be a plain directory name,
/// `conflict` when the target directory holds files that are not part of the checkpoint,
/// `io` for a local file system error, `server` for an unusable manifest or a storage
/// failure, `interrupted` on Ctrl-C, and the mapped API error otherwise.
pub async fn download(
    env: &Environment,
    settings: &Settings,
    args: &PoliciesDownloadArgs,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    let api = client(env).await?;
    let policy = PolicyId::new(args.policy.as_str());
    let checkpoints = match api.client.policies().checkpoints(&policy).await {
        Ok(checkpoints) => checkpoints,
        Err(error) => return Err(api.error(error).await),
    };
    let checkpoint = resolve(&checkpoints, &args.policy, &args.checkpoint)?;
    if args.checkpoint == LATEST {
        write_event(
            renderer,
            &CheckpointResolved {
                policy: &args.policy,
                checkpoint: &checkpoint.id,
            },
        )?;
    }
    let target = match &args.output {
        Some(output) => output.clone(),
        None => default_target(&args.policy, &checkpoint.id)?,
    };

    let live = if settings.live_view {
        let view = DownloadView::new(
            format!("{} {}", args.policy, checkpoint.id),
            target.display().to_string(),
            checkpoint.size,
            Theme::new(settings.color),
        );
        tui::live_or_warn(LiveView::stderr(view), renderer)
            .map_err(|error| progress_error(&error))?
    } else {
        None
    };
    let source = DownloadSource::checkpoint(args.policy.as_str(), checkpoint.id.as_str());
    let downloader = Downloader::new(api.client.clone());
    let summary = match fetch(downloader, &source, &target, live, renderer).await {
        Ok(summary) => summary,
        Err(Failure::Sdk(error)) => return Err(api.error(error).await),
        Err(Failure::Cli(error)) => return Err(error),
    };
    write_event(
        renderer,
        &DownloadResult {
            policy: &args.policy,
            checkpoint: &checkpoint.id,
            summary: &summary,
        },
    )
}

/// The checkpoint `requested` of `policy` among `checkpoints`. `latest` selects the one with
/// the highest step: the highest id when every id is a number, otherwise the last listed one.
///
/// # Errors
///
/// Returns `not_found` when the policy has no checkpoints or none with the id `requested`.
fn resolve<'a>(
    checkpoints: &'a [Checkpoint],
    policy: &str,
    requested: &str,
) -> Result<&'a Checkpoint, CliError> {
    let found = if requested == LATEST {
        latest(checkpoints)
    } else {
        checkpoints
            .iter()
            .find(|checkpoint| checkpoint.id == requested)
    };
    found.ok_or_else(|| {
        let message = if checkpoints.is_empty() {
            format!("Policy {policy} has no checkpoints.")
        } else {
            format!("Policy {policy} has no checkpoint {requested}.")
        };
        CliError::new(ErrorCode::NotFound, message).with_hint(format!(
            "Run `imify policies show {policy}` to list its checkpoints."
        ))
    })
}

/// The checkpoint with the highest step: the highest id when every id is a number, otherwise
/// the last listed one.
fn latest(checkpoints: &[Checkpoint]) -> Option<&Checkpoint> {
    let steps: Option<Vec<u64>> = checkpoints
        .iter()
        .map(|checkpoint| checkpoint.id.parse().ok())
        .collect();
    match steps {
        Some(steps) => checkpoints
            .iter()
            .zip(steps)
            .max_by_key(|(_, step)| *step)
            .map(|(checkpoint, _)| checkpoint),
        None => checkpoints.last(),
    }
}

/// `./<policy>-<checkpoint>`.
///
/// # Errors
///
/// Returns `invalid_argument` when that name is not a single plain directory name, such as
/// for an id that contains `/`.
fn default_target(policy: &str, checkpoint: &str) -> Result<PathBuf, CliError> {
    let name = format!("{policy}-{checkpoint}");
    let mut components = Path::new(&name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(PathBuf::from(name)),
        _ => Err(CliError::new(
            ErrorCode::InvalidArgument,
            format!("{name} is not a plain directory name."),
        )
        .with_hint("Pass the target directory with `-o`.")),
    }
}

/// Why [`fetch`] failed: an SDK error the caller maps with the token's origin, or an error
/// of the CLI itself.
enum Failure {
    Sdk(imify::Error),
    Cli(CliError),
}

/// Downloads `source` into `target` and reports the progress in `live`, or as event lines
/// without it.
async fn fetch(
    downloader: Downloader,
    source: &DownloadSource,
    target: &Path,
    live: Option<LiveView<StderrBackend, DownloadView>>,
    renderer: &mut dyn Renderer,
) -> Result<DownloadSummary, Failure> {
    let start = Instant::now();
    let (events, received) = mpsc::unbounded_channel();
    let live_in_use = live.is_some();
    let forward = forwarder(events.clone(), start, live_in_use);
    let downloader = downloader.on_event(move |event| forward(event));
    let operation = async {
        let result = downloader.download(source, target).await;
        if live_in_use {
            let step = if result.is_ok() {
                DownloadStep::Finished
            } else {
                DownloadStep::Failed
            };
            send_progress(&events, start, step);
        }
        result
    };

    let outcome = tui::run(
        Progress::new(live, renderer),
        received,
        operation,
        tui::ctrl_c(),
    )
    .await
    .map_err(|error| Failure::Cli(progress_error(&error)))?;
    match outcome {
        Outcome::Finished(result) => result.map_err(Failure::Sdk),
        Outcome::Interrupted => Err(Failure::Cli(CliError::interrupted())),
    }
}

/// The event handler of the downloader: stamps each event with the time since `start` and
/// sends it to `events`. Without the live view, [`DownloadStep::FileProgress`] is dropped.
fn forwarder(
    events: UnboundedSender<DownloadProgress>,
    start: Instant,
    live: bool,
) -> impl Fn(&DownloadEvent) + Send + Sync + 'static {
    move |event| {
        if let Some(step) = DownloadStep::from_event(event)
            .filter(|step| live || !matches!(step, DownloadStep::FileProgress { .. }))
        {
            send_progress(&events, start, step);
        }
    }
}

fn send_progress(events: &UnboundedSender<DownloadProgress>, start: Instant, step: DownloadStep) {
    // Sending fails only after the progress loop ended; the event then has no reader.
    let _ = events.send(DownloadProgress {
        at: start.elapsed(),
        step,
    });
}

fn progress_error(error: &io::Error) -> CliError {
    CliError::new(
        ErrorCode::Internal,
        format!("Cannot write the download progress: {error}"),
    )
}

fn write_event(renderer: &mut dyn Renderer, event: &dyn Render) -> Result<(), CliError> {
    renderer.event(event).map_err(|error| {
        CliError::new(
            ErrorCode::Internal,
            format!("Cannot write the output: {error}"),
        )
    })
}

/// The `checkpoint_resolved` event: the checkpoint `latest` stands for.
struct CheckpointResolved<'a> {
    policy: &'a str,
    checkpoint: &'a str,
}

impl Render for CheckpointResolved<'_> {
    fn kind(&self) -> &'static str {
        "checkpoint_resolved"
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({ "checkpoint": self.checkpoint })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(
            w,
            "Latest checkpoint of {}: {}.",
            printable(self.policy),
            printable(self.checkpoint)
        )
    }
}

/// The `result` event of a completed download.
struct DownloadResult<'a> {
    policy: &'a str,
    checkpoint: &'a str,
    summary: &'a DownloadSummary,
}

impl Render for DownloadResult<'_> {
    fn kind(&self) -> &'static str {
        RESULT_EVENT
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "policy": self.policy,
            "checkpoint": self.checkpoint,
            "path": self.summary.target.display().to_string(),
            "files": self.summary.files,
            "bytes": self.summary.bytes,
            "resumed_bytes": self.summary.bytes_skipped,
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        let summary = self.summary;
        let mut rows = vec![
            ("policy", self.policy.to_owned()),
            ("checkpoint", self.checkpoint.to_owned()),
            ("path", summary.target.display().to_string()),
            (
                "files",
                format!("{} ({})", summary.files, format::bytes(summary.bytes)),
            ),
        ];
        if summary.files_skipped > 0 {
            rows.push((
                "resumed",
                format!(
                    "{} ({}) already present",
                    summary.files_skipped,
                    format::bytes(summary.bytes_skipped)
                ),
            ));
        }
        for (key, value) in rows {
            writeln!(w, "{key:KEY_WIDTH$}{}", printable(&value))?;
        }
        Ok(())
    }
}

/// One step of a download, stamped with the time since the download started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadProgress {
    /// Time since the download started.
    pub at: Duration,
    pub step: DownloadStep,
}

/// What happened during a download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadStep {
    /// `files_done` files of `bytes_done` bytes are complete in the target directory and
    /// not fetched again.
    Resumed {
        files_done: usize,
        bytes_done: u64,
    },
    FileStarted {
        path: String,
        size: u64,
    },
    /// Bytes of a file written so far. Sent to the live view only.
    FileProgress {
        path: String,
        bytes_received: u64,
    },
    FileRetrying {
        path: String,
        attempt: u32,
        reason: String,
    },
    FileFinished {
        path: String,
        size: u64,
    },
    RateLimited {
        pause: Duration,
    },
    /// Storage rejected an expired URL and the manifest was fetched again.
    ManifestRefreshed,
    /// The download is complete. Sent to the live view only.
    Finished,
    /// The download failed. Sent to the live view only.
    Failed,
}

impl DownloadStep {
    /// The step for an SDK event, or `None` for an event this CLI version does not know.
    pub fn from_event(event: &DownloadEvent) -> Option<Self> {
        let step = match event {
            DownloadEvent::Resumed {
                files_done,
                bytes_done,
            } => Self::Resumed {
                files_done: *files_done,
                bytes_done: *bytes_done,
            },
            DownloadEvent::FileStarted { path, size } => Self::FileStarted {
                path: path.clone(),
                size: *size,
            },
            DownloadEvent::FileProgress {
                path,
                bytes_received,
            } => Self::FileProgress {
                path: path.clone(),
                bytes_received: *bytes_received,
            },
            DownloadEvent::FileRetrying {
                path,
                attempt,
                reason,
            } => Self::FileRetrying {
                path: path.clone(),
                attempt: *attempt,
                reason: reason.clone(),
            },
            DownloadEvent::FileFinished { path, size } => Self::FileFinished {
                path: path.clone(),
                size: *size,
            },
            DownloadEvent::RateLimited { pause } => Self::RateLimited { pause: *pause },
            DownloadEvent::ManifestRefreshed => Self::ManifestRefreshed,
            _ => return None,
        };
        Some(step)
    }
}

impl Render for DownloadProgress {
    fn kind(&self) -> &'static str {
        match self.step {
            DownloadStep::Resumed { .. } => "resumed",
            DownloadStep::FileStarted { .. } => "file_started",
            DownloadStep::FileProgress { .. } => "file_progress",
            DownloadStep::FileRetrying { .. } => "file_retrying",
            DownloadStep::FileFinished { .. } => "file_finished",
            DownloadStep::RateLimited { .. } => "rate_limited",
            DownloadStep::ManifestRefreshed => "manifest_refreshed",
            DownloadStep::Finished => "finished",
            DownloadStep::Failed => "failed",
        }
    }

    fn json(&self) -> serde_json::Value {
        match &self.step {
            DownloadStep::Resumed {
                files_done,
                bytes_done,
            } => serde_json::json!({ "files_done": files_done, "bytes_done": bytes_done }),
            DownloadStep::FileStarted { path, size }
            | DownloadStep::FileFinished { path, size } => {
                serde_json::json!({ "path": path, "size": size })
            }
            DownloadStep::FileProgress {
                path,
                bytes_received,
            } => serde_json::json!({ "path": path, "bytes_received": bytes_received }),
            DownloadStep::FileRetrying {
                path,
                attempt,
                reason,
            } => serde_json::json!({ "path": path, "attempt": attempt, "reason": reason }),
            DownloadStep::RateLimited { pause } => {
                serde_json::json!({ "pause_ms": u64::try_from(pause.as_millis()).unwrap_or(u64::MAX) })
            }
            DownloadStep::ManifestRefreshed | DownloadStep::Finished | DownloadStep::Failed => {
                serde_json::json!({})
            }
        }
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        match &self.step {
            DownloadStep::Resumed {
                files_done,
                bytes_done,
            } => writeln!(
                w,
                "Resuming: {files_done} files ({}) already downloaded.",
                format::bytes(*bytes_done)
            ),
            DownloadStep::FileStarted { path, size } => writeln!(
                w,
                "Downloading {} ({}).",
                printable(path),
                format::bytes(*size)
            ),
            DownloadStep::FileProgress {
                path,
                bytes_received,
            } => writeln!(
                w,
                "Received {} of {}.",
                format::bytes(*bytes_received),
                printable(path)
            ),
            DownloadStep::FileRetrying {
                path,
                attempt,
                reason,
            } => writeln!(
                w,
                "Retrying {}, attempt {attempt}: {}.",
                printable(path),
                printable(reason)
            ),
            DownloadStep::FileFinished { path, .. } => {
                writeln!(w, "Downloaded {}.", printable(path))
            }
            DownloadStep::RateLimited { pause } => writeln!(
                w,
                "Rate limited: no request starts for {}.",
                format::duration(*pause)
            ),
            DownloadStep::ManifestRefreshed => writeln!(
                w,
                "The storage URLs expired; fetched new ones from the API."
            ),
            DownloadStep::Finished | DownloadStep::Failed => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoints(ids: &[&str]) -> Vec<Checkpoint> {
        let listing: Vec<serde_json::Value> = ids
            .iter()
            .map(|id| serde_json::json!({"id": id, "size": 1024, "last_modified": "2026-10-01T10:15:02Z"}))
            .collect();
        serde_json::from_value(serde_json::Value::Array(listing)).unwrap()
    }

    fn resolved(ids: &[&str], requested: &str) -> Result<String, CliError> {
        let checkpoints = checkpoints(ids);
        resolve(&checkpoints, "run-3c9e1a7f", requested).map(|checkpoint| checkpoint.id.clone())
    }

    #[test]
    fn latest_is_the_checkpoint_with_the_highest_step_compared_numerically() {
        assert_eq!(
            resolved(&["020000", "100000", "005000"], LATEST).unwrap(),
            "100000"
        );
    }

    #[test]
    fn latest_compares_steps_of_different_widths_as_numbers() {
        assert_eq!(resolved(&["9000", "10000"], LATEST).unwrap(), "10000");
    }

    #[test]
    fn latest_is_the_last_listed_checkpoint_when_an_id_is_not_a_number() {
        assert_eq!(
            resolved(&["step-020000", "005000", "final"], LATEST).unwrap(),
            "final"
        );
    }

    #[test]
    fn an_explicit_checkpoint_id_selects_that_checkpoint() {
        assert_eq!(resolved(&["010000", "020000"], "010000").unwrap(), "010000");
    }

    #[test]
    fn an_unknown_checkpoint_id_is_not_found() {
        let error = resolved(&["010000"], "030000").unwrap_err();

        assert_eq!(
            (error.code, error.message.as_str()),
            (
                ErrorCode::NotFound,
                "Policy run-3c9e1a7f has no checkpoint 030000."
            )
        );
    }

    #[test]
    fn latest_of_a_policy_without_checkpoints_is_not_found() {
        let error = resolved(&[], LATEST).unwrap_err();

        assert_eq!(
            (error.code, error.message.as_str()),
            (
                ErrorCode::NotFound,
                "Policy run-3c9e1a7f has no checkpoints."
            )
        );
    }

    #[test]
    fn default_target_is_the_policy_and_checkpoint_joined_by_a_hyphen() {
        assert_eq!(
            default_target("run-3c9e1a7f", "020000").unwrap(),
            PathBuf::from("run-3c9e1a7f-020000")
        );
    }

    #[test]
    fn default_target_refuses_an_id_that_is_not_a_plain_directory_name() {
        let error = default_target("run-3c9e1a7f", "x/../../etc").unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }
}
