use std::collections::HashMap;
use std::fs;
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;
use transport::BoxFuture;
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
            match self.rejected.get(&path) {
                Some(message) => Err(Error::Validation {
                    message: message.clone(),
                    fields: Vec::new(),
                }),
                None => Ok(()),
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
