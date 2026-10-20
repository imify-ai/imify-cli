//! The download flow against a mock API and mock storage, through the public interface.

use std::fs;
use std::time::Duration;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::test_support::*;
use super::{DownloadEvent, DownloadSummary, Downloader};
use crate::{DownloadError, Error};

const WEIGHTS: &[u8] = &[7_u8; 4096];

/// Every message of `error` and its sources, and its debug output.
fn error_text(error: &Error) -> String {
    let mut text = format!("{error:?}");
    let mut source: Option<&dyn std::error::Error> = Some(error);
    while let Some(current) = source {
        text.push_str(&current.to_string());
        source = current.source();
    }
    text
}

fn assert_no_signature(error: &Error, events: &Events) {
    let secret = SIGNATURE.split_once('=').unwrap().1;
    assert!(!error_text(error).contains(secret), "{error:?}");
    assert!(!format!("{:?}", events.all()).contains(secret));
}

/// Mounts the manifest with `relative_path` served from `/storage/<url_name>`.
async fn mount_renamed(server: &MockServer, relative_path: &str, url_name: &str, size: u64) {
    let url = storage_url(&server.uri(), url_name);
    mount_manifest(server, &manifest(&[entry(relative_path, &url, size)])).await;
}

#[tokio::test]
async fn every_file_lands_at_its_relative_path_with_the_manifest_bytes() {
    let server = MockServer::start().await;
    let files: &[(&str, &[u8])] = &[
        ("config.json", b"{\"type\": \"act\"}"),
        ("weights/model.safetensors", WEIGHTS),
        ("weights/optimizer/state.bin", b"adam"),
    ];
    mount_files(&server, files).await;
    let target = tempfile::tempdir().unwrap();

    downloader(&server)
        .download(&source(), target.path())
        .await
        .unwrap();

    assert_eq!(
        files_below(target.path()),
        [
            "config.json",
            "weights/model.safetensors",
            "weights/optimizer/state.bin"
        ]
    );
    for (name, bytes) in files {
        assert_eq!(fs::read(target.path().join(name)).unwrap(), *bytes);
    }
}

#[tokio::test]
async fn the_summary_counts_the_files_and_bytes_of_the_manifest() {
    let server = MockServer::start().await;
    mount_files(&server, &[("config.json", b"{}"), ("model.bin", WEIGHTS)]).await;
    let target = tempfile::tempdir().unwrap();

    let summary = downloader(&server)
        .download(&source(), target.path())
        .await
        .unwrap();

    assert_eq!(
        summary,
        DownloadSummary {
            target: fs::canonicalize(target.path()).unwrap(),
            files: 2,
            bytes: 2 + 4096,
            files_skipped: 0,
            bytes_skipped: 0,
        }
    );
}

#[tokio::test]
async fn a_missing_target_directory_is_created_with_its_parents() {
    let server = MockServer::start().await;
    mount_files(&server, &[("model.bin", WEIGHTS)]).await;
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("checkpoints/step-20000");

    downloader(&server)
        .download(&source(), &target)
        .await
        .unwrap();

    assert_eq!(fs::read(target.join("model.bin")).unwrap(), WEIGHTS);
}

#[tokio::test]
async fn the_manifest_request_carries_the_token_and_storage_requests_do_not() {
    let server = MockServer::start().await;
    mount_files(&server, &[("model.bin", WEIGHTS)]).await;
    let target = tempfile::tempdir().unwrap();

    downloader(&server)
        .download(&source(), target.path())
        .await
        .unwrap();

    let manifest_requests = requests_to(&server, MANIFEST).await;
    assert_eq!(
        manifest_requests[0].headers["authorization"],
        format!("Bearer {TOKEN}").as_str()
    );
    let storage = storage_requests(&server).await;
    assert_eq!(storage.len(), 1);
    assert!(!storage[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn at_most_the_configured_number_of_files_is_in_flight() {
    let server = MockServer::start().await;
    let names = ["a.bin", "b.bin", "c.bin", "d.bin", "e.bin"];
    let files: Vec<(&str, &[u8])> = names.iter().map(|name| (*name, WEIGHTS)).collect();
    mount_manifest(&server, &manifest_of(&server, &files)).await;
    for name in names {
        let delayed = ResponseTemplate::new(200)
            .set_body_bytes(WEIGHTS)
            .set_delay(Duration::from_millis(50));
        mount_storage(&server, name, delayed).await;
    }
    let target = tempfile::tempdir().unwrap();
    let events = Events::default();

    events
        .attach(downloader(&server).options(concurrency(2)))
        .download(&source(), target.path())
        .await
        .unwrap();

    let mut in_flight = 0_i32;
    let mut most = 0;
    for event in events.all() {
        match event {
            DownloadEvent::FileStarted { .. } => in_flight += 1,
            DownloadEvent::FileFinished { .. } => in_flight -= 1,
            _ => {}
        }
        most = most.max(in_flight);
    }
    assert_eq!(most, 2);
}

#[tokio::test]
async fn events_of_a_file_arrive_as_started_progress_finished() {
    let server = MockServer::start().await;
    mount_files(&server, &[("model.bin", WEIGHTS)]).await;
    let target = tempfile::tempdir().unwrap();
    let events = Events::default();

    events
        .attach(downloader(&server))
        .download(&source(), target.path())
        .await
        .unwrap();

    let events = events.all();
    let path = "model.bin".to_owned();
    assert_eq!(
        events.first(),
        Some(&DownloadEvent::FileStarted {
            path: path.clone(),
            size: 4096
        })
    );
    assert_eq!(
        events.last(),
        Some(&DownloadEvent::FileFinished { path, size: 4096 })
    );
    let progress: Vec<u64> = events[1..events.len() - 1]
        .iter()
        .map(|event| match event {
            DownloadEvent::FileProgress { bytes_received, .. } => *bytes_received,
            other => panic!("expected progress, got {other:?}"),
        })
        .collect();
    assert!(progress.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(progress.last(), Some(&4096));
}

#[tokio::test]
async fn an_expired_url_fetches_the_manifest_again_and_completes_the_file() {
    let server = MockServer::start().await;
    let uri = server.uri();
    let first = manifest(&[
        entry("a.bin", &storage_url(&uri, "old/a.bin"), 4),
        entry("b.bin", &storage_url(&uri, "old/b.bin"), 4096),
    ]);
    let second = manifest(&[
        entry("a.bin", &storage_url(&uri, "new/a.bin"), 4),
        entry("b.bin", &storage_url(&uri, "new/b.bin"), 4096),
    ]);
    Mock::given(method("GET"))
        .and(path(MANIFEST))
        .respond_with(ResponseTemplate::new(200).set_body_json(first))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_manifest(&server, &second).await;
    mount_file(&server, "old/a.bin", b"head").await;
    let expired = ResponseTemplate::new(403)
        .set_body_string(
            "<Error><Code>AccessDenied</Code><Message>Request has expired</Message></Error>",
        )
        .insert_header("content-type", "application/xml");
    mount_storage(&server, "old/b.bin", expired).await;
    mount_file(&server, "new/b.bin", WEIGHTS).await;
    let target = tempfile::tempdir().unwrap();
    let events = Events::default();

    events
        .attach(downloader(&server).options(concurrency(1)))
        .download(&source(), target.path())
        .await
        .unwrap();

    assert_eq!(fs::read(target.path().join("b.bin")).unwrap(), WEIGHTS);
    assert_eq!(requests_to(&server, MANIFEST).await.len(), 2);
    assert_eq!(requests_to(&server, "/storage/old/a.bin").await.len(), 1);
    assert!(requests_to(&server, "/storage/new/a.bin").await.is_empty());
    assert!(events.all().contains(&DownloadEvent::ManifestRefreshed));
}

#[tokio::test]
async fn files_rejected_at_the_same_time_share_one_manifest_refresh() {
    let server = MockServer::start().await;
    let uri = server.uri();
    let first = manifest(&[
        entry("a.bin", &storage_url(&uri, "old/a.bin"), 4096),
        entry("b.bin", &storage_url(&uri, "old/b.bin"), 4096),
    ]);
    let second = manifest(&[
        entry("a.bin", &storage_url(&uri, "new/a.bin"), 4096),
        entry("b.bin", &storage_url(&uri, "new/b.bin"), 4096),
    ]);
    Mock::given(method("GET"))
        .and(path(MANIFEST))
        .respond_with(ResponseTemplate::new(200).set_body_json(first))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_manifest(&server, &second).await;
    for name in ["a.bin", "b.bin"] {
        let expired = ResponseTemplate::new(403).set_delay(Duration::from_millis(20));
        mount_storage(&server, &format!("old/{name}"), expired).await;
        mount_file(&server, &format!("new/{name}"), WEIGHTS).await;
    }
    let target = tempfile::tempdir().unwrap();

    downloader(&server)
        .download(&source(), target.path())
        .await
        .unwrap();

    assert_eq!(requests_to(&server, MANIFEST).await.len(), 2);
    assert_eq!(files_below(target.path()), ["a.bin", "b.bin"]);
}

#[tokio::test]
async fn a_second_rejection_of_the_same_file_fails_with_url_expired() {
    let server = MockServer::start().await;
    mount_manifest(&server, &manifest_of(&server, &[("model.bin", WEIGHTS)])).await;
    mount_storage(&server, "model.bin", ResponseTemplate::new(403)).await;
    let target = tempfile::tempdir().unwrap();

    let error = downloader(&server)
        .download(&source(), target.path())
        .await
        .unwrap_err();

    assert!(matches!(download_reason(&error), DownloadError::UrlExpired));
    assert_eq!(requests_to(&server, MANIFEST).await.len(), 2);
    assert!(!target.path().join("model.bin").exists());
}

#[tokio::test]
async fn a_response_refused_before_its_body_leaves_no_part_file() {
    let refusals = [
        ResponseTemplate::new(403),
        ResponseTemplate::new(404),
        ResponseTemplate::new(200).set_body_bytes(vec![0_u8; 8192]),
    ];
    for response in refusals {
        let server = MockServer::start().await;
        mount_manifest(&server, &manifest_of(&server, &[("model.bin", WEIGHTS)])).await;
        mount_storage(&server, "model.bin", response).await;
        let target = tempfile::tempdir().unwrap();

        let error = downloader(&server)
            .download(&source(), target.path())
            .await
            .unwrap_err();

        assert!(
            files_below(target.path()).is_empty(),
            "{error:?}: {:?}",
            files_below(target.path())
        );
    }
}

#[tokio::test]
async fn a_refreshed_manifest_with_another_size_fails_with_manifest_changed() {
    let server = MockServer::start().await;
    let uri = server.uri();
    let first = manifest(&[entry(
        "model.bin",
        &storage_url(&uri, "old/model.bin"),
        4096,
    )]);
    let second = manifest(&[entry(
        "model.bin",
        &storage_url(&uri, "new/model.bin"),
        8192,
    )]);
    Mock::given(method("GET"))
        .and(path(MANIFEST))
        .respond_with(ResponseTemplate::new(200).set_body_json(first))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_manifest(&server, &second).await;
    mount_storage(&server, "old/model.bin", ResponseTemplate::new(403)).await;
    let target = tempfile::tempdir().unwrap();

    let error = downloader(&server)
        .download(&source(), target.path())
        .await
        .unwrap_err();

    assert!(matches!(
        download_reason(&error),
        DownloadError::ManifestChanged(_)
    ));
    assert!(
        requests_to(&server, "/storage/new/model.bin")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn a_larger_content_length_fails_with_size_mismatch_without_a_retry() {
    let server = MockServer::start().await;
    mount_renamed(&server, "model.bin", "model.bin", 2048).await;
    mount_file(&server, "model.bin", WEIGHTS).await;
    let target = tempfile::tempdir().unwrap();

    let error = downloader(&server)
        .download(&source(), target.path())
        .await
        .unwrap_err();

    assert!(matches!(
        download_reason(&error),
        DownloadError::SizeMismatch {
            expected: 2048,
            received: 4096
        }
    ));
    assert_eq!(storage_requests(&server).await.len(), 1);
    assert!(!target.path().join("model.bin").exists());
}

#[tokio::test]
async fn a_body_without_content_length_stops_at_the_manifest_size() {
    let server = MockServer::start().await;
    let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n8\r\n01234567\r\n0\r\n\r\n";
    let storage = raw_storage(vec![chunked.to_vec()]).await;
    let url = format!("{storage}/model.bin?{SIGNATURE}");
    mount_manifest(&server, &manifest(&[entry("model.bin", &url, 4)])).await;
    let target = tempfile::tempdir().unwrap();

    let error = downloader(&server)
        .download(&source(), target.path())
        .await
        .unwrap_err();

    assert!(matches!(
        download_reason(&error),
        DownloadError::SizeMismatch {
            expected: 4,
            received: 8
        }
    ));
    assert!(!target.path().join("model.bin").exists());
}

#[tokio::test]
async fn a_truncated_transfer_is_fetched_again_from_the_first_byte() {
    let server = MockServer::start().await;
    let storage = raw_storage(vec![
        raw_response(WEIGHTS, WEIGHTS.len() / 2),
        raw_response(WEIGHTS, WEIGHTS.len()),
    ])
    .await;
    let url = format!("{storage}/model.bin?{SIGNATURE}");
    mount_manifest(&server, &manifest(&[entry("model.bin", &url, 4096)])).await;
    let target = tempfile::tempdir().unwrap();
    let events = Events::default();

    events
        .attach(downloader(&server))
        .download(&source(), target.path())
        .await
        .unwrap();

    assert_eq!(fs::read(target.path().join("model.bin")).unwrap(), WEIGHTS);
    assert!(
        events
            .all()
            .iter()
            .any(|event| matches!(event, DownloadEvent::FileRetrying { attempt: 1, .. }))
    );
}

#[tokio::test]
async fn a_transfer_truncated_on_every_attempt_fails_with_a_network_error() {
    let server = MockServer::start().await;
    let truncated = raw_response(WEIGHTS, WEIGHTS.len() / 2);
    let storage = raw_storage(vec![truncated; 3]).await;
    let url = format!("{storage}/model.bin?{SIGNATURE}");
    mount_manifest(&server, &manifest(&[entry("model.bin", &url, 4096)])).await;
    let target = tempfile::tempdir().unwrap();

    let error = Downloader::new(client_with(&server, 2))
        .download(&source(), target.path())
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Network { .. }), "{error:?}");
    assert!(!target.path().join("model.bin").exists());
    // The connection close can arrive as a reset before the body bytes, so the part file
    // holds between none and all of the 2048 bytes sent.
    let part = fs::read(target.path().join("model.bin.part")).unwrap();
    assert!(WEIGHTS[..2048].starts_with(&part), "{} bytes", part.len());
}

#[tokio::test]
async fn a_storage_server_error_is_retried() {
    let server = MockServer::start().await;
    mount_manifest(&server, &manifest_of(&server, &[("model.bin", WEIGHTS)])).await;
    Mock::given(method("GET"))
        .and(path("/storage/model.bin"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_file(&server, "model.bin", WEIGHTS).await;
    let target = tempfile::tempdir().unwrap();

    downloader(&server)
        .download(&source(), target.path())
        .await
        .unwrap();

    assert_eq!(storage_requests(&server).await.len(), 2);
    assert_eq!(fs::read(target.path().join("model.bin")).unwrap(), WEIGHTS);
}

#[tokio::test]
async fn a_storage_rate_limit_pauses_and_is_retried() {
    let server = MockServer::start().await;
    mount_manifest(&server, &manifest_of(&server, &[("model.bin", WEIGHTS)])).await;
    Mock::given(method("GET"))
        .and(path("/storage/model.bin"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_file(&server, "model.bin", WEIGHTS).await;
    let target = tempfile::tempdir().unwrap();
    let events = Events::default();

    events
        .attach(downloader(&server))
        .download(&source(), target.path())
        .await
        .unwrap();

    assert!(
        events
            .all()
            .iter()
            .any(|event| matches!(event, DownloadEvent::RateLimited { .. }))
    );
    assert_eq!(fs::read(target.path().join("model.bin")).unwrap(), WEIGHTS);
}

#[tokio::test]
async fn a_storage_not_found_fails_without_a_retry() {
    let server = MockServer::start().await;
    mount_manifest(&server, &manifest_of(&server, &[("model.bin", WEIGHTS)])).await;
    mount_storage(&server, "model.bin", ResponseTemplate::new(404)).await;
    let target = tempfile::tempdir().unwrap();
    let events = Events::default();

    let error = events
        .attach(downloader(&server))
        .download(&source(), target.path())
        .await
        .unwrap_err();

    assert!(matches!(
        download_reason(&error),
        DownloadError::Storage { status: 404 }
    ));
    assert_eq!(storage_requests(&server).await.len(), 1);
    assert_no_signature(&error, &events);
}

#[tokio::test]
async fn a_storage_redirect_to_another_origin_is_not_followed() {
    let server = MockServer::start().await;
    let elsewhere = MockServer::start().await;
    mount_file(&elsewhere, "model.bin", WEIGHTS).await;
    mount_manifest(&server, &manifest_of(&server, &[("model.bin", WEIGHTS)])).await;
    let redirect = ResponseTemplate::new(307)
        .insert_header("location", storage_url(&elsewhere.uri(), "model.bin"));
    mount_storage(&server, "model.bin", redirect).await;
    let target = tempfile::tempdir().unwrap();

    let error = downloader(&server)
        .download(&source(), target.path())
        .await
        .unwrap_err();

    assert!(matches!(
        download_reason(&error),
        DownloadError::Storage { status: 307 }
    ));
    assert!(elsewhere.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_storage_redirect_within_the_origin_is_followed() {
    let server = MockServer::start().await;
    mount_manifest(&server, &manifest_of(&server, &[("model.bin", WEIGHTS)])).await;
    let redirect = ResponseTemplate::new(307)
        .insert_header("location", storage_url(&server.uri(), "moved/model.bin"));
    mount_storage(&server, "model.bin", redirect).await;
    mount_file(&server, "moved/model.bin", WEIGHTS).await;
    let target = tempfile::tempdir().unwrap();

    downloader(&server)
        .download(&source(), target.path())
        .await
        .unwrap();

    assert_eq!(fs::read(target.path().join("model.bin")).unwrap(), WEIGHTS);
}

#[tokio::test]
async fn a_network_error_names_no_presigned_query() {
    let server = MockServer::start().await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let url = storage_url(&base, "model.bin");
    mount_manifest(&server, &manifest(&[entry("model.bin", &url, 4096)])).await;
    let target = tempfile::tempdir().unwrap();
    let events = Events::default();

    let error = events
        .attach(downloader(&server))
        .download(&source(), target.path())
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Network { .. }), "{error:?}");
    assert_no_signature(&error, &events);
}
