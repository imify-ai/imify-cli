use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;

fn write(root: &Path, path: &str, size: usize) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; size]).unwrap();
}

fn dataset(files: &[(&str, usize)]) -> TempDir {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), INFO_FILE, 10);
    for (path, size) in files {
        write(root.path(), path, *size);
    }
    root
}

fn planned(root: &TempDir) -> Vec<(String, u64)> {
    UploadPlan::from_dir(root.path())
        .unwrap()
        .files()
        .iter()
        .map(|file| (file.path().to_owned(), file.size()))
        .collect()
}

fn skipped(root: &TempDir) -> Vec<String> {
    UploadPlan::from_dir(root.path())
        .unwrap()
        .skipped()
        .to_vec()
}

#[test]
fn lists_files_under_meta_data_and_videos_with_their_sizes() {
    let root = dataset(&[
        ("meta/stats.json", 3),
        ("data/chunk-000/file-000.parquet", 7),
        ("videos/front/chunk-000/file-000.mp4", 11),
    ]);

    assert_eq!(
        planned(&root),
        [
            ("meta/info.json".to_owned(), 10),
            ("data/chunk-000/file-000.parquet".to_owned(), 7),
            ("meta/stats.json".to_owned(), 3),
            ("videos/front/chunk-000/file-000.mp4".to_owned(), 11),
        ]
    );
}

#[test]
fn info_file_comes_first_and_the_rest_is_sorted_by_path() {
    let root = dataset(&[("meta/episodes.jsonl", 1), ("data/b", 1), ("data/a", 1)]);

    let paths: Vec<String> = planned(&root).into_iter().map(|(path, _)| path).collect();

    assert_eq!(
        paths,
        ["meta/info.json", "data/a", "data/b", "meta/episodes.jsonl"]
    );
}

#[test]
fn files_outside_the_uploaded_directories_are_skipped() {
    let root = dataset(&[("README.md", 1), ("outputs/model.bin", 1)]);

    assert_eq!(
        (planned(&root).len(), skipped(&root)),
        (1, vec!["README.md".to_owned(), "outputs/".to_owned()])
    );
}

#[test]
fn paths_with_a_hidden_segment_are_skipped() {
    let root = dataset(&[("videos/.cache/x.mp4", 1), ("data/.DS_Store", 1)]);

    assert_eq!(
        (planned(&root).len(), skipped(&root)),
        (
            1,
            vec!["data/.DS_Store".to_owned(), "videos/.cache/".to_owned()]
        )
    );
}

#[test]
fn symlinked_file_is_listed_with_the_target_size() {
    let root = dataset(&[]);
    let target = tempfile::tempdir().unwrap();
    write(target.path(), "blob", 42);
    fs::create_dir_all(root.path().join("data")).unwrap();
    std::os::unix::fs::symlink(target.path().join("blob"), root.path().join("data/a")).unwrap();

    assert!(planned(&root).contains(&("data/a".to_owned(), 42)));
}

#[test]
fn symlinked_directory_is_skipped() {
    let root = dataset(&[]);
    let target = tempfile::tempdir().unwrap();
    write(target.path(), "blob", 1);
    std::os::unix::fs::symlink(target.path(), root.path().join("data")).unwrap();

    assert_eq!(skipped(&root), ["data/"]);
}

#[test]
fn nested_paths_use_slash_as_separator() {
    let root = dataset(&[("data/chunk-000/file-000.parquet", 1)]);

    assert_eq!(planned(&root)[1].0, "data/chunk-000/file-000.parquet");
}

#[test]
fn root_without_info_file_returns_io_naming_that_path() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "data/a", 1);

    let error = UploadPlan::from_dir(root.path()).unwrap_err();

    assert!(matches!(error, Error::Io { ref path, .. } if *path == root.path().join(INFO_FILE)));
}

#[test]
fn missing_root_returns_io() {
    let root = tempfile::tempdir().unwrap();

    let error = UploadPlan::from_dir(root.path().join("missing")).unwrap_err();

    assert!(matches!(error, Error::Io { .. }));
}

#[test]
fn retain_removes_files_and_total_bytes_follows() {
    let root = dataset(&[("data/a", 5), ("data/b", 7)]);
    let mut plan = UploadPlan::from_dir(root.path()).unwrap();

    plan.retain(|file| file.path() != "data/b");

    assert_eq!((plan.files().len(), plan.total_bytes()), (2, 15));
}
