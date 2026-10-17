//! The update hint: after a successful command, one line on stderr when a newer release
//! exists.
//!
//! The check runs on its own thread while the command runs, at most once per 24 hours, and
//! has a deadline of 1 s from its start. Its failures are logged at debug level and never
//! change the command's output or exit code.

use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use imify::update::{ReleaseSource, Version};
use serde::{Deserialize, Serialize};

use super::install::InstallMethod;
use super::{installation, release_client, running_version};
use crate::cli::{Command, SelfCommand};
use crate::dirs::update_check_file;
use crate::output::renderer::Renderer;
use crate::output::{CliError, Environment, ErrorCode, OutputMode, Settings};
use crate::private_file::{self, Permissions, PrivateFileError};

/// How long after its start the CLI waits for the check, and the time limit of its request.
const DEADLINE: Duration = Duration::from_secs(1);

/// The shortest time between two checks.
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Whether the update check runs for `command`: human output without `--quiet`, stderr on a
/// terminal, no opt-out variable set, and a command other than `imify self update` and
/// `imify doctor`. Bare `imify` does not check.
fn applies(command: Option<&Command>, settings: &Settings, env: &Environment) -> bool {
    let checked_command = match command {
        None
        | Some(
            Command::Doctor
            | Command::Itself {
                command: SelfCommand::Update,
            },
        ) => false,
        Some(_) => true,
    };
    checked_command
        && settings.mode == OutputMode::Human
        && !settings.quiet
        && env.stderr_is_tty
        && !env.update_check_opted_out()
}

/// An update check running on its own thread.
#[derive(Debug)]
pub struct PendingCheck {
    hint: mpsc::Receiver<Option<String>>,
    deadline: Instant,
}

impl PendingCheck {
    /// Starts the update check when it [`applies`] to the invocation; `None` otherwise.
    pub fn start(
        command: Option<&Command>,
        settings: &Settings,
        env: &Environment,
    ) -> Option<Self> {
        if !applies(command, settings, env) {
            return None;
        }
        let env = env.clone();
        Self::spawn(move || check(&env), DEADLINE)
    }

    /// Runs `job` on a new thread, with a deadline of `deadline` from now. `None`, logged at
    /// debug level, when the thread cannot be started.
    fn spawn(
        job: impl FnOnce() -> Option<String> + Send + 'static,
        deadline: Duration,
    ) -> Option<Self> {
        let started = Instant::now();
        let (sender, hint) = mpsc::sync_channel(1);
        let spawned = std::thread::Builder::new()
            .name("update-check".to_owned())
            .spawn(move || {
                // The receiver is gone when the command failed or the deadline passed.
                let _ = sender.send(job());
            });
        match spawned {
            Ok(_) => Some(Self {
                hint,
                deadline: started + deadline,
            }),
            Err(error) => {
                tracing::debug!("cannot start the update check: {error}");
                None
            }
        }
    }

    /// Waits for the check until its deadline and writes its hint, if any, to stderr. A
    /// check that misses the deadline is dropped.
    pub fn finish(self, renderer: &mut dyn Renderer) {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        match self.hint.recv_timeout(remaining) {
            Ok(Some(hint)) => {
                if let Err(error) = renderer.notice(&hint) {
                    tracing::debug!("cannot write the update hint: {error}");
                }
            }
            Ok(None) | Err(RecvTimeoutError::Disconnected) => {}
            Err(RecvTimeoutError::Timeout) => {
                tracing::debug!("the update check did not finish within {DEADLINE:?}");
            }
        }
    }
}

/// The update hint for the running binary; `None`, logged at debug level, on any failure.
fn check(env: &Environment) -> Option<String> {
    let result = std::env::current_exe()
        .map_err(|error| {
            CliError::new(
                ErrorCode::Internal,
                format!("Cannot locate the running executable: {error}"),
            )
        })
        .and_then(|executable| {
            let installed = running_version()?;
            crate::runtime()?.block_on(hint(env, &executable, &installed, SystemTime::now()))
        });
    result.unwrap_or_else(|error| {
        tracing::debug!("update check failed: {}", error.message);
        None
    })
}

/// Looks up the latest release when the last check is 24 hours or more before `now`, and
/// returns the hint when it is newer than `installed`. Records `now` before the request.
///
/// # Errors
///
/// Returns `internal` when the time cannot be recorded, in which case no request is sent,
/// and the errors of the release lookup.
async fn hint(
    env: &Environment,
    executable: &Path,
    installed: &Version,
    now: SystemTime,
) -> Result<Option<String>, CliError> {
    let file = update_check_file(env).ok_or_else(|| {
        CliError::new(
            ErrorCode::Internal,
            "Cannot locate the update check file: HOME is not set to an absolute path.",
        )
    })?;
    if !claim(&file, now)? {
        return Ok(None);
    }
    let latest = release_client(env, ReleaseSource::imify_cli(), Some(DEADLINE))?
        .latest()
        .await?
        .version;
    if latest <= *installed {
        return Ok(None);
    }
    Ok(Some(format!(
        "imify {latest} is available (installed: {installed}). {}",
        update_command(env, executable)
    )))
}

/// What to run to update `executable`: `imify self update` for an install with a receipt,
/// otherwise the update command of its install method.
fn update_command(env: &Environment, executable: &Path) -> String {
    match installation(env, executable, &ReleaseSource::imify_cli()) {
        Ok(Some(_)) => return "Run `imify self update`.".to_owned(),
        Ok(None) => {}
        Err(error) => tracing::debug!("cannot read the install receipt: {}", error.message),
    }
    InstallMethod::detect(executable, env).hint()
}

/// The content of the update check file.
#[derive(Debug, Serialize, Deserialize)]
struct State {
    /// The time of the last check, in seconds since the Unix epoch.
    checked_at: u64,
}

/// Whether a check is due at `now`: the file at `path` records no check, an unreadable one,
/// or one 24 hours or more before `now` or after it. A due check records `now` in the file,
/// with mode `0600`.
///
/// # Errors
///
/// Returns an error when the directory cannot be created or the file cannot be written.
fn claim(path: &Path, now: SystemTime) -> Result<bool, PrivateFileError> {
    if let Some(last) = last_check(path) {
        let due = now
            .duration_since(last)
            .map_or(true, |elapsed| elapsed >= INTERVAL);
        if !due {
            return Ok(false);
        }
    }
    let checked_at = now
        .duration_since(UNIX_EPOCH)
        .map_err(|error| PrivateFileError::new(path, error.to_string()))?
        .as_secs();
    let content = serde_json::to_vec(&State { checked_at })
        .map_err(|error| PrivateFileError::new(path, error.to_string()))?;
    if let Some(dir) = path.parent() {
        private_file::create_dir(dir)?;
    }
    private_file::write(path, &content)?;
    Ok(true)
}

/// The time of the last check from the file at `path`; `None` when there is none or the file
/// cannot be read.
fn last_check(path: &Path) -> Option<SystemTime> {
    let content = match private_file::read(path, Permissions::Any) {
        Ok(content) => content?,
        Err(error) => {
            tracing::debug!("cannot read the update check file: {error}");
            return None;
        }
    };
    match serde_json::from_slice::<State>(&content) {
        Ok(state) => Some(UNIX_EPOCH + Duration::from_secs(state.checked_at)),
        Err(error) => {
            tracing::debug!("the update check file is invalid: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests;
