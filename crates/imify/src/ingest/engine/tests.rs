use std::collections::HashMap;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use imify_test_support::{DatasetBuilder, Format, SyntheticDataset};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use transport::{BoxFuture, Digest};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::ingest::transport;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Started(String),
    Finished(String),
}

/// A transport that records when each file starts and finishes, without network I/O.
#[derive(Debug, Default)]
struct RecordingTransport {
    steps: Mutex<Vec<Step>>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    rejected: HashMap<String, String>,
}

impl RecordingTransport {
    fn rejecting(path: &str) -> Self {
        Self {
            rejected: HashMap::from([(path.to_owned(), "path rejected".to_owned())]),
            ..Self::default()
        }
    }

    fn steps(&self) -> Vec<Step> {
        self.steps.lock().unwrap().clone()
    }

    fn started(&self) -> Vec<String> {
        self.steps()
            .into_iter()
            .filter_map(|step| match step {
                Step::Started(path) => Some(path),
                Step::Finished(_) => None,
            })
            .collect()
    }
}

impl Transport for RecordingTransport {
    fn put<'a>(&'a self, upload: &'a FileUpload<'a>) -> BoxFuture<'a, Result<Digest, Error>> {
        Box::pin(async move {
            let path = upload.file.path().to_owned();
            let digest = Sha256::digest(fs::read(&upload.local_path).unwrap()).into();
            self.steps.lock().unwrap().push(Step::Started(path.clone()));
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(now, Ordering::SeqCst);
            (upload.progress)(upload.file.size() / 2);
            tokio::time::sleep(Duration::from_millis(5)).await;
            (upload.progress)(upload.file.size());
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            self.steps
                .lock()
                .unwrap()
                .push(Step::Finished(path.clone()));
            match self.rejected.get(&path) {
                Some(message) => Err(Error::Validation {
                    message: message.clone(),
                    fields: Vec::new(),
                }),
                None => Ok(digest),
            }
        })
    }
}

fn dataset(files: &[&str]) -> TempDir {
    let root = tempfile::tempdir().unwrap();
    for file in std::iter::once(&INFO_FILE).chain(files) {
        let path = root.path().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"0123456789").unwrap();
    }
    root
}

fn data_files(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("data/file-{index:03}"))
        .collect()
}

fn options(concurrency: usize) -> UploadOptions {
    UploadOptions {
        concurrency: NonZeroUsize::new(concurrency).unwrap(),
        request_interval: Duration::ZERO,
    }
}

fn uploader(base_url: &str, transport: &Arc<RecordingTransport>, concurrency: usize) -> Uploader {
    let mut policy = crate::RetryPolicy::none();
    policy.initial_backoff = Duration::from_millis(1);
    let client = Client::builder(base_url)
        .retry_policy(policy)
        .build()
        .unwrap();
    let transport: Arc<dyn Transport> = transport.clone();
    Uploader::with_transport(client, transport).options(options(concurrency))
}

fn offline_uploader(transport: &Arc<RecordingTransport>, concurrency: usize) -> Uploader {
    uploader("http://127.0.0.1:9", transport, concurrency)
}

fn recorder() -> (
    Arc<Mutex<Vec<UploadEvent>>>,
    impl Fn(&UploadEvent) + Send + Sync,
) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    (events, move |event: &UploadEvent| {
        sink.lock().unwrap().push(event.clone());
    })
}

fn session_body(status: &str) -> serde_json::Value {
    json!({
        "id": "3f2c9a",
        "name": "pick-place",
        "dataset_format": null,
        "robot_type": null,
        "fps": null,
        "total_episodes": null,
        "status": status,
        "task_description": "",
        "created_at": "2026-10-16T09:30:00Z"
    })
}

async fn session_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/ingest/sessions"))
        .respond_with(ResponseTemplate::new(201).set_body_json(session_body("uploading")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/ingest/sessions/3f2c9a/complete"))
        .respond_with(ResponseTemplate::new(200).set_body_json(session_body("ready")))
        .mount(&server)
        .await;
    server
}

fn session_id() -> SessionId {
    SessionId::new("3f2c9a")
}

#[tokio::test]
async fn no_other_file_starts_before_the_info_file_has_finished() {
    let root = dataset(&["data/a", "data/b", "videos/c"]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::default());

    offline_uploader(&transport, 4)
        .send_files(&session_id(), &plan)
        .await
        .unwrap();

    assert_eq!(
        transport.steps()[..2],
        [
            Step::Started(INFO_FILE.to_owned()),
            Step::Finished(INFO_FILE.to_owned())
        ]
    );
}

#[tokio::test]
async fn no_more_than_concurrency_files_are_in_flight() {
    let files = data_files(12);
    let root = dataset(&files.iter().map(String::as_str).collect::<Vec<_>>());
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::default());

    offline_uploader(&transport, 3)
        .send_files(&session_id(), &plan)
        .await
        .unwrap();

    assert_eq!(transport.max_in_flight.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn plan_without_the_info_file_starts_with_the_other_files() {
    let root = dataset(&["data/a", "data/b"]);
    let mut plan = UploadPlan::from_dir(root.path()).unwrap();
    plan.retain(|file| file.path() != INFO_FILE);
    let transport = Arc::new(RecordingTransport::default());

    offline_uploader(&transport, 4)
        .send_files(&session_id(), &plan)
        .await
        .unwrap();

    let mut started = transport.started();
    started.sort();
    assert_eq!(started, ["data/a", "data/b"]);
}

#[tokio::test]
async fn failing_file_ends_send_files_and_starts_no_further_files() {
    let root = dataset(&["data/a", "data/b", "data/c"]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::rejecting("data/b"));

    let error = offline_uploader(&transport, 1)
        .send_files(&session_id(), &plan)
        .await
        .unwrap_err();

    assert_eq!(
        (
            matches!(error, Error::Validation { ref message, .. } if message == "path rejected"),
            transport.started()
        ),
        (
            true,
            vec![INFO_FILE.to_owned(), "data/a".into(), "data/b".into()]
        )
    );
}

#[tokio::test]
async fn events_of_one_file_arrive_started_progress_finished() {
    let root = dataset(&["data/a"]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::default());
    let (events, handler) = recorder();

    offline_uploader(&transport, 4)
        .on_event(handler)
        .send_files(&session_id(), &plan)
        .await
        .unwrap();

    let of_file: Vec<UploadEvent> = events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| match event {
            UploadEvent::FileStarted { path, .. }
            | UploadEvent::FileProgress { path, .. }
            | UploadEvent::FileFinished { path, .. } => path == "data/a",
            _ => false,
        })
        .cloned()
        .collect();
    let path = String::from("data/a");
    assert_eq!(
        of_file,
        [
            UploadEvent::FileStarted {
                path: path.clone(),
                size: 10
            },
            UploadEvent::FileProgress {
                path: path.clone(),
                bytes_sent: 5
            },
            UploadEvent::FileProgress {
                path: path.clone(),
                bytes_sent: 10
            },
            UploadEvent::FileFinished { path, size: 10 },
        ]
    );
}

#[tokio::test]
async fn upload_reports_the_session_before_files_and_completion_after_them() {
    let server = session_server().await;
    let root = dataset(&["data/a", "data/b"]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::default());
    let (events, handler) = recorder();

    uploader(&server.uri(), &transport, 2)
        .on_event(handler)
        .upload(&plan, &NewSession::new("pick-place"))
        .await
        .unwrap();

    let events = events.lock().unwrap().clone();
    let position = |wanted: fn(&UploadEvent) -> bool| events.iter().position(wanted).unwrap();
    let last_finished = events
        .iter()
        .rposition(|event| matches!(event, UploadEvent::FileFinished { .. }))
        .unwrap();
    assert_eq!(
        (
            position(|event| matches!(event, UploadEvent::SessionCreated { .. })),
            position(|event| matches!(event, UploadEvent::Completing)),
        ),
        (0, last_finished + 1)
    );
}

#[tokio::test]
async fn upload_does_not_complete_when_a_file_fails() {
    let server = session_server().await;
    let root = dataset(&["data/a"]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::rejecting("data/a"));

    let result = uploader(&server.uri(), &transport, 2)
        .upload(&plan, &NewSession::new("pick-place"))
        .await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        (result.is_err(), requests.len(), requests[0].url.path()),
        (true, 1, "/api/v1/ingest/sessions")
    );
}

#[test]
fn debug_prints_the_options() {
    let transport = Arc::new(RecordingTransport::default());

    let debug = format!("{:?}", offline_uploader(&transport, 2));

    assert!(debug.starts_with("Uploader { options: UploadOptions { concurrency: 2"));
}

/// A two-episode v3.0 dataset and a journal directory beside it.
struct Resumable {
    dataset: SyntheticDataset,
    journals: TempDir,
}

impl Resumable {
    fn new() -> Self {
        Self {
            dataset: DatasetBuilder::new(Format::V3_0)
                .episodes(2)
                .build()
                .unwrap(),
            journals: tempfile::tempdir().unwrap(),
        }
    }

    fn plan(&self) -> UploadPlan {
        UploadPlan::from_dir(self.dataset.root()).unwrap()
    }

    fn paths(&self) -> Vec<String> {
        self.plan()
            .files()
            .iter()
            .map(|file| file.path().to_owned())
            .collect()
    }

    /// The path of the last file of the plan, which concurrency 1 sends last.
    fn last_path(&self) -> String {
        self.paths().pop().unwrap()
    }

    fn file(&self, path: &str) -> PathBuf {
        self.dataset.root().join(path)
    }

    fn uploader(
        &self,
        server: &MockServer,
        transport: &Arc<RecordingTransport>,
        policy: ResumePolicy,
    ) -> Uploader {
        uploader(&server.uri(), transport, 1).journal(self.journals.path(), policy)
    }

    async fn upload(
        &self,
        server: &MockServer,
        transport: RecordingTransport,
        policy: ResumePolicy,
        session: &NewSession,
    ) -> (Result<Session, Error>, Vec<String>) {
        let transport = Arc::new(transport);
        let result = self
            .uploader(server, &transport, policy)
            .upload(&self.plan(), session)
            .await;
        (result, transport.started())
    }

    /// Runs an upload that fails on the last file of the plan.
    async fn fail_on_last_file(&self, server: &MockServer) {
        let (result, _) = self
            .upload(
                server,
                RecordingTransport::rejecting(&self.last_path()),
                ResumePolicy::IfJournaled,
                &NewSession::new("pick-place"),
            )
            .await;
        assert!(result.is_err());
    }

    async fn rerun(
        &self,
        server: &MockServer,
        policy: ResumePolicy,
    ) -> (Result<Session, Error>, Vec<String>) {
        self.upload(
            server,
            RecordingTransport::default(),
            policy,
            &NewSession::new("pick-place"),
        )
        .await
    }

    fn journal_files(&self) -> Vec<PathBuf> {
        fs::read_dir(self.journals.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect()
    }

    fn journal_lines(&self) -> Vec<serde_json::Value> {
        let [journal] = &self.journal_files()[..] else {
            panic!("expected one journal, found {:?}", self.journal_files());
        };
        fs::read_to_string(journal)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

async fn request_count(server: &MockServer, wanted: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == wanted)
        .count()
}

const SESSIONS: &str = "/api/v1/ingest/sessions";
const COMPLETE: &str = "/api/v1/ingest/sessions/3f2c9a/complete";

fn is_journal_error(result: &Result<Session, Error>, wanted: fn(&JournalError) -> bool) -> bool {
    matches!(result, Err(Error::Journal { reason, .. }) if wanted(reason))
}

#[tokio::test]
async fn failed_upload_keeps_a_journal_of_the_session_and_the_files_stored_before_the_failure() {
    let server = session_server().await;
    let resumable = Resumable::new();

    resumable.fail_on_last_file(&server).await;

    let lines = resumable.journal_lines();
    let journaled: Vec<&str> = lines[1..]
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    let mut stored = resumable.paths();
    stored.pop();
    assert_eq!(
        (lines[0]["session"].as_str(), journaled),
        (Some("3f2c9a"), stored.iter().map(String::as_str).collect())
    );
}

#[tokio::test]
async fn upload_whose_session_cannot_be_created_leaves_no_journal() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SESSIONS))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({ "detail": "invalid name" })))
        .mount(&server)
        .await;
    let resumable = Resumable::new();

    let (result, _) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!((result.is_err(), resumable.journal_files()), (true, vec![]));
}

#[tokio::test]
async fn rerun_after_a_failure_sends_only_the_unjournaled_files_to_the_journaled_session() {
    let server = session_server().await;
    let resumable = Resumable::new();
    resumable.fail_on_last_file(&server).await;

    let (result, started) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!(
        (
            result.ok().map(|session| session.id),
            started,
            request_count(&server, SESSIONS).await
        ),
        (Some(session_id()), vec![resumable.last_path()], 1)
    );
}

#[tokio::test]
async fn rerun_reports_the_session_resumed_with_the_files_and_bytes_not_sent_again() {
    let server = session_server().await;
    let resumable = Resumable::new();
    resumable.fail_on_last_file(&server).await;
    let plan = resumable.plan();
    let (done, _) = plan.files().split_at(plan.files().len() - 1);
    let (events, handler) = recorder();
    let transport = Arc::new(RecordingTransport::default());

    resumable
        .uploader(&server, &transport, ResumePolicy::IfJournaled)
        .on_event(handler)
        .upload(&plan, &NewSession::new("pick-place"))
        .await
        .unwrap();

    assert_eq!(
        events.lock().unwrap()[0],
        UploadEvent::SessionResumed {
            session: session_id(),
            files_done: done.len(),
            bytes_done: done.iter().map(PlannedFile::size).sum(),
        }
    );
}

#[tokio::test]
async fn journaled_file_whose_size_changed_is_sent_again() {
    let server = session_server().await;
    let resumable = Resumable::new();
    resumable.fail_on_last_file(&server).await;
    let mut info = fs::read(resumable.file(INFO_FILE)).unwrap();
    info.push(b'\n');
    fs::write(resumable.file(INFO_FILE), info).unwrap();

    let (_, started) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!(started, [INFO_FILE.to_owned(), resumable.last_path()]);
}

fn set_modified(path: &Path, seconds_since_epoch: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds_since_epoch))
        .unwrap();
}

#[tokio::test]
async fn journaled_file_whose_mtime_changed_and_content_did_not_is_not_sent_again() {
    let server = session_server().await;
    let resumable = Resumable::new();
    resumable.fail_on_last_file(&server).await;
    set_modified(&resumable.file(INFO_FILE), 1_000_000);

    let (_, started) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!(started, [resumable.last_path()]);
}

#[tokio::test]
async fn journaled_file_whose_mtime_and_content_changed_at_the_same_size_is_sent_again() {
    let server = session_server().await;
    let resumable = Resumable::new();
    resumable.fail_on_last_file(&server).await;
    let info = resumable.file(INFO_FILE);
    let changed: Vec<u8> = fs::read(&info)
        .unwrap()
        .iter()
        .map(|byte| if *byte == b'3' { b'4' } else { *byte })
        .collect();
    fs::write(&info, changed).unwrap();
    set_modified(&info, 1_000_000);

    let (_, started) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!(started, [INFO_FILE.to_owned(), resumable.last_path()]);
}

#[tokio::test]
async fn journaled_file_that_is_gone_fails_with_mismatch_before_any_request() {
    let server = session_server().await;
    let resumable = Resumable::new();
    resumable.fail_on_last_file(&server).await;
    let requests = server.received_requests().await.unwrap().len();
    let gone = resumable.paths()[1].clone();
    fs::remove_file(resumable.file(&gone)).unwrap();

    let (result, _) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!(
        (
            is_journal_error(&result, |reason| matches!(
                reason,
                JournalError::Mismatch(_)
            )),
            server.received_requests().await.unwrap().len()
        ),
        (true, requests)
    );
}

#[tokio::test]
async fn journal_of_another_task_description_fails_with_mismatch_before_any_request() {
    let server = session_server().await;
    let resumable = Resumable::new();
    resumable.fail_on_last_file(&server).await;
    let requests = server.received_requests().await.unwrap().len();

    let (result, _) = resumable
        .upload(
            &server,
            RecordingTransport::default(),
            ResumePolicy::IfJournaled,
            &NewSession::new("pick-place").task_description("Stack the cups"),
        )
        .await;

    assert_eq!(
        (
            is_journal_error(&result, |reason| matches!(
                reason,
                JournalError::Mismatch(_)
            )),
            server.received_requests().await.unwrap().len()
        ),
        (true, requests)
    );
}

#[tokio::test]
async fn required_resume_without_a_journal_fails_with_missing_before_any_request() {
    let server = session_server().await;
    let resumable = Resumable::new();

    let (result, _) = resumable.rerun(&server, ResumePolicy::Required).await;

    assert_eq!(
        (
            is_journal_error(&result, |reason| matches!(reason, JournalError::Missing)),
            server.received_requests().await.unwrap().len()
        ),
        (true, 0)
    );
}

#[tokio::test]
async fn restart_ignores_the_journal_and_creates_a_new_session() {
    let server = session_server().await;
    let resumable = Resumable::new();
    resumable.fail_on_last_file(&server).await;

    let (_, started) = resumable.rerun(&server, ResumePolicy::Restart).await;

    assert_eq!(
        (started, request_count(&server, SESSIONS).await),
        (resumable.paths(), 2)
    );
}

#[tokio::test]
async fn restart_deletes_the_journal() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SESSIONS))
        .respond_with(ResponseTemplate::new(201).set_body_json(session_body("uploading")))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let resumable = Resumable::new();
    resumable.fail_on_last_file(&server).await;

    let (result, _) = resumable.rerun(&server, ResumePolicy::Restart).await;

    assert_eq!((result.is_err(), resumable.journal_files()), (true, vec![]));
}

/// Returns whether the only journal in `journals` has an entry for `path`.
fn is_journaled(journals: &Path, path: &str) -> bool {
    let entries: Vec<_> = fs::read_dir(journals).unwrap().collect();
    let [Ok(journal)] = &entries[..] else {
        return false;
    };
    fs::read_to_string(journal.path())
        .unwrap()
        .lines()
        .skip(1)
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .any(|entry| entry["path"] == path)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_file_is_journaled_before_its_file_finished_event_with_concurrency_4() {
    let server = session_server().await;
    let files = data_files(16);
    let root = dataset(&files.iter().map(String::as_str).collect::<Vec<_>>());
    let journals = tempfile::tempdir().unwrap();
    let journal_dir = journals.path().to_owned();
    let finished = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&finished);
    let transport = Arc::new(RecordingTransport::default());

    uploader(&server.uri(), &transport, 4)
        .journal(journals.path(), ResumePolicy::IfJournaled)
        .on_event(move |event| {
            if let UploadEvent::FileFinished { path, .. } = event {
                let journaled = is_journaled(&journal_dir, path);
                sink.lock().unwrap().push((path.clone(), journaled));
            }
        })
        .upload(
            &UploadPlan::from_dir(root.path()).unwrap(),
            &NewSession::new("pick-place"),
        )
        .await
        .unwrap();

    let finished = finished.lock().unwrap().clone();
    let not_journaled: Vec<&String> = finished
        .iter()
        .filter(|(_, journaled)| !journaled)
        .map(|(path, _)| path)
        .collect();
    assert_eq!(
        (finished.len(), not_journaled),
        (files.len() + 1, Vec::<&String>::new())
    );
}

#[tokio::test]
async fn successful_complete_deletes_the_journal() {
    let server = session_server().await;
    let resumable = Resumable::new();

    let (result, _) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!((result.is_ok(), resumable.journal_files()), (true, vec![]));
}

#[tokio::test]
async fn failed_complete_keeps_the_journal_and_the_rerun_only_completes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(COMPLETE))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({ "detail": "not yet" })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(SESSIONS))
        .respond_with(ResponseTemplate::new(201).set_body_json(session_body("uploading")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(COMPLETE))
        .respond_with(ResponseTemplate::new(200).set_body_json(session_body("ready")))
        .mount(&server)
        .await;
    let resumable = Resumable::new();
    let (failed, _) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;
    let kept = resumable.journal_files().len();

    let (result, started) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!(
        (
            failed.is_err(),
            kept,
            result.is_ok(),
            started,
            request_count(&server, SESSIONS).await,
            request_count(&server, COMPLETE).await
        ),
        (true, 1, true, Vec::<String>::new(), 1, 2)
    );
}

#[tokio::test]
async fn journal_directory_that_cannot_be_created_fails_with_io_before_any_request() {
    let server = session_server().await;
    let dataset = DatasetBuilder::new(Format::V3_0).build().unwrap();
    let blocker = tempfile::NamedTempFile::new().unwrap();
    let transport = Arc::new(RecordingTransport::default());

    let result = uploader(&server.uri(), &transport, 1)
        .journal(blocker.path().join("uploads"), ResumePolicy::IfJournaled)
        .upload(
            &UploadPlan::from_dir(dataset.root()).unwrap(),
            &NewSession::new("pick-place"),
        )
        .await;

    assert_eq!(
        (
            is_journal_error(&result, |reason| matches!(reason, JournalError::Io(_))),
            server.received_requests().await.unwrap().len()
        ),
        (true, 0)
    );
}

#[tokio::test]
async fn upload_without_a_journal_writes_no_file() {
    let server = session_server().await;
    let dataset = DatasetBuilder::new(Format::V3_0).build().unwrap();
    let transport = Arc::new(RecordingTransport::default());

    uploader(&server.uri(), &transport, 1)
        .upload(
            &UploadPlan::from_dir(dataset.root()).unwrap(),
            &NewSession::new("pick-place"),
        )
        .await
        .unwrap();

    let mut written = Vec::new();
    collect_files(dataset.root(), dataset.root(), &mut written);
    assert_eq!(written, dataset.files());
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_files(root, &path, files);
        } else {
            files.push(path.strip_prefix(root).unwrap().to_owned());
        }
    }
    files.sort();
}
