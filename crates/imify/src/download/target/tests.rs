//! What a download does with the entries of its target directory, through the public
//! interface.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use wiremock::{MockServer, ResponseTemplate};

use crate::download::test_support::*;
use crate::download::{DownloadEvent, Downloader};
use crate::{DownloadError, Error};

const WEIGHTS: &[u8] = &[7_u8; 4096];

/// A mock serving `model.bin` and `weights/head.bin`.
async fn policy_server() -> MockServer {
    let server = MockServer::start().await;
    mount_files(
        &server,
        &[("model.bin", WEIGHTS), ("weights/head.bin", b"head")],
    )
    .await;
    server
}

async fn download_into(server: &MockServer, target: &Path) -> Result<(), Error> {
    downloader(server)
        .download(&source(), target)
        .await
        .map(|_| ())
}

/// Downloads with a future that owns its arguments, so it can run on a spawned task.
async fn download_owned(downloader: Downloader, target: PathBuf) -> Result<(), Error> {
    downloader.download(&source(), target).await.map(|_| ())
}

fn assert_foreign(error: &Error) {
    assert!(
        matches!(download_reason(error), DownloadError::Foreign(_)),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_rerun_fetches_only_the_files_that_are_not_complete() {
    let server = policy_server().await;
    let target = tempfile::tempdir().unwrap();
    write(target.path(), "model.bin", WEIGHTS);
    write(target.path(), "weights/head.bin.part", b"he");
    let events = Events::default();

    let summary = events
        .attach(downloader(&server))
        .download(&source(), target.path())
        .await
        .unwrap();

    let requested: Vec<String> = storage_requests(&server)
        .await
        .iter()
        .map(|request| request.url.path().to_owned())
        .collect();
    assert_eq!(requested, ["/storage/weights/head.bin"]);
    assert_eq!(
        events.all().first(),
        Some(&DownloadEvent::Resumed {
            files_done: 1,
            bytes_done: 4096
        })
    );
    assert_eq!((summary.files_skipped, summary.bytes_skipped), (1, 4096));
    assert_eq!(
        files_below(target.path()),
        ["model.bin", "weights/head.bin"]
    );
    assert_eq!(
        fs::read(target.path().join("weights/head.bin")).unwrap(),
        b"head"
    );
}

#[tokio::test]
async fn a_file_not_in_the_manifest_is_refused_before_any_transfer() {
    let server = policy_server().await;
    let target = tempfile::tempdir().unwrap();
    write(target.path(), "notes.txt", b"keep");

    let error = download_into(&server, target.path()).await.unwrap_err();

    assert_foreign(&error);
    assert!(storage_requests(&server).await.is_empty());
}

#[tokio::test]
async fn a_manifest_file_with_another_size_is_refused_and_kept() {
    let server = policy_server().await;
    let target = tempfile::tempdir().unwrap();
    write(target.path(), "model.bin", b"older weights");

    let error = download_into(&server, target.path()).await.unwrap_err();

    assert_foreign(&error);
    assert_eq!(
        fs::read(target.path().join("model.bin")).unwrap(),
        b"older weights"
    );
    assert!(storage_requests(&server).await.is_empty());
}

#[tokio::test]
async fn a_symbolic_link_below_the_target_is_refused_and_its_target_unchanged() {
    for link in ["model.bin", "model.bin.part", "weights"] {
        let server = policy_server().await;
        let target = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "victim", b"unchanged");
        symlink(outside.path().join("victim"), target.path().join(link)).unwrap();

        let error = download_into(&server, target.path()).await.unwrap_err();

        assert_foreign(&error);
        assert_eq!(
            fs::read(outside.path().join("victim")).unwrap(),
            b"unchanged",
            "{link}"
        );
        assert!(storage_requests(&server).await.is_empty(), "{link}");
    }
}

#[tokio::test]
async fn a_part_file_hard_linked_outside_the_target_leaves_the_outside_file_unchanged() {
    let server = policy_server().await;
    let target = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), "victim", b"unchanged");
    fs::hard_link(
        outside.path().join("victim"),
        target.path().join("model.bin.part"),
    )
    .unwrap();

    download_into(&server, target.path()).await.unwrap();

    assert_eq!(
        fs::read(outside.path().join("victim")).unwrap(),
        b"unchanged"
    );
    assert_eq!(fs::read(target.path().join("model.bin")).unwrap(), WEIGHTS);
}

#[tokio::test]
async fn a_symbolic_link_placed_after_the_scan_is_refused_and_not_followed() {
    let server = MockServer::start().await;
    mount_manifest(
        &server,
        &manifest_of(
            &server,
            &[("a.bin", WEIGHTS), ("weights/head.bin", b"head")],
        ),
    )
    .await;
    let slow = ResponseTemplate::new(200)
        .set_body_bytes(WEIGHTS)
        .set_delay(Duration::from_millis(300));
    mount_storage(&server, "a.bin", slow).await;
    mount_file(&server, "weights/head.bin", b"head").await;
    let target = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let download = tokio::spawn(download_owned(
        downloader(&server).options(concurrency(1)),
        target.path().to_owned(),
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    while requests_to(&server, "/storage/a.bin").await.is_empty() {
        assert!(Instant::now() < deadline, "a.bin was never requested");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    symlink(outside.path(), target.path().join("weights")).unwrap();
    let error = download.await.unwrap().unwrap_err();

    assert_foreign(&error);
    assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn a_target_that_is_a_symbolic_link_to_a_directory_receives_the_files() {
    let server = policy_server().await;
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("real")).unwrap();
    symlink(root.path().join("real"), root.path().join("link")).unwrap();

    download_into(&server, &root.path().join("link"))
        .await
        .unwrap();

    assert_eq!(
        fs::read(root.path().join("real/model.bin")).unwrap(),
        WEIGHTS
    );
}

#[tokio::test]
async fn a_target_that_is_a_regular_file_fails_with_an_io_error() {
    let server = policy_server().await;
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "policy", b"not a directory");

    let error = download_into(&server, &root.path().join("policy"))
        .await
        .unwrap_err();

    assert!(
        matches!(download_reason(&error), DownloadError::Io(_)),
        "{error:?}"
    );
}
