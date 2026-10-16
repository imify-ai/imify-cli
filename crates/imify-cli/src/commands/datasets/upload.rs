//! `imify datasets upload`: validates a local dataset and uploads it into an ingest session.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use imify::JournalError;
use imify::ingest::{
    NewSession, ResumePolicy, Session, SessionStatus, UploadEvent, UploadPlan, Uploader,
};
use imify::validate::Report;
use tokio::sync::mpsc::{self, UnboundedSender};

use super::journal::journal_dir;
use super::{
    dataset_root_error, finding_json, invalid_report_error, unreadable_root, write_findings,
};
use crate::cli::UploadArgs;
use crate::commands::client;
use crate::output::renderer::{RESULT_EVENT, Render, Renderer};
use crate::output::{CliError, Environment, ErrorCode, Settings};
use crate::tui::live::LiveView;
use crate::tui::theme::Theme;
use crate::tui::upload::UploadView;
use crate::tui::{self, Outcome, Progress, format};

/// Width of the key column of the human result.
const KEY_WIDTH: usize = 10;

/// `imify datasets upload <path> --task <text>`: validates the dataset, uploads it into an
/// ingest session and writes the session.
///
/// Writes `validated` and `planned`, then the upload's progress, then the `result` event. With
/// `--dry-run`, stops after `planned` and needs no client. Otherwise the upload keeps a journal
/// in the [`journal_dir`] and continues the session of an interrupted upload of `<path>`;
/// `--resume` requires such a session, `--restart` discards it.
///
/// # Errors
///
/// Returns the error of building the client and of locating the journal directory before the
/// dataset is read. Returns `invalid_argument` when `<path>` is not a readable directory or a
/// file changes during the upload, `dataset_invalid` when the report has an error or the API
/// rejects the dataset, `not_found` for `--resume` without an interrupted upload, `conflict`
/// when the journal belongs to another upload or is corrupt, `interrupted` on Ctrl-C, and the
/// mapped API error otherwise.
pub async fn upload(
    env: &Environment,
    settings: &Settings,
    args: &UploadArgs,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    let target = if args.dry_run {
        None
    } else {
        Some(Target {
            client: client(env)?,
            journal_dir: journal_dir(env)?,
            policy: resume_policy(args.resume, args.restart),
        })
    };
    let name = session_name(&args.path)?;
    let report = imify::validate::validate_dataset(&args.path).map_err(dataset_root_error)?;
    write_event(renderer, &Validated(&report))?;
    if let Some(error) = invalid_report_error(&report, &args.path) {
        return Err(error);
    }
    let plan = UploadPlan::from_dir(&args.path).map_err(dataset_root_error)?;
    // The live view shows the totals; its scrollback keeps the findings and the summary only.
    if !settings.live_view {
        write_event(renderer, &Planned(&plan))?;
    }
    let mut result = UploadResult {
        name: &name,
        task: &args.task,
        files: plan.files().len(),
        bytes: plan.total_bytes(),
        session: None,
        resumed: false,
    };
    let Some(target) = target else {
        return write_event(renderer, &result);
    };

    let new_session = NewSession::new(name.as_str()).task_description(args.task.as_str());
    let sent = send(target, settings, &plan, &new_session, renderer).await?;
    result.session = Some(&sent.session);
    result.resumed = sent.resumed;
    write_event(renderer, &result)
}

/// Where an upload goes: the API client, and the journal that lets it continue an interrupted
/// upload.
struct Target {
    client: imify::Client,
    journal_dir: PathBuf,
    policy: ResumePolicy,
}

/// The policy for the journal: `--resume` requires a journaled session, `--restart` discards
/// it, and without either a journaled session is continued when one exists.
fn resume_policy(resume: bool, restart: bool) -> ResumePolicy {
    match (resume, restart) {
        (true, _) => ResumePolicy::Required,
        (false, true) => ResumePolicy::Restart,
        (false, false) => ResumePolicy::IfJournaled,
    }
}

/// The completed session of an upload and whether the upload continued a journaled session.
struct Sent {
    session: Session,
    resumed: bool,
}

/// The session name: the final component of the canonicalized `path`.
///
/// # Errors
///
/// Returns `invalid_argument` when `path` cannot be resolved or has no final component.
fn session_name(path: &Path) -> Result<String, CliError> {
    let resolved = path
        .canonicalize()
        .map_err(|source| unreadable_root(path, &source))?;
    resolved
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| {
            CliError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "{} has no directory name to name the ingest session after.",
                    resolved.display()
                ),
            )
            .with_hint("Pass the dataset root: the directory that contains `meta/info.json`.")
        })
}

/// Uploads `plan` into a new or a journaled session and reports the progress in a live view or
/// as event lines.
async fn send(
    target: Target,
    settings: &Settings,
    plan: &UploadPlan,
    new_session: &NewSession,
    renderer: &mut dyn Renderer,
) -> Result<Sent, CliError> {
    let start = Instant::now();
    let (events, received) = mpsc::unbounded_channel();
    let live = settings.live_view;
    let resumed = Arc::new(AtomicBool::new(false));
    let forward = forwarder(events.clone(), start, live);
    let resumed_flag = Arc::clone(&resumed);
    let uploader = Uploader::new(target.client)
        .journal(target.journal_dir, target.policy)
        .on_event(move |event| {
            if matches!(event, UploadEvent::SessionResumed { .. }) {
                resumed_flag.store(true, Ordering::Relaxed);
            }
            forward(event);
        });
    let operation = async {
        let result = uploader.upload(plan, new_session).await;
        if live {
            let step = if result.is_ok() {
                UploadStep::Finished
            } else {
                UploadStep::Failed
            };
            send_progress(&events, start, step);
        }
        result
    };

    let progress = if live {
        let view = UploadView::new(
            new_session.name.as_str(),
            plan.files().len(),
            plan.total_bytes(),
            Theme::new(settings.color),
        );
        Progress::Live(LiveView::stderr(view).map_err(|error| progress_error(&error))?)
    } else {
        Progress::Lines(renderer)
    };
    match tui::run(progress, received, operation, tui::ctrl_c())
        .await
        .map_err(|error| progress_error(&error))?
    {
        Outcome::Finished(result) => result.map_err(upload_error).map(|session| Sent {
            session,
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

/// Maps an upload error: [`imify::Error::Validation`] becomes `dataset_invalid` with the
/// API's message, [`imify::Error::Io`] becomes `invalid_argument` naming the file, and
/// [`imify::Error::Journal`] is mapped by [`journal_error`].
fn upload_error(error: imify::Error) -> CliError {
    match error {
        imify::Error::Journal { path, reason } => journal_error(&path, reason),
        imify::Error::Validation { message, .. } => {
            CliError::new(ErrorCode::DatasetInvalid, message)
        }
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

/// The hint for a journal that cannot be continued.
const RESTART_HINT: &str = "Run again with `--restart`.";

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
        serde_json::json!({
            "files": self.0.files().len(),
            "bytes": self.0.total_bytes(),
            "skipped": self.0.skipped(),
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(
            w,
            "Planned {} files ({}).",
            self.0.files().len(),
            format::bytes(self.0.total_bytes())
        )
    }
}

/// The `result` event: the ingest session, or with `--dry-run` only what would be uploaded.
struct UploadResult<'a> {
    name: &'a str,
    task: &'a str,
    files: usize,
    bytes: u64,
    /// `None` for a dry run.
    session: Option<&'a Session>,
    /// Whether the upload continued a journaled session.
    resumed: bool,
}

fn status_text(status: &SessionStatus) -> &str {
    match status {
        SessionStatus::Uploading => "uploading",
        SessionStatus::Ready => "ready",
        SessionStatus::Other(value) => value,
        _ => "unknown",
    }
}

impl Render for UploadResult<'_> {
    fn kind(&self) -> &'static str {
        RESULT_EVENT
    }

    fn json(&self) -> serde_json::Value {
        let mut result = serde_json::json!({
            "dry_run": self.session.is_none(),
            "name": self.name,
            "task": self.task,
            "files": self.files,
            "bytes": self.bytes,
        });
        if let (Some(session), Some(fields)) = (self.session, result.as_object_mut()) {
            fields.extend([
                ("session_id".to_owned(), session.id.as_str().into()),
                ("format".to_owned(), session.dataset_format.clone().into()),
                ("robot_type".to_owned(), session.robot_type.clone().into()),
                ("fps".to_owned(), session.fps.into()),
                ("episodes".to_owned(), session.total_episodes.into()),
                ("status".to_owned(), status_text(&session.status).into()),
                ("resumed".to_owned(), self.resumed.into()),
            ]);
        }
        result
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        let mut rows: Vec<(&str, String)> = Vec::new();
        if let Some(session) = self.session {
            rows.push(("session", session.id.to_string()));
        }
        rows.push(("name", self.name.to_owned()));
        rows.push(("task", self.task.to_owned()));
        if let Some(session) = self.session {
            let or_dash = |value: Option<String>| value.unwrap_or_else(|| "-".to_owned());
            rows.extend([
                ("format", or_dash(session.dataset_format.clone())),
                ("robot", or_dash(session.robot_type.clone())),
                ("fps", or_dash(session.fps.map(|fps| fps.to_string()))),
                (
                    "episodes",
                    or_dash(session.total_episodes.map(|n| n.to_string())),
                ),
                ("status", status_text(&session.status).to_owned()),
            ]);
        }
        rows.push((
            "files",
            format!("{} ({})", self.files, format::bytes(self.bytes)),
        ));
        for (key, value) in rows {
            writeln!(w, "{key:KEY_WIDTH$}{value}")?;
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
    SessionCreated {
        session: String,
    },
    /// The upload continues a journaled session; `files_done` files of `bytes_done` bytes are
    /// not sent again.
    SessionResumed {
        session: String,
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
    /// The session is complete. Sent to the live view only.
    Finished,
    /// The upload failed. Sent to the live view only.
    Failed,
}

impl UploadStep {
    /// The step for an SDK event, or `None` for an event this CLI version does not know.
    pub fn from_event(event: &UploadEvent) -> Option<Self> {
        let step = match event {
            UploadEvent::SessionCreated { session } => Self::SessionCreated {
                session: session.to_string(),
            },
            UploadEvent::SessionResumed {
                session,
                files_done,
                bytes_done,
            } => Self::SessionResumed {
                session: session.to_string(),
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
            UploadStep::SessionCreated { .. } => "session_created",
            UploadStep::SessionResumed { .. } => "session_resumed",
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
            UploadStep::SessionCreated { session } => serde_json::json!({ "session_id": session }),
            UploadStep::SessionResumed {
                session,
                files_done,
                bytes_done,
            } => serde_json::json!({
                "session_id": session,
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
            UploadStep::SessionCreated { session } => {
                writeln!(w, "Created ingest session {session}.")
            }
            UploadStep::SessionResumed {
                session,
                files_done,
                bytes_done,
            } => writeln!(
                w,
                "Resumed ingest session {session}: {files_done} files ({}) already uploaded.",
                format::bytes(*bytes_done)
            ),
            UploadStep::FileStarted { path, size } => {
                writeln!(w, "Uploading {path} ({}).", format::bytes(*size))
            }
            UploadStep::FileProgress { path, bytes_sent } => {
                writeln!(w, "Sent {} of {path}.", format::bytes(*bytes_sent))
            }
            UploadStep::FileRetrying {
                path,
                attempt,
                reason,
            } => writeln!(w, "Retrying {path}, attempt {attempt}: {reason}."),
            UploadStep::FileFinished { path, .. } => writeln!(w, "Uploaded {path}."),
            UploadStep::RateLimited { pause } => writeln!(
                w,
                "Rate limited: no request starts for {}.",
                format::duration(*pause)
            ),
            UploadStep::Completing => writeln!(w, "Completing the ingest session."),
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
    fn session_name_is_the_final_component_of_the_canonicalized_path() {
        let (_parent, root) = dataset_dir();

        let names = [
            session_name(&root).unwrap(),
            session_name(&root.join("")).unwrap(),
            session_name(&root.join(".")).unwrap(),
            session_name(&root.join("meta/..")).unwrap(),
        ];

        assert_eq!(names, ["pick-cube"; 4]);
    }

    #[test]
    fn session_name_of_a_path_without_a_final_component_is_an_invalid_argument() {
        let error = session_name(Path::new("/")).unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn session_name_of_a_missing_directory_is_an_invalid_argument() {
        let (_parent, root) = dataset_dir();

        let error = session_name(&root.join("missing")).unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn validation_error_maps_to_dataset_invalid_with_the_api_message() {
        let error = upload_error(imify::Error::Validation {
            message: "Unsupported codebase_version v1.6".into(),
            fields: Vec::new(),
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
            step: UploadStep::from_event(&UploadEvent::SessionResumed {
                session: imify::ingest::SessionId::new("3f2c9a"),
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
    fn session_resumed_line_names_the_session_and_what_is_already_done() {
        let mut line = Vec::new();

        session_resumed().human(&mut line).unwrap();

        assert_eq!(
            String::from_utf8(line).unwrap(),
            "Resumed ingest session 3f2c9a: 3 files (2.0 KiB) already uploaded.\n"
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
