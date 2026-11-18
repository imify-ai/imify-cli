//! `imify datasets upload`: validates a local dataset and uploads it into a recording.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use imify::ingest::{
    NewRecording, Recording, ResumePolicy, SkipReason, SkippedEntry, UploadEvent, UploadPlan,
    Uploader,
};
use imify::validate::Report;
use imify::{JournalError, UploadError};
use tokio::sync::mpsc::{self, UnboundedSender};

use super::{
    dataset_root_error, finding_json, invalid_report_error, unreadable_root, write_findings,
};
use crate::cli::UploadArgs;
use crate::commands::client;
use crate::dirs::journal_dir;
use crate::output::renderer::{RESULT_EVENT, Render, Renderer};
use crate::output::sanitize::printable;
use crate::output::{CliError, Environment, ErrorCode, Settings};
use crate::tui::live::{LiveView, StderrBackend};
use crate::tui::theme::Theme;
use crate::tui::upload::UploadView;
use crate::tui::{self, Outcome, Progress, format};

/// Width of the key column of the human result.
const KEY_WIDTH: usize = 12;

/// Most characters of the name of a recording: longer directory names are cut.
const MAX_NAME_CHARS: usize = 128;

/// `imify datasets upload <path> --task <text>`: validates the dataset, uploads it into a
/// recording and writes the recording.
///
/// Writes `validated` and `planned`, then the upload's progress, then the `result` event. With
/// `--dry-run`, stops after `planned` and needs no client. Otherwise the upload keeps a journal
/// in the [`journal_dir`] and continues the recording of an interrupted upload of `<path>`;
/// `--resume` requires such a recording, `--restart` discards it.
///
/// # Errors
///
/// Returns the error of building the client and of locating the journal directory before the
/// dataset is read. Returns `invalid_argument` when `<path>` is not a readable directory, a
/// recording option is outside the limits of the API or a file changes during the upload,
/// `dataset_invalid` when the report has an error or the API rejects the dataset, `not_found`
/// for `--resume` without an interrupted upload, `conflict` when the journal belongs to another
/// upload, is corrupt or outdated, or a file changed since the recording knows it,
/// `interrupted` on Ctrl-C, and the mapped API error otherwise.
pub async fn upload(
    env: &Environment,
    settings: &Settings,
    args: &UploadArgs,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    let target = if args.dry_run {
        None
    } else {
        let api = client(env).await?;
        Some((
            Target {
                client: api.client,
                journal_dir: journal_dir(env)?,
                policy: resume_policy(args.resume, args.restart),
            },
            (api.origin, api.lookup),
        ))
    };
    let name = recording_name(&args.path)?;
    let new_recording = new_recording(&name, args)?;
    let report = imify::validate::validate_dataset(&args.path).map_err(dataset_root_error)?;
    write_event(renderer, &Validated(&report))?;
    if let Some(error) = invalid_report_error(&report, &args.path) {
        return Err(error);
    }
    let plan = UploadPlan::from_dir(&args.path).map_err(dataset_root_error)?;
    // The live view shows the totals; its scrollback keeps the findings, the skipped symbolic
    // links and the summary only.
    let links_skipped = skipped_links(&plan).next().is_some();
    if planned_before_open(settings.live_view, links_skipped) {
        write_event(renderer, &Planned(&plan))?;
    }
    let mut result = UploadResult {
        settings: &new_recording,
        files: plan.files().len(),
        bytes: plan.total_bytes(),
        recording: None,
        resumed: false,
    };
    let Some((target, (origin, lookup))) = target else {
        return write_event(renderer, &result);
    };

    let live = if settings.live_view {
        let view = UploadView::new(
            name.as_str(),
            plan.files().len(),
            plan.total_bytes(),
            Theme::new(settings.color),
        );
        tui::live_or_warn(LiveView::stderr(view), renderer)
            .map_err(|error| progress_error(&error))?
    } else {
        None
    };
    if planned_after_open(settings.live_view, links_skipped, live.is_some()) {
        write_event(renderer, &Planned(&plan))?;
    }
    let sent = match send(target, live, &plan, &new_recording, renderer).await {
        Ok(sent) => sent,
        Err(error) => return Err(origin.adjust(error, lookup).await),
    };
    result.recording = Some(&sent.recording);
    result.resumed = sent.resumed;
    write_event(renderer, &result)
}

/// The settings of the new recording: named `name`, with the task and the recording options
/// of `args`.
///
/// # Errors
///
/// Returns `invalid_argument` when a value is outside the limits of the API.
fn new_recording(name: &str, args: &UploadArgs) -> Result<NewRecording, CliError> {
    let mut recording = NewRecording::new(name).task(args.task.as_str());
    recording.robot.clone_from(&args.robot);
    recording.operator.clone_from(&args.operator);
    recording.site.clone_from(&args.site);
    recording.start_time.clone_from(&args.start_time);
    recording.labels = args.labels.iter().cloned().collect();
    recording.check().map_err(|error| match error {
        imify::Error::Validation { message, .. } => CliError::new(
            ErrorCode::InvalidArgument,
            format!("Invalid recording option: {message}."),
        )
        .with_hint("Run `imify datasets upload --help`."),
        other => other.into(),
    })?;
    Ok(recording)
}

/// Where an upload goes: the API client, and the journal that lets it continue an interrupted
/// upload.
struct Target {
    client: imify::Client,
    journal_dir: PathBuf,
    policy: ResumePolicy,
}

/// The policy for the journal: `--resume` requires a journaled recording, `--restart` discards
/// it, and without either a journaled recording is continued when one exists.
fn resume_policy(resume: bool, restart: bool) -> ResumePolicy {
    match (resume, restart) {
        (true, _) => ResumePolicy::Required,
        (false, true) => ResumePolicy::Restart,
        (false, false) => ResumePolicy::IfJournaled,
    }
}

/// The completed recording of an upload and whether the upload continued a journaled
/// recording.
struct Sent {
    recording: Recording,
    resumed: bool,
}

/// The recording name: the final component of the canonicalized `path`, cut to its first 128
/// characters.
///
/// # Errors
///
/// Returns `invalid_argument` when `path` cannot be resolved or has no final component.
fn recording_name(path: &Path) -> Result<String, CliError> {
    let resolved = path
        .canonicalize()
        .map_err(|source| unreadable_root(path, &source))?;
    resolved
        .file_name()
        .map(|name| {
            name.to_string_lossy()
                .chars()
                .take(MAX_NAME_CHARS)
                .collect()
        })
        .ok_or_else(|| {
            CliError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "{} has no directory name to name the recording after.",
                    resolved.display()
                ),
            )
            .with_hint("Pass the dataset root: the directory that contains `meta/info.json`.")
        })
}

/// Uploads `plan` into a new or a journaled recording and reports the progress in `live`, or
/// as event lines without it.
async fn send(
    target: Target,
    live: Option<LiveView<StderrBackend, UploadView>>,
    plan: &UploadPlan,
    new_recording: &NewRecording,
    renderer: &mut dyn Renderer,
) -> Result<Sent, CliError> {
    let start = Instant::now();
    let (events, received) = mpsc::unbounded_channel();
    let live_in_use = live.is_some();
    let resumed = Arc::new(AtomicBool::new(false));
    let forward = forwarder(events.clone(), start, live_in_use);
    let resumed_flag = Arc::clone(&resumed);
    let uploader = Uploader::new(target.client)
        .journal(target.journal_dir, target.policy)
        .on_event(move |event| {
            if matches!(event, UploadEvent::RecordingResumed { .. }) {
                resumed_flag.store(true, Ordering::Relaxed);
            }
            forward(event);
        });
    let operation = async {
        let result = uploader.upload(plan, new_recording).await;
        if live_in_use {
            let step = if result.is_ok() {
                UploadStep::Finished
            } else {
                UploadStep::Failed
            };
            send_progress(&events, start, step);
        }
        result
    };

    match tui::run(
        Progress::new(live, renderer),
        received,
        operation,
        tui::ctrl_c(),
    )
    .await
    .map_err(|error| progress_error(&error))?
    {
        Outcome::Finished(result) => result.map_err(upload_error).map(|recording| Sent {
            recording,
            resumed: resumed.load(Ordering::Relaxed),
        }),
        Outcome::Interrupted => Err(CliError::interrupted()),
    }
}

/// The event handler of the uploader: stamps each event with the time since `start` and
/// sends it to `events`. Without the live view, [`UploadStep::FileProgress`] is dropped.
fn forwarder(
    events: UnboundedSender<UploadProgress>,
    start: Instant,
    live: bool,
) -> impl Fn(&UploadEvent) + Send + Sync + 'static {
    move |event| {
        if let Some(step) = forwarded_step(event, live) {
            send_progress(&events, start, step);
        }
    }
}

/// The step `event` is reported as, or `None` when it is not reported.
fn forwarded_step(event: &UploadEvent, live: bool) -> Option<UploadStep> {
    UploadStep::from_event(event)
        .filter(|step| live || !matches!(step, UploadStep::FileProgress { .. }))
}

fn send_progress(events: &UnboundedSender<UploadProgress>, start: Instant, step: UploadStep) {
    // Sending fails only after the progress loop ended; the event then has no reader.
    let _ = events.send(UploadProgress {
        at: start.elapsed(),
        step,
    });
}

/// The reason of a `409` for a file announced before with another size or SHA-256.
const FILE_CONFLICT: &str = "FILE_CONFLICT";

/// Maps an upload error: [`imify::Error::Validation`] becomes `dataset_invalid` with the
/// API's message, [`imify::Error::Io`] becomes `invalid_argument` naming the file, a file that
/// changed since the recording knows it becomes `conflict` with the hint to restart, and
/// [`imify::Error::Journal`] is mapped by [`journal_error`].
fn upload_error(error: imify::Error) -> CliError {
    match error {
        imify::Error::Journal { path, reason } => journal_error(&path, reason),
        imify::Error::Validation { message, .. } => {
            CliError::new(ErrorCode::DatasetInvalid, message)
        }
        imify::Error::Conflict {
            message,
            reason: Some(reason),
        } if reason == FILE_CONFLICT => changed_file_error(&message),
        imify::Error::Upload {
            path,
            reason: UploadError::ChecksumMismatch,
        } => changed_file_error(&format!("{path} changed since the upload started")),
        imify::Error::Upload { path, reason } => CliError::new(
            ErrorCode::Server,
            format!("Storage refused {}: {reason}.", printable(&path)),
        ),
        imify::Error::Io { path, source } => CliError::new(
            ErrorCode::InvalidArgument,
            format!(
                "Cannot upload {}: {source}. The file changed or became unreadable during the upload.",
                path.display()
            ),
        )
        .with_hint("Run the upload again when the dataset files no longer change."),
        other => other.into(),
    }
}

/// The hint for a journal or a recording that cannot be continued.
const RESTART_HINT: &str = "Run again with `--restart`.";

/// The error for a local file that changed after the recording learned its size and SHA-256:
/// the recording cannot take the new content.
fn changed_file_error(message: &str) -> CliError {
    CliError::new(
        ErrorCode::Conflict,
        format!(
            "A dataset file changed during the upload: {}. The recording keeps the content it was announced with.",
            printable(message)
        ),
    )
    .with_hint(RESTART_HINT)
}

/// Maps an unusable journal: missing for `--resume` becomes `not_found`, mismatched or corrupt
/// becomes `conflict` with the hint to restart, and an I/O failure becomes `internal`.
fn journal_error(path: &Path, reason: JournalError) -> CliError {
    match reason {
        JournalError::Missing => CliError::new(
            ErrorCode::NotFound,
            "No interrupted upload of this dataset directory to resume.",
        )
        .with_hint("Run without `--resume`."),
        JournalError::Mismatch(detail) => CliError::new(
            ErrorCode::Conflict,
            format!(
                "The interrupted upload of this dataset directory cannot be continued: {detail}."
            ),
        )
        .with_hint(RESTART_HINT),
        JournalError::Corrupt(detail) => CliError::new(
            ErrorCode::Conflict,
            format!(
                "The upload journal {} is corrupt: {detail}.",
                path.display()
            ),
        )
        .with_hint(RESTART_HINT),
        JournalError::Outdated(_) => CliError::new(
            ErrorCode::Conflict,
            "An older version of imify started the interrupted upload of this dataset directory. It cannot be continued.",
        )
        .with_hint(RESTART_HINT),
        other => CliError::new(
            ErrorCode::Internal,
            format!("Cannot use the upload journal {}: {other}.", path.display()),
        ),
    }
}

fn progress_error(error: &io::Error) -> CliError {
    CliError::new(
        ErrorCode::Internal,
        format!("Cannot write the upload progress: {error}"),
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

/// The `validated` event: the counts and findings of the validation report.
struct Validated<'a>(&'a Report);

impl Render for Validated<'_> {
    fn kind(&self) -> &'static str {
        "validated"
    }

    fn json(&self) -> serde_json::Value {
        let findings: Vec<serde_json::Value> = self.0.findings.iter().map(finding_json).collect();
        serde_json::json!({
            "errors": self.0.errors().count(),
            "warnings": self.0.warnings().count(),
            "findings": findings,
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        write_findings(w, &self.0.findings)
    }
}

/// The `planned` event: the files to upload and the entries left out.
struct Planned<'a>(&'a UploadPlan);

impl Render for Planned<'_> {
    fn kind(&self) -> &'static str {
        "planned"
    }

    fn json(&self) -> serde_json::Value {
        let skipped = self.0.skipped();
        let entries: Vec<serde_json::Value> = skipped
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "path": entry.path(),
                    "reason": entry.reason().as_str(),
                })
            })
            .collect();
        serde_json::json!({
            "files": self.0.files().len(),
            "bytes": self.0.total_bytes(),
            "skipped": skipped.iter().map(SkippedEntry::path).collect::<Vec<_>>(),
            "skipped_entries": entries,
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(
            w,
            "Planned {} files ({}).",
            self.0.files().len(),
            format::bytes(self.0.total_bytes())
        )?;
        for link in skipped_links(self.0) {
            writeln!(
                w,
                "Skipped {}: symbolic links are not uploaded.",
                printable(link.path())
            )?;
        }
        Ok(())
    }
}

/// Whether the `planned` event is written before the live view opens: without the live view,
/// and when links are skipped, so that the scrollback names them.
fn planned_before_open(live_view: bool, links_skipped: bool) -> bool {
    !live_view || links_skipped
}

/// Whether the `planned` event is written after the live view was opened or failed to open:
/// when the progress falls back to lines and the event was not written before.
fn planned_after_open(live_view: bool, links_skipped: bool, opened: bool) -> bool {
    !opened && !planned_before_open(live_view, links_skipped)
}

/// The symbolic links the plan leaves out.
fn skipped_links(plan: &UploadPlan) -> impl Iterator<Item = &SkippedEntry> {
    plan.skipped()
        .iter()
        .filter(|entry| entry.reason() == SkipReason::SymbolicLink)
}

/// The `result` event: the recording, or with `--dry-run` only what would be uploaded.
struct UploadResult<'a> {
    /// The settings sent to create the recording.
    settings: &'a NewRecording,
    files: usize,
    bytes: u64,
    /// `None` for a dry run.
    recording: Option<&'a Recording>,
    /// Whether the upload continued a journaled recording.
    resumed: bool,
}

impl UploadResult<'_> {
    /// The recording options: as sent with `--dry-run`, as the API reports them otherwise.
    /// An empty value is `None`.
    fn options(&self) -> RecordingOptions {
        let non_empty = |value: &str| (!value.is_empty()).then(|| value.to_owned());
        match self.recording {
            Some(recording) => RecordingOptions {
                robot: non_empty(&recording.robot),
                operator: non_empty(&recording.operator),
                site: non_empty(&recording.site),
                start_time: recording.start_time.clone(),
                labels: recording.labels.clone().into_iter().collect(),
            },
            None => RecordingOptions {
                robot: self.settings.robot.clone(),
                operator: self.settings.operator.clone(),
                site: self.settings.site.clone(),
                start_time: self.settings.start_time.clone(),
                labels: self.settings.labels.clone().into_iter().collect(),
            },
        }
    }
}

/// The robot, operator, site, start time and labels of a recording.
struct RecordingOptions {
    robot: Option<String>,
    operator: Option<String>,
    site: Option<String>,
    start_time: Option<String>,
    labels: Vec<(String, String)>,
}

impl Render for UploadResult<'_> {
    fn kind(&self) -> &'static str {
        RESULT_EVENT
    }

    fn json(&self) -> serde_json::Value {
        let options = self.options();
        let labels: serde_json::Map<String, serde_json::Value> = options
            .labels
            .into_iter()
            .map(|(key, value)| (key, value.into()))
            .collect();
        let mut result = serde_json::json!({
            "dry_run": self.recording.is_none(),
            "name": self.settings.display_name,
            "task": self.settings.task,
            "robot": options.robot,
            "operator": options.operator,
            "site": options.site,
            "start_time": options.start_time,
            "labels": labels,
            "files": self.files,
            "bytes": self.bytes,
        });
        if let (Some(recording), Some(fields)) = (self.recording, result.as_object_mut()) {
            fields.extend([
                ("session_id".to_owned(), recording.id.as_str().into()),
                ("dataset_id".to_owned(), recording.dataset_id.clone().into()),
                ("format".to_owned(), recording.format.clone().into()),
                ("robot_type".to_owned(), recording.robot_type.clone().into()),
                ("fps".to_owned(), recording.fps.into()),
                ("episodes".to_owned(), recording.episode_count.into()),
                ("status".to_owned(), recording.state.as_str().into()),
                ("resumed".to_owned(), self.resumed.into()),
            ]);
        }
        result
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        let mut rows: Vec<(&str, String)> = Vec::new();
        if let Some(recording) = self.recording {
            rows.push(("recording", recording.id.to_string()));
        }
        rows.push(("name", self.settings.display_name.clone()));
        rows.push(("task", self.settings.task.clone()));
        let options = self.options();
        rows.extend(
            [
                ("robot", options.robot),
                ("operator", options.operator),
                ("site", options.site),
                ("start time", options.start_time),
            ]
            .into_iter()
            .filter_map(|(key, value)| Some((key, value?))),
        );
        if !options.labels.is_empty() {
            let labels: Vec<String> = options
                .labels
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            rows.push(("labels", labels.join(", ")));
        }
        if let Some(recording) = self.recording {
            let or_dash = |value: Option<String>| value.unwrap_or_else(|| "-".to_owned());
            rows.extend([
                ("dataset", or_dash(recording.dataset_id.clone())),
                ("format", or_dash(recording.format.clone())),
                ("robot type", or_dash(recording.robot_type.clone())),
                ("fps", or_dash(recording.fps.map(|fps| fps.to_string()))),
                ("episodes", recording.episode_count.to_string()),
                ("status", recording.state.as_str().to_owned()),
            ]);
        }
        rows.push((
            "files",
            format!("{} ({})", self.files, format::bytes(self.bytes)),
        ));
        for (key, value) in rows {
            writeln!(w, "{key:KEY_WIDTH$}{}", printable(&value))?;
        }
        Ok(())
    }
}

/// One step of an upload, stamped with the time since the upload started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadProgress {
    /// Time since the upload started.
    pub at: Duration,
    pub step: UploadStep,
}

/// What happened during an upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadStep {
    RecordingCreated {
        recording: String,
    },
    /// The upload continues a journaled recording; `files_done` files of `bytes_done` bytes
    /// are in storage and not sent again.
    RecordingResumed {
        recording: String,
        files_done: usize,
        bytes_done: u64,
    },
    FileStarted {
        path: String,
        size: u64,
    },
    /// Bytes of the current request of a file handed to the network. Sent to the live view
    /// only.
    FileProgress {
        path: String,
        bytes_sent: u64,
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
    Completing,
    /// The recording is complete. Sent to the live view only.
    Finished,
    /// The upload failed. Sent to the live view only.
    Failed,
}

impl UploadStep {
    /// The step for an SDK event, or `None` for an event this CLI version does not know.
    pub fn from_event(event: &UploadEvent) -> Option<Self> {
        let step = match event {
            UploadEvent::RecordingCreated { recording } => Self::RecordingCreated {
                recording: recording.to_string(),
            },
            UploadEvent::RecordingResumed {
                recording,
                files_done,
                bytes_done,
            } => Self::RecordingResumed {
                recording: recording.to_string(),
                files_done: *files_done,
                bytes_done: *bytes_done,
            },
            UploadEvent::FileStarted { path, size } => Self::FileStarted {
                path: path.clone(),
                size: *size,
            },
            UploadEvent::FileProgress { path, bytes_sent } => Self::FileProgress {
                path: path.clone(),
                bytes_sent: *bytes_sent,
            },
            UploadEvent::FileRetrying {
                path,
                attempt,
                reason,
            } => Self::FileRetrying {
                path: path.clone(),
                attempt: *attempt,
                reason: reason.clone(),
            },
            UploadEvent::FileFinished { path, size } => Self::FileFinished {
                path: path.clone(),
                size: *size,
            },
            UploadEvent::RateLimited { pause } => Self::RateLimited { pause: *pause },
            UploadEvent::Completing => Self::Completing,
            _ => return None,
        };
        Some(step)
    }
}

impl Render for UploadProgress {
    fn kind(&self) -> &'static str {
        match self.step {
            UploadStep::RecordingCreated { .. } => "session_created",
            UploadStep::RecordingResumed { .. } => "session_resumed",
            UploadStep::FileStarted { .. } => "file_started",
            UploadStep::FileProgress { .. } => "file_progress",
            UploadStep::FileRetrying { .. } => "file_retrying",
            UploadStep::FileFinished { .. } => "file_finished",
            UploadStep::RateLimited { .. } => "rate_limited",
            UploadStep::Completing => "completing",
            UploadStep::Finished => "finished",
            UploadStep::Failed => "failed",
        }
    }

    fn json(&self) -> serde_json::Value {
        match &self.step {
            UploadStep::RecordingCreated { recording } => {
                serde_json::json!({ "session_id": recording })
            }
            UploadStep::RecordingResumed {
                recording,
                files_done,
                bytes_done,
            } => serde_json::json!({
                "session_id": recording,
                "files_done": files_done,
                "bytes_done": bytes_done,
            }),
            UploadStep::FileStarted { path, size } | UploadStep::FileFinished { path, size } => {
                serde_json::json!({ "path": path, "size": size })
            }
            UploadStep::FileProgress { path, bytes_sent } => {
                serde_json::json!({ "path": path, "bytes_sent": bytes_sent })
            }
            UploadStep::FileRetrying {
                path,
                attempt,
                reason,
            } => serde_json::json!({ "path": path, "attempt": attempt, "reason": reason }),
            UploadStep::RateLimited { pause } => {
                serde_json::json!({ "pause_ms": u64::try_from(pause.as_millis()).unwrap_or(u64::MAX) })
            }
            UploadStep::Completing | UploadStep::Finished | UploadStep::Failed => {
                serde_json::json!({})
            }
        }
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        match &self.step {
            UploadStep::RecordingCreated { recording } => {
                writeln!(w, "Created recording {}.", printable(recording))
            }
            UploadStep::RecordingResumed {
                recording,
                files_done,
                bytes_done,
            } => writeln!(
                w,
                "Resumed recording {}: {files_done} files ({}) already uploaded.",
                printable(recording),
                format::bytes(*bytes_done)
            ),
            UploadStep::FileStarted { path, size } => {
                writeln!(
                    w,
                    "Uploading {} ({}).",
                    printable(path),
                    format::bytes(*size)
                )
            }
            UploadStep::FileProgress { path, bytes_sent } => {
                writeln!(
                    w,
                    "Sent {} of {}.",
                    format::bytes(*bytes_sent),
                    printable(path)
                )
            }
            UploadStep::FileRetrying {
                path,
                attempt,
                reason,
            } => writeln!(
                w,
                "Retrying {}, attempt {attempt}: {}.",
                printable(path),
                printable(reason)
            ),
            UploadStep::FileFinished { path, .. } => writeln!(w, "Uploaded {}.", printable(path)),
            UploadStep::RateLimited { pause } => writeln!(
                w,
                "Rate limited: no request starts for {}.",
                format::duration(*pause)
            ),
            UploadStep::Completing => writeln!(w, "Completing the recording."),
            UploadStep::Finished | UploadStep::Failed => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    fn dataset_dir() -> (tempfile::TempDir, PathBuf) {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("pick-cube");
        fs::create_dir_all(root.join("meta")).unwrap();
        (parent, root)
    }

    #[test]
    fn recording_name_is_the_final_component_of_the_canonicalized_path() {
        let (_parent, root) = dataset_dir();

        let names = [
            recording_name(&root).unwrap(),
            recording_name(&root.join("")).unwrap(),
            recording_name(&root.join(".")).unwrap(),
            recording_name(&root.join("meta/..")).unwrap(),
        ];

        assert_eq!(names, ["pick-cube"; 4]);
    }

    #[test]
    fn recording_name_of_a_path_without_a_final_component_is_an_invalid_argument() {
        let error = recording_name(Path::new("/")).unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn recording_name_of_a_missing_directory_is_an_invalid_argument() {
        let (_parent, root) = dataset_dir();

        let error = recording_name(&root.join("missing")).unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn validation_error_maps_to_dataset_invalid_with_the_api_message() {
        let error = upload_error(imify::Error::Validation {
            message: "Unsupported codebase_version v1.6".into(),
            fields: Vec::new(),
            reason: Some("INVALID_DATASET".into()),
        });

        assert_eq!(
            error,
            CliError::new(
                ErrorCode::DatasetInvalid,
                "Unsupported codebase_version v1.6"
            )
        );
    }

    #[test]
    fn io_error_during_the_upload_maps_to_invalid_argument_naming_the_file() {
        let error = upload_error(imify::Error::Io {
            path: PathBuf::from("pick-cube/data/chunk-000/file-000.parquet"),
            source: io::Error::other("file size changed since planning"),
        });

        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(
            error
                .message
                .contains("pick-cube/data/chunk-000/file-000.parquet"),
            "{}",
            error.message
        );
        assert!(error.hint.is_some());
    }

    #[test]
    fn other_upload_errors_keep_their_mapping() {
        let error = upload_error(imify::Error::UnexpectedResponse {
            status: 418,
            message: "I'm a teapot".into(),
        });

        assert_eq!(error.code, ErrorCode::Internal);
    }

    fn journal_error(reason: imify::JournalError) -> CliError {
        upload_error(imify::Error::Journal {
            path: PathBuf::from("uploads/3f2c.jsonl"),
            reason,
        })
    }

    #[test]
    fn missing_journal_maps_to_not_found_with_the_hint_to_run_without_resume() {
        let error = journal_error(imify::JournalError::Missing);

        assert_eq!(
            (error.code, error.hint.as_deref()),
            (ErrorCode::NotFound, Some("Run without `--resume`."))
        );
    }

    #[test]
    fn mismatched_or_corrupt_journal_maps_to_conflict_with_the_hint_to_restart() {
        let errors = [
            journal_error(imify::JournalError::Mismatch(
                "the task description differs".into(),
            )),
            journal_error(imify::JournalError::Corrupt(
                "line 2 is not valid JSON".into(),
            )),
        ];

        for error in errors {
            assert_eq!(
                (error.code, error.hint.as_deref()),
                (ErrorCode::Conflict, Some("Run again with `--restart`.")),
                "{}",
                error.message
            );
        }
    }

    #[test]
    fn mismatched_journal_message_names_the_reason() {
        let error = journal_error(imify::JournalError::Mismatch(
            "the task description differs".into(),
        ));

        assert!(
            error.message.contains("the task description differs"),
            "{}",
            error.message
        );
    }

    #[test]
    fn unusable_journal_file_maps_to_internal() {
        let error = journal_error(imify::JournalError::Io(io::Error::other(
            "permission denied",
        )));

        assert_eq!(error.code, ErrorCode::Internal);
    }

    #[test]
    fn resume_policy_follows_the_resume_and_restart_flags() {
        let policies = [(false, false), (true, false), (false, true)]
            .map(|(resume, restart)| resume_policy(resume, restart));

        assert_eq!(
            policies,
            [
                ResumePolicy::IfJournaled,
                ResumePolicy::Required,
                ResumePolicy::Restart
            ]
        );
    }

    fn session_resumed() -> UploadProgress {
        UploadProgress {
            at: Duration::ZERO,
            step: UploadStep::from_event(&UploadEvent::RecordingResumed {
                recording: imify::ingest::RecordingId::new("3f2c9a"),
                files_done: 3,
                bytes_done: 2048,
            })
            .unwrap(),
        }
    }

    #[test]
    fn session_resumed_json_carries_the_session_and_what_is_already_done() {
        let event = session_resumed();

        assert_eq!(
            (event.kind(), event.json()),
            (
                "session_resumed",
                serde_json::json!({ "session_id": "3f2c9a", "files_done": 3, "bytes_done": 2048 })
            )
        );
    }

    #[test]
    fn session_resumed_line_names_the_recording_and_what_is_already_done() {
        let mut line = Vec::new();

        session_resumed().human(&mut line).unwrap();

        assert_eq!(
            String::from_utf8(line).unwrap(),
            "Resumed recording 3f2c9a: 3 files (2.0 KiB) already uploaded.\n"
        );
    }

    fn file_progress() -> UploadEvent {
        UploadEvent::FileProgress {
            path: "meta/info.json".into(),
            bytes_sent: 512,
        }
    }

    #[test]
    fn file_progress_is_not_forwarded_without_the_live_view() {
        assert_eq!(forwarded_step(&file_progress(), false), None);
    }

    #[test]
    fn file_progress_is_forwarded_to_the_live_view() {
        assert_eq!(
            forwarded_step(&file_progress(), true),
            Some(UploadStep::FileProgress {
                path: "meta/info.json".into(),
                bytes_sent: 512
            })
        );
    }

    #[test]
    fn other_events_are_forwarded_without_the_live_view() {
        assert_eq!(
            forwarded_step(&UploadEvent::Completing, false),
            Some(UploadStep::Completing)
        );
    }

    /// How often an upload writes the `planned` event.
    fn planned_writes(live_view: bool, links_skipped: bool, opened: bool) -> usize {
        usize::from(planned_before_open(live_view, links_skipped))
            + usize::from(planned_after_open(live_view, links_skipped, opened))
    }

    #[test]
    fn planned_is_written_when_the_live_view_falls_back_to_lines() {
        assert_eq!(planned_writes(true, false, false), 1);
    }

    #[test]
    fn planned_is_not_written_when_the_live_view_opens_and_no_link_is_skipped() {
        assert_eq!(planned_writes(true, false, true), 0);
    }

    #[test]
    fn planned_is_written_once_when_links_are_skipped_and_the_live_view_falls_back() {
        assert_eq!(planned_writes(true, true, false), 1);
    }

    #[test]
    fn rate_limited_json_carries_the_pause_in_milliseconds() {
        let event = UploadProgress {
            at: Duration::ZERO,
            step: UploadStep::RateLimited {
                pause: Duration::from_millis(3000),
            },
        };

        assert_eq!(event.json(), serde_json::json!({ "pause_ms": 3000 }));
    }
}
