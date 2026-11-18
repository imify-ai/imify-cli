//! Ctrl-C during `imify datasets upload`, sent as SIGINT to the binary, and the upload run
//! again afterwards.
#![cfg(unix)]

use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use imify_test_support::{DatasetBuilder, Format};
use serde_json::json;
use wiremock::MockServer;

mod recordings;

use recordings::{RECORDINGS, Stored, completed};

/// Time a data file `PUT` takes to answer: longer than the test waits for the exit.
const SLOW_ANSWER: Duration = Duration::from_secs(30);
/// Time allowed from the start of the binary to the first data file transfer.
const START_LIMIT: Duration = Duration::from_secs(30);
/// Time allowed from SIGINT to the exit of the binary.
const EXIT_LIMIT: Duration = Duration::from_secs(10);

/// Storage stores `meta/info.json` at once and answers every other file after
/// [`SLOW_ANSWER`].
fn slow_data_files(path: &str) -> Duration {
    if path == "meta/info.json" {
        Duration::ZERO
    } else {
        SLOW_ANSWER
    }
}

/// A recordings API whose storage is slow for data files, and the files it holds.
async fn slow_ingest_api() -> (MockServer, Stored) {
    let server = MockServer::start().await;
    let stored = Stored::default();
    recordings::mount_slow(&server, &stored, completed(), slow_data_files).await;
    (server, stored)
}

/// Replaces the mocks of `server` with a recordings API that stores every file at once and
/// completes the recording, and clears the recorded requests. Storage keeps `stored`.
async fn make_fast(server: &MockServer, stored: &Stored) {
    server.reset().await;
    recordings::mount(server, stored, completed()).await;
}

/// Kills the child process when dropped, so that a failed assertion leaves no process behind.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Waits up to `limit` for the child to exit. `None` when it is still running.
fn wait_for_exit(child: &mut Child, limit: Duration) -> std::io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(None)
}

/// Whether `line` is the `file_started` event of a file other than `meta/info.json`.
fn is_data_transfer(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .is_ok_and(|event| event["type"] == "file_started" && event["path"] != "meta/info.json")
}

/// Whether `line` is the `file_finished` event of `meta/info.json`.
fn is_info_finished(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .is_ok_and(|event| event["type"] == "file_finished" && event["path"] == "meta/info.json")
}

/// `imify datasets upload . --task "Pick up the red cube" --json` in `root` against `server`,
/// with `HOME` set to `home`.
fn upload_command(server: &MockServer, root: &Path, home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_imify"));
    command
        .env_remove("IMIFY_OUTPUT")
        .env_remove("NO_COLOR")
        .env_remove("XDG_STATE_HOME")
        .env("HOME", home)
        .env("IMIFY_TOKEN", "tok_interrupt")
        .env("IMIFY_API_URL", server.uri())
        .env("IMIFY_CONFIG_DIR", home.join("config"))
        .current_dir(root)
        .args(["datasets", "upload", ".", "--task", "Pick up the red cube"])
        .arg("--json")
        .stdin(Stdio::null());
    command
}

/// What an interrupted upload wrote and how it exited.
struct Interrupted {
    status: ExitStatus,
    stdout: Vec<String>,
    stderr: String,
}

/// Starts the upload, sends SIGINT once `meta/info.json` is uploaded and the transfer of a data
/// file started, and waits for the exit.
///
/// # Errors
///
/// Returns an error when the process cannot be started or signalled, no data file transfer
/// starts within [`START_LIMIT`], or the process does not exit within [`EXIT_LIMIT`] of SIGINT.
fn interrupt_upload(mut command: Command) -> io::Result<Interrupted> {
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut child = KillOnDrop(child);
    let (Some(stdout), Some(mut stderr_pipe)) = (child.0.stdout.take(), child.0.stderr.take())
    else {
        return Err(io::Error::other("stdout and stderr are not piped"));
    };
    let (lines_tx, lines) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines_tx.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + START_LIMIT;
    let mut written = Vec::new();
    while !(written.iter().any(|line: &String| is_data_transfer(line))
        && written.iter().any(|line: &String| is_info_finished(line)))
    {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = lines.recv_timeout(left).map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("no data file transfer started: {written:?}"),
            )
        })?;
        written.push(line);
    }
    let sent = Command::new("kill")
        .args(["-INT", &child.0.id().to_string()])
        .status()?;
    assert!(sent.success());
    let status = wait_for_exit(&mut child.0, EXIT_LIMIT)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("no exit within {EXIT_LIMIT:?} of SIGINT"),
        )
    })?;

    reader
        .join()
        .map_err(|_| io::Error::other("the stdout reader panicked"))?;
    written.extend(lines.try_iter());
    let mut stderr = String::new();
    stderr_pipe.read_to_string(&mut stderr)?;
    Ok(Interrupted {
        status,
        stdout: written,
        stderr,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn ctrl_c_during_an_upload_exits_with_130_and_leaves_the_recording_open() {
    let (server, _) = slow_ingest_api().await;
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("pick-cube");
    DatasetBuilder::new(Format::V3_0).write_to(&root).unwrap();

    let Interrupted {
        status,
        stdout: written,
        stderr,
    } = interrupt_upload(upload_command(&server, &root, &parent.path().join("home"))).unwrap();

    assert_eq!(status.code(), Some(130), "{stderr}");
    let envelope: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "interrupted");
    assert!(
        written
            .iter()
            .any(|line| line.contains("\"type\":\"session_created\"")),
        "{written:?}"
    );
    assert!(
        written
            .iter()
            .all(|line| !line.contains("\"type\":\"result\"")),
        "{written:?}"
    );
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|request| !request.url.path().ends_with(":complete")),
        "complete was requested"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn running_an_interrupted_upload_again_completes_the_same_recording() {
    let (server, stored) = slow_ingest_api().await;
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("pick-cube");
    let home = parent.path().join("home");
    DatasetBuilder::new(Format::V3_0).write_to(&root).unwrap();
    let interrupted = interrupt_upload(upload_command(&server, &root, &home)).unwrap();
    assert_eq!(
        interrupted.status.code(),
        Some(130),
        "{}",
        interrupted.stderr
    );
    make_fast(&server, &stored).await;

    let output = upload_command(&server, &root, &home).output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let events: Vec<serde_json::Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let resumed = events
        .iter()
        .find(|event| event["type"] == "session_resumed")
        .unwrap_or_else(|| panic!("no session_resumed event: {events:?}"));
    assert_eq!(resumed["session_id"], "3f2c9a");
    assert!(resumed["files_done"].as_u64() >= Some(1), "{resumed}");
    let result = events.last().unwrap();
    assert_eq!(
        (&result["type"], &result["session_id"], &result["resumed"]),
        (&json!("result"), &json!("3f2c9a"), &json!(true))
    );
    let requests = server.received_requests().await.unwrap();
    let paths: Vec<&str> = requests.iter().map(|request| request.url.path()).collect();
    assert!(!paths.contains(&RECORDINGS), "{paths:?}");
    assert!(
        !paths.contains(&format!("{}meta/info.json", recordings::STORAGE).as_str()),
        "{paths:?}"
    );
}
