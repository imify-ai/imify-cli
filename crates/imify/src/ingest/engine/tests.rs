use std::collections::{HashMap, VecDeque};
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use imify_test_support::{DatasetBuilder, Format, SyntheticDataset};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use transport::BoxFuture;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::ingest::fake_api::{
    self, BATCH_CREATE, COMPLETE, RECORDING, RECORDING_PATH, RECORDINGS, Storage,
};
use crate::ingest::transport;

const INFO_FILE: &str = "meta/info.json";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Started(String),
    Finished(String),
}

/// A transport that records when each file starts and finishes, without network I/O. A
/// file it sends is held by `storage`; a file with planned failures fails with the next.
#[derive(Debug, Default)]
struct RecordingTransport {
    steps: Mutex<Vec<Step>>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    failures: Mutex<HashMap<String, VecDeque<UploadError>>>,
    storage: Storage,
}

impl RecordingTransport {
    fn new(storage: &Storage) -> Self {
        Self {
            storage: storage.clone(),
            ..Self::default()
        }
    }

    /// Fails each attempt of `path` with the next of `failures`.
    fn failing(self, path: &str, failures: &[UploadError]) -> Self {
        self.failures
            .lock()
            .unwrap()
            .insert(path.to_owned(), failures.iter().cloned().collect());
        self
    }

    fn rejecting(storage: &Storage, path: &str) -> Self {
        Self::new(storage).failing(
            path,
            &[UploadError::Storage {
                status: 400,
                code: Some("InvalidRequest".into()),
            }],
        )
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
    fn put<'a>(&'a self, upload: &'a FileUpload<'a>) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let path = upload.file.path().to_owned();
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
            let failure = self
                .failures
                .lock()
                .unwrap()
                .get_mut(&path)
                .and_then(VecDeque::pop_front);
            if let Some(reason) = failure {
                return Err(Error::Upload { path, reason });
            }
            self.storage.store(&path);
            Ok(())
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

/// A recordings API whose upload URLs point to `https://storage.example.com`.
async fn api(storage: &Storage) -> MockServer {
    let server = MockServer::start().await;
    fake_api::mount(&server, storage, "https://storage.example.com/bucket").await;
    server
}

fn recording_id() -> RecordingId {
    RecordingId::new(RECORDING)
}

fn new_recording() -> NewRecording {
    NewRecording::new("pick-place").task("Pick up the red cube")
}

async fn request_count(server: &MockServer, wanted: &str) -> usize {
    fake_api::requests_to(server, wanted).await.len()
}

fn sha256_hex(content: &[u8]) -> String {
    hex(&Sha256::digest(content).into())
}

#[tokio::test]
async fn no_more_than_concurrency_files_are_in_flight() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let files = data_files(12);
    let root = dataset(&files.iter().map(String::as_str).collect::<Vec<_>>());
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::new(&storage));

    uploader(&server.uri(), &transport, 3)
        .send_files(&recording_id(), &plan)
        .await
        .unwrap();

    assert_eq!(transport.max_in_flight.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn send_files_announces_every_file_with_its_sha256() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let root = dataset(&["data/a", "videos/b"]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::new(&storage));

    uploader(&server.uri(), &transport, 2)
        .send_files(&recording_id(), &plan)
        .await
        .unwrap();

    let announced = storage.announced();
    let digest = sha256_hex(b"0123456789");
    assert_eq!(
        (
            announced.len(),
            announced.values().all(|sha256| *sha256 == digest)
        ),
        (3, true)
    );
}

#[tokio::test]
async fn send_files_sends_only_the_files_storage_lacks() {
    let storage = Storage::default();
    storage.store("data/a");
    let server = api(&storage).await;
    let root = dataset(&["data/a", "data/b"]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::new(&storage));

    uploader(&server.uri(), &transport, 1)
        .send_files(&recording_id(), &plan)
        .await
        .unwrap();

    assert_eq!(transport.started(), [INFO_FILE, "data/b"]);
}

#[tokio::test]
async fn failing_file_ends_send_files_and_starts_no_further_files() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let root = dataset(&["data/a", "data/b", "data/c"]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::rejecting(&storage, "data/b"));

    let error = uploader(&server.uri(), &transport, 1)
        .send_files(&recording_id(), &plan)
        .await
        .unwrap_err();

    assert_eq!(
        (
            matches!(
                error,
                Error::Upload {
                    reason: UploadError::Storage { status: 400, .. },
                    ..
                }
            ),
            transport.started()
        ),
        (
            true,
            vec![INFO_FILE.to_owned(), "data/a".into(), "data/b".into()]
        )
    );
}

#[tokio::test]
async fn an_expired_url_is_renewed_once_and_the_file_sent_again() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let root = dataset(&[]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport =
        Arc::new(RecordingTransport::new(&storage).failing(INFO_FILE, &[UploadError::UrlExpired]));

    uploader(&server.uri(), &transport, 1)
        .send_files(&recording_id(), &plan)
        .await
        .unwrap();

    assert_eq!(
        (
            transport.started().len(),
            request_count(&server, BATCH_CREATE).await,
            storage.holds(INFO_FILE)
        ),
        (2, 2, true)
    );
}

#[tokio::test]
async fn a_url_rejected_again_after_the_renewal_fails_with_url_expired() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let root = dataset(&[]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::new(&storage).failing(
        INFO_FILE,
        &[UploadError::UrlExpired, UploadError::UrlExpired],
    ));

    let error = uploader(&server.uri(), &transport, 1)
        .send_files(&recording_id(), &plan)
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            Error::Upload {
                reason: UploadError::UrlExpired,
                ..
            }
        ),
        "{error:?}"
    );
    assert_eq!(transport.started().len(), 2);
}

#[tokio::test]
async fn a_checksum_mismatch_reads_the_file_once_more_then_fails() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let root = dataset(&[]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::new(&storage).failing(
        INFO_FILE,
        &[UploadError::ChecksumMismatch, UploadError::ChecksumMismatch],
    ));

    let error = uploader(&server.uri(), &transport, 1)
        .send_files(&recording_id(), &plan)
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            Error::Upload {
                reason: UploadError::ChecksumMismatch,
                ..
            }
        ),
        "{error:?}"
    );
    assert_eq!(
        (
            transport.started().len(),
            request_count(&server, BATCH_CREATE).await
        ),
        (2, 1)
    );
}

#[tokio::test]
async fn a_renewal_and_a_second_read_each_report_the_file_retrying() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let root = dataset(&[]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::new(&storage).failing(
        INFO_FILE,
        &[UploadError::UrlExpired, UploadError::ChecksumMismatch],
    ));
    let (events, handler) = recorder();

    uploader(&server.uri(), &transport, 1)
        .on_event(handler)
        .send_files(&recording_id(), &plan)
        .await
        .unwrap();

    let attempts: Vec<u32> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            UploadEvent::FileRetrying { attempt, .. } => Some(*attempt),
            _ => None,
        })
        .collect();
    assert_eq!(attempts, [1, 2]);
}

#[tokio::test]
async fn events_of_one_file_arrive_started_progress_finished() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let root = dataset(&["data/a"]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::new(&storage));
    let (events, handler) = recorder();

    uploader(&server.uri(), &transport, 4)
        .on_event(handler)
        .send_files(&recording_id(), &plan)
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
async fn upload_reports_the_recording_before_files_and_completion_after_them() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let root = dataset(&["data/a", "data/b"]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::new(&storage));
    let (events, handler) = recorder();

    uploader(&server.uri(), &transport, 2)
        .on_event(handler)
        .upload(&plan, &new_recording())
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
            position(|event| matches!(event, UploadEvent::RecordingCreated { .. })),
            position(|event| matches!(event, UploadEvent::Completing)),
        ),
        (0, last_finished + 1)
    );
}

#[tokio::test]
async fn upload_returns_the_completed_recording() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let root = dataset(&["data/a"]);
    let transport = Arc::new(RecordingTransport::new(&storage));

    let recording = uploader(&server.uri(), &transport, 2)
        .upload(
            &UploadPlan::from_dir(root.path()).unwrap(),
            &new_recording(),
        )
        .await
        .unwrap();

    assert_eq!(
        (
            recording.id,
            recording.state,
            recording.dataset_id.as_deref(),
            recording.episode_count
        ),
        (recording_id(), RecordingState::Ready, Some("d-41"), 2)
    );
}

#[tokio::test]
async fn upload_does_not_complete_when_a_file_fails() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let root = dataset(&[]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let transport = Arc::new(RecordingTransport::rejecting(&storage, INFO_FILE));

    let result = uploader(&server.uri(), &transport, 2)
        .upload(&plan, &new_recording())
        .await;

    let paths: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| request.url.path().to_owned())
        .collect();
    assert_eq!(
        (result.is_err(), paths),
        (true, vec![RECORDINGS.to_owned(), BATCH_CREATE.to_owned()])
    );
}

#[tokio::test]
async fn upload_of_a_recording_created_as_done_sends_no_file() {
    let storage = Storage::default();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(RECORDINGS))
        .respond_with(ResponseTemplate::new(201).set_body_json(fake_api::recording("PROCESSING")))
        .mount(&server)
        .await;
    let root = dataset(&["data/a"]);
    let transport = Arc::new(RecordingTransport::new(&storage));

    let recording = uploader(&server.uri(), &transport, 2)
        .upload(
            &UploadPlan::from_dir(root.path()).unwrap(),
            &new_recording(),
        )
        .await
        .unwrap();

    assert_eq!(
        (
            recording.state,
            transport.started(),
            server.received_requests().await.unwrap().len()
        ),
        (RecordingState::Processing, Vec::<String>::new(), 1)
    );
}

#[tokio::test]
async fn upload_with_a_setting_outside_the_limits_fails_before_any_request() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let root = dataset(&[]);
    let transport = Arc::new(RecordingTransport::new(&storage));

    let error = uploader(&server.uri(), &transport, 2)
        .upload(
            &UploadPlan::from_dir(root.path()).unwrap(),
            &new_recording().label("Shift", "night"),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Validation { .. }), "{error:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn upload_creates_the_recording_with_a_new_uuid_request_id() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let root = dataset(&[]);
    let transport = Arc::new(RecordingTransport::new(&storage));
    let upload = uploader(&server.uri(), &transport, 2);
    let plan = UploadPlan::from_dir(root.path()).unwrap();

    upload.upload(&plan, &new_recording()).await.unwrap();
    upload.upload(&plan, &new_recording()).await.unwrap();

    let ids: Vec<String> = fake_api::requests_to(&server, RECORDINGS)
        .await
        .iter()
        .map(|request| {
            let body: serde_json::Value = request.body_json().unwrap();
            body["requestId"].as_str().unwrap().to_owned()
        })
        .collect();
    assert!(
        ids.len() == 2
            && ids[0] != ids[1]
            && ids
                .iter()
                .all(|id| id.len() == 36 && id.as_bytes()[14] == b'4'),
        "{ids:?}"
    );
}

#[test]
fn debug_prints_the_options() {
    let transport = Arc::new(RecordingTransport::default());

    let debug = format!("{:?}", uploader("http://127.0.0.1:9", &transport, 2));

    assert!(debug.starts_with("Uploader { options: UploadOptions { concurrency: 2"));
}

/// A two-episode v3.0 dataset, a journal directory beside it and the storage of a fake
/// API.
struct Resumable {
    dataset: SyntheticDataset,
    journals: TempDir,
    storage: Storage,
}

impl Resumable {
    fn new() -> Self {
        Self {
            dataset: DatasetBuilder::new(Format::V3_0)
                .episodes(2)
                .build()
                .unwrap(),
            journals: tempfile::tempdir().unwrap(),
            storage: Storage::default(),
        }
    }

    async fn api(&self) -> MockServer {
        api(&self.storage).await
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
        recording: &NewRecording,
    ) -> (Result<Recording, Error>, Vec<String>) {
        let transport = Arc::new(transport);
        let result = self
            .uploader(server, &transport, policy)
            .upload(&self.plan(), recording)
            .await;
        (result, transport.started())
    }

    /// Runs an upload that fails on the last file of the plan.
    async fn fail_on_last_file(&self, server: &MockServer) {
        let (result, _) = self
            .upload(
                server,
                RecordingTransport::rejecting(&self.storage, &self.last_path()),
                ResumePolicy::IfJournaled,
                &new_recording(),
            )
            .await;
        assert!(result.is_err());
    }

    async fn rerun(
        &self,
        server: &MockServer,
        policy: ResumePolicy,
    ) -> (Result<Recording, Error>, Vec<String>) {
        self.upload(
            server,
            RecordingTransport::new(&self.storage),
            policy,
            &new_recording(),
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

fn is_journal_error(result: &Result<Recording, Error>, wanted: fn(&JournalError) -> bool) -> bool {
    matches!(result, Err(Error::Journal { reason, .. }) if wanted(reason))
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
async fn failed_upload_keeps_a_journal_of_the_recording_and_the_files_uploaded_before() {
    let resumable = Resumable::new();
    let server = resumable.api().await;

    resumable.fail_on_last_file(&server).await;

    let lines = resumable.journal_lines();
    let journaled: Vec<&str> = lines[1..]
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    let mut uploaded = resumable.paths();
    uploaded.pop();
    assert_eq!(
        (
            lines[0]["version"].as_u64(),
            lines[0]["recording"].as_str(),
            lines[0]["request_id"].as_str().map(str::len),
            lines[0]["settings"]["task"].as_str(),
            journaled
        ),
        (
            Some(2),
            Some(RECORDING),
            Some(36),
            Some("Pick up the red cube"),
            uploaded.iter().map(String::as_str).collect()
        )
    );
}

#[tokio::test]
async fn upload_whose_recording_cannot_be_created_leaves_no_journal() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(RECORDINGS))
        .respond_with(fake_api::api_error(400, "INVALID_ARGUMENT", "INVALID_TASK"))
        .mount(&server)
        .await;
    let resumable = Resumable::new();

    let (result, _) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!((result.is_err(), resumable.journal_files()), (true, vec![]));
}

#[tokio::test]
async fn rerun_after_a_failure_sends_only_the_files_storage_lacks_to_the_same_recording() {
    let resumable = Resumable::new();
    let server = resumable.api().await;
    resumable.fail_on_last_file(&server).await;

    let (result, started) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!(
        (
            result.ok().map(|recording| recording.id),
            started,
            request_count(&server, RECORDINGS).await,
            request_count(&server, RECORDING_PATH).await
        ),
        (Some(recording_id()), vec![resumable.last_path()], 1, 1)
    );
}

#[tokio::test]
async fn rerun_reports_the_recording_resumed_with_the_files_storage_holds() {
    let resumable = Resumable::new();
    let server = resumable.api().await;
    resumable.fail_on_last_file(&server).await;
    let plan = resumable.plan();
    let (done, _) = plan.files().split_at(plan.files().len() - 1);
    let (events, handler) = recorder();
    let transport = Arc::new(RecordingTransport::new(&resumable.storage));

    resumable
        .uploader(&server, &transport, ResumePolicy::IfJournaled)
        .on_event(handler)
        .upload(&plan, &new_recording())
        .await
        .unwrap();

    assert_eq!(
        events.lock().unwrap()[0],
        UploadEvent::RecordingResumed {
            recording: recording_id(),
            files_done: done.len(),
            bytes_done: done.iter().map(PlannedFile::size).sum(),
        }
    );
}

#[tokio::test]
async fn rerun_sends_a_journaled_file_again_that_storage_lacks() {
    let resumable = Resumable::new();
    let server = resumable.api().await;
    resumable.fail_on_last_file(&server).await;
    resumable.storage.forget(INFO_FILE);

    let (_, started) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!(started, [INFO_FILE.to_owned(), resumable.last_path()]);
}

#[tokio::test]
async fn rerun_announces_the_journaled_sha256_of_a_file_with_unchanged_size_and_mtime() {
    let resumable = Resumable::new();
    let server = resumable.api().await;
    resumable.fail_on_last_file(&server).await;
    let info = resumable.file(INFO_FILE);
    let original = fs::read(&info).unwrap();
    let modified = fs::metadata(&info).unwrap().modified().unwrap();
    let changed: Vec<u8> = original
        .iter()
        .map(|byte| if *byte == b'3' { b'4' } else { *byte })
        .collect();
    fs::write(&info, changed).unwrap();
    fs::File::options()
        .write(true)
        .open(&info)
        .unwrap()
        .set_modified(modified)
        .unwrap();

    let (result, _) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        resumable.storage.announced()[INFO_FILE],
        sha256_hex(&original)
    );
}

#[tokio::test]
async fn rerun_announces_a_new_sha256_for_a_journaled_file_whose_mtime_changed() {
    let resumable = Resumable::new();
    let server = resumable.api().await;
    resumable.fail_on_last_file(&server).await;
    let info = resumable.file(INFO_FILE);
    let changed: Vec<u8> = fs::read(&info)
        .unwrap()
        .iter()
        .map(|byte| if *byte == b'3' { b'4' } else { *byte })
        .collect();
    fs::write(&info, &changed).unwrap();
    set_modified(&info, 1_000_000);

    let (result, _) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        resumable.storage.announced()[INFO_FILE],
        sha256_hex(&changed)
    );
}

#[tokio::test]
async fn rerun_of_a_recording_whose_upload_is_done_returns_it_and_deletes_the_journal() {
    let resumable = Resumable::new();
    let server = resumable.api().await;
    resumable.fail_on_last_file(&server).await;
    Mock::given(method("GET"))
        .and(path(RECORDING_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(fake_api::recording("READY")))
        .with_priority(1)
        .mount(&server)
        .await;

    let (result, started) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!(
        (
            result.ok().map(|recording| recording.state),
            started,
            request_count(&server, COMPLETE).await,
            resumable.journal_files()
        ),
        (
            Some(RecordingState::Ready),
            Vec::<String>::new(),
            0,
            Vec::<PathBuf>::new()
        )
    );
}

#[tokio::test]
async fn journaled_file_that_is_gone_fails_with_mismatch_before_any_request() {
    let resumable = Resumable::new();
    let server = resumable.api().await;
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
async fn journal_of_other_settings_fails_with_mismatch_before_any_request() {
    let resumable = Resumable::new();
    let server = resumable.api().await;
    resumable.fail_on_last_file(&server).await;
    let requests = server.received_requests().await.unwrap().len();

    for other in [
        new_recording().task("Stack the cups"),
        new_recording().robot("so100-0042"),
    ] {
        let (result, _) = resumable
            .upload(
                &server,
                RecordingTransport::new(&resumable.storage),
                ResumePolicy::IfJournaled,
                &other,
            )
            .await;

        assert!(
            is_journal_error(&result, |reason| matches!(
                reason,
                JournalError::Mismatch(_)
            )),
            "{result:?}"
        );
    }
    assert_eq!(server.received_requests().await.unwrap().len(), requests);
}

#[tokio::test]
async fn journal_of_an_older_version_fails_with_outdated_before_any_request() {
    let resumable = Resumable::new();
    let server = resumable.api().await;
    resumable.fail_on_last_file(&server).await;
    let [journal] = &resumable.journal_files()[..] else {
        panic!("expected one journal");
    };
    let header = json!({
        "version": 1,
        "session": "3f2c9a",
        "name": "pick-place",
        "task_description": "Pick up the red cube"
    });
    fs::write(journal, format!("{header}\n")).unwrap();
    let requests = server.received_requests().await.unwrap().len();

    let (result, _) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!(
        (
            is_journal_error(&result, |reason| matches!(
                reason,
                JournalError::Outdated(1)
            )),
            server.received_requests().await.unwrap().len()
        ),
        (true, requests)
    );
}

#[tokio::test]
async fn required_resume_without_a_journal_fails_with_missing_before_any_request() {
    let resumable = Resumable::new();
    let server = resumable.api().await;

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
async fn restart_ignores_the_journal_and_creates_a_new_recording() {
    let resumable = Resumable::new();
    let server = resumable.api().await;
    resumable.fail_on_last_file(&server).await;

    let (result, _) = resumable.rerun(&server, ResumePolicy::Restart).await;

    assert!(result.is_ok(), "{result:?}");
    assert_eq!(request_count(&server, RECORDINGS).await, 2);
}

#[tokio::test]
async fn restart_deletes_the_journal() {
    let server = MockServer::start().await;
    let resumable = Resumable::new();
    fake_api::mount(&server, &resumable.storage, "https://storage.example.com").await;
    resumable.fail_on_last_file(&server).await;
    server.reset().await;

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
    let storage = Storage::default();
    let server = api(&storage).await;
    let files = data_files(16);
    let root = dataset(&files.iter().map(String::as_str).collect::<Vec<_>>());
    let journals = tempfile::tempdir().unwrap();
    let journal_dir = journals.path().to_owned();
    let finished = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&finished);
    let transport = Arc::new(RecordingTransport::new(&storage));

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
            &new_recording(),
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
    let resumable = Resumable::new();
    let server = resumable.api().await;

    let (result, _) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!((result.is_ok(), resumable.journal_files()), (true, vec![]));
}

#[tokio::test]
async fn failed_complete_keeps_the_journal_and_the_rerun_only_completes() {
    let resumable = Resumable::new();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(COMPLETE))
        .respond_with(fake_api::api_error(
            412,
            "FAILED_PRECONDITION",
            "FILES_NOT_UPLOADED",
        ))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    fake_api::mount(&server, &resumable.storage, "https://storage.example.com").await;
    let (failed, _) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;
    let kept = resumable.journal_files().len();

    let (result, started) = resumable.rerun(&server, ResumePolicy::IfJournaled).await;

    assert_eq!(
        (
            matches!(failed, Err(Error::FailedPrecondition { .. })),
            kept,
            result.is_ok(),
            started,
            request_count(&server, RECORDINGS).await,
            request_count(&server, COMPLETE).await
        ),
        (true, 1, true, Vec::<String>::new(), 1, 2)
    );
}

#[tokio::test]
async fn journal_directory_that_cannot_be_created_fails_with_io_before_any_request() {
    let storage = Storage::default();
    let server = api(&storage).await;
    let dataset = DatasetBuilder::new(Format::V3_0).build().unwrap();
    let blocker = tempfile::NamedTempFile::new().unwrap();
    let transport = Arc::new(RecordingTransport::new(&storage));

    let result = uploader(&server.uri(), &transport, 1)
        .journal(blocker.path().join("uploads"), ResumePolicy::IfJournaled)
        .upload(
            &UploadPlan::from_dir(dataset.root()).unwrap(),
            &new_recording(),
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
    let storage = Storage::default();
    let server = api(&storage).await;
    let dataset = DatasetBuilder::new(Format::V3_0).build().unwrap();
    let transport = Arc::new(RecordingTransport::new(&storage));

    uploader(&server.uri(), &transport, 1)
        .upload(
            &UploadPlan::from_dir(dataset.root()).unwrap(),
            &new_recording(),
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
