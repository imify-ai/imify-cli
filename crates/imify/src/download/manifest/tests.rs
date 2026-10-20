//! Manifest requests and the checks of manifest entries, through the public interface.

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::download::DownloadSource;
use crate::download::test_support::*;
use crate::{DownloadError, Error};

/// Downloads with `manifest` into a directory that does not exist yet. Returns the error
/// and whether the directory was created.
async fn refused_with(server: &MockServer, body: &Value) -> (Error, bool) {
    mount_manifest(server, body).await;
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("policy");

    let error = downloader(server)
        .download(&source(), &target)
        .await
        .unwrap_err();

    (error, target.exists())
}

/// A manifest of `paths`, each with 4 bytes from storage on `server`.
fn manifest_of_paths(server: &MockServer, paths: &[&str]) -> Value {
    let entries: Vec<Value> = paths
        .iter()
        .map(|name| entry(name, &storage_url(&server.uri(), "file.bin"), 4))
        .collect();
    manifest(&entries)
}

#[tokio::test]
async fn unsafe_relative_paths_are_refused_before_anything_is_written() {
    for unsafe_path in [
        "../x",
        "a/../../x",
        "/etc/x",
        "",
        "a//b",
        "./a",
        "a\\b",
        "a\0b",
    ] {
        let server = MockServer::start().await;

        let (error, created) =
            refused_with(&server, &manifest_of_paths(&server, &[unsafe_path])).await;

        assert!(
            matches!(download_reason(&error), DownloadError::UnsafePath(_)),
            "{unsafe_path:?}: {error:?}"
        );
        assert!(!created, "{unsafe_path:?}");
        assert!(storage_requests(&server).await.is_empty());
    }
}

#[tokio::test]
async fn colliding_entries_are_refused() {
    let collisions: [&[&str]; 4] = [
        &["model.bin", "model.bin"],
        &["Model.bin", "model.bin"],
        &["weights", "weights/model.bin"],
        &["model.bin", "model.bin.part"],
    ];
    for paths in collisions {
        let server = MockServer::start().await;

        let (error, created) = refused_with(&server, &manifest_of_paths(&server, paths)).await;

        assert!(
            matches!(download_reason(&error), DownloadError::UnsafePath(_)),
            "{paths:?}: {error:?}"
        );
        assert!(!created);
    }
}

#[tokio::test]
async fn plain_http_storage_on_another_host_is_refused_without_naming_the_query() {
    let server = MockServer::start().await;
    let url = format!("http://storage.example.com/model.bin?{SIGNATURE}");

    let (error, created) = refused_with(&server, &manifest(&[entry("model.bin", &url, 4)])).await;

    let DownloadError::UnsafeUrl(message) = download_reason(&error) else {
        panic!("expected an unsafe URL, got {error:?}");
    };
    assert!(message.contains("http://storage.example.com"), "{message}");
    assert!(!error.to_string().contains(SIGNATURE));
    assert!(!format!("{error:?}").contains(SIGNATURE));
    assert!(!created);
}

#[tokio::test]
async fn a_total_size_other_than_the_sum_of_the_sizes_is_an_unexpected_response() {
    let server = MockServer::start().await;
    let url = storage_url(&server.uri(), "model.bin");
    let body = json!({"files": [entry("model.bin", &url, 4)], "total_size_bytes": 5});

    let (error, created) = refused_with(&server, &body).await;

    assert!(
        matches!(error, Error::UnexpectedResponse { .. }),
        "{error:?}"
    );
    assert!(!created);
}

#[tokio::test]
async fn an_unknown_checkpoint_is_not_found_and_creates_no_directory() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MANIFEST))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"detail": "Checkpoint not found"})),
        )
        .mount(&server)
        .await;
    let root = tempfile::tempdir().unwrap();

    let error = downloader(&server)
        .download(&source(), root.path().join("policy"))
        .await
        .unwrap_err();

    assert!(matches!(error, Error::NotFound { .. }), "{error:?}");
    assert!(!root.path().join("policy").exists());
}

#[tokio::test]
async fn storage_without_downloads_is_a_validation_error_with_the_detail() {
    let server = MockServer::start().await;
    let detail = "Downloads are not available for this storage backend";
    Mock::given(method("GET"))
        .and(path(MANIFEST))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({"detail": detail})))
        .mount(&server)
        .await;
    let root = tempfile::tempdir().unwrap();

    let error = downloader(&server)
        .download(&source(), root.path().join("policy"))
        .await
        .unwrap_err();

    assert!(
        matches!(&error, Error::Validation { message, .. } if message == detail),
        "{error:?}"
    );
    assert!(!root.path().join("policy").exists());
}

#[tokio::test]
async fn ids_with_a_slash_are_sent_as_single_path_segments() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/training-runs/team%2F8d41c7/checkpoints/step%2F1",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(manifest(&[])))
        .mount(&server)
        .await;
    let target = tempfile::tempdir().unwrap();

    let summary = downloader(&server)
        .download(
            &DownloadSource::checkpoint("team/8d41c7", "step/1"),
            target.path(),
        )
        .await
        .unwrap();

    assert_eq!(summary.files, 0);
}
