use std::fs;

use super::*;

#[test]
fn a_regular_file_opens_with_its_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("info.json");
    fs::write(&path, b"{}").unwrap();

    let (_, metadata) = open_regular(&path).unwrap();

    assert_eq!(metadata.len(), 2);
}

#[test]
fn a_symbolic_link_to_a_regular_file_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("target"), b"{}").unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(dir.path().join("target"), &link).unwrap();

    let error = open_regular(&link).unwrap_err();

    assert_eq!(
        (error.kind(), error.to_string()),
        (
            io::ErrorKind::InvalidInput,
            "the path is a symbolic link".to_owned()
        )
    );
}

#[test]
fn a_fifo_fails_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("fifo");
    make_fifo(&fifo);

    let error = open_regular(&fifo).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn a_directory_is_not_a_regular_file() {
    let dir = tempfile::tempdir().unwrap();

    let error = open_regular(dir.path()).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn a_missing_file_fails_with_not_found() {
    let dir = tempfile::tempdir().unwrap();

    let error = open_regular(&dir.path().join("missing")).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

#[test]
fn a_file_within_the_limit_is_read_whole() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("episodes.jsonl");
    fs::write(&path, b"0123456789").unwrap();

    assert_eq!(read_capped(&path, 10).unwrap(), b"0123456789");
}

#[test]
fn a_file_above_the_limit_fails_with_file_too_large() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("episodes.jsonl");
    fs::write(&path, b"0123456789").unwrap();

    let error = read_capped(&path, 9).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
}
