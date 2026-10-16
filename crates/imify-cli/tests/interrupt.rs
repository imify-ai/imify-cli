//! Ctrl-C during `imify datasets upload`, sent as SIGINT to the binary.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use imify_test_support::{DatasetBuilder, Format};
use serde_json::json;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const SESSIONS: &str = "/api/v1/ingest/sessions";
const FILES: &str = "/api/v1/ingest/sessions/3f2c9a/files/";
/// Time a data file `PUT` takes to answer: longer than the test waits for the exit.
const SLOW_ANSWER: Duration = Duration::from_secs(30);
/// Time allowed from the start of the binary to the first data file transfer.
const START_LIMIT: Duration = Duration::from_secs(30);
/// Time allowed from SIGINT to the exit of the binary.
const EXIT_LIMIT: Duration = Duration::from_secs(10);

/// Answers a file `PUT` with the stored path and size after `delay`.
struct Store {
    delay: Duration,
}

impl Respond for Store {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let stored = request.url.path().strip_prefix(FILES).unwrap_or_default();
        ResponseTemplate::new(200)
            .set_body_json(json!({ "path": stored, "size": request.body.len() }))
            .set_delay(self.delay)
    }
}

/// An ingest API that stores `meta/info.json` at once and answers every other file after
/// [`SLOW_ANSWER`]. It has no `complete` endpoint.
async fn slow_ingest_api() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SESSIONS))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "3f2c9a",
            "name": "pick-cube",
            "dataset_format": null,
            "robot_type": null,
            "fps": null,
            "total_episodes": null,
            "status": "uploading",
            "task_description": "Pick up the red cube",
            "created_at": "2026-10-16T09:30:00Z"
        })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("{FILES}meta/info.json")))
        .respond_with(Store {
            delay: Duration::ZERO,
        })
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(format!("^{FILES}")))
        .respond_with(Store { delay: SLOW_ANSWER })
        .mount(&server)
        .await;
    server
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

#[tokio::test(flavor = "multi_thread")]
async fn ctrl_c_during_an_upload_exits_with_130_and_leaves_the_session_open() {
    let server = slow_ingest_api().await;
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("pick-cube");
    DatasetBuilder::new(Format::V3_0).write_to(&root).unwrap();

    let child = Command::new(env!("CARGO_BIN_EXE_imify"))
        .env_remove("IMIFY_OUTPUT")
        .env_remove("NO_COLOR")
        .env("IMIFY_TOKEN", "tok_interrupt")
        .env("IMIFY_API_URL", server.uri())
        .current_dir(&root)
        .args(["datasets", "upload", ".", "--task", "Pick up the red cube"])
        .arg("--json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = KillOnDrop(child);
    let stdout = child.0.stdout.take().unwrap();
    let (lines_tx, lines) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if lines_tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + START_LIMIT;
    let mut written = Vec::new();
    while !written
        .last()
        .is_some_and(|line: &String| is_data_transfer(line))
    {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = lines
            .recv_timeout(left)
            .unwrap_or_else(|_| panic!("no data file transfer started: {written:?}"));
        written.push(line);
    }
    let sent = Command::new("kill")
        .args(["-INT", &child.0.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    let status = wait_for_exit(&mut child.0, EXIT_LIMIT)
        .unwrap()
        .unwrap_or_else(|| panic!("no exit within {EXIT_LIMIT:?} of SIGINT"));

    reader.join().unwrap();
    written.extend(lines.try_iter());
    let mut stderr = String::new();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
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
            .all(|request| !request.url.path().ends_with("/complete")),
        "complete was requested"
    );
}
