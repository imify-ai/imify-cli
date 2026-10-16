use std::fs;
use std::io::Write as _;
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
        .iter()
        .map(|entry| entry.path().to_owned())
        .collect()
}

/// Opens the second planned file, the first after `meta/info.json`.
fn open_data_file(root: &TempDir, plan: &UploadPlan) -> Result<File, Error> {
    let file = &plan.files()[1];
    file.open(&root.path().join(file.path()))
}

fn skipped_with_reasons(root: &TempDir) -> Vec<(String, SkipReason)> {
    UploadPlan::from_dir(root.path())
        .unwrap()
        .skipped()
        .iter()
        .map(|entry| (entry.path().to_owned(), entry.reason()))
        .collect()
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
fn symlinked_file_outside_the_root_is_skipped_as_a_symbolic_link() {
    let root = dataset(&[]);
    let target = tempfile::tempdir().unwrap();
    write(target.path(), "blob", 42);
    fs::create_dir_all(root.path().join("data")).unwrap();
    std::os::unix::fs::symlink(target.path().join("blob"), root.path().join("data/a")).unwrap();

    assert_eq!(
        (planned(&root).len(), skipped_with_reasons(&root)),
        (1, vec![("data/a".to_owned(), SkipReason::SymbolicLink)])
    );
}

#[test]
fn symlinked_file_inside_the_root_is_skipped_as_a_symbolic_link() {
    let root = dataset(&[("data/a", 3)]);
    std::os::unix::fs::symlink(root.path().join("data/a"), root.path().join("data/b")).unwrap();

    assert_eq!(
        skipped_with_reasons(&root),
        [("data/b".to_owned(), SkipReason::SymbolicLink)]
    );
}

#[test]
fn symlinked_directory_is_skipped_as_a_symbolic_link() {
    let root = dataset(&[]);
    let target = tempfile::tempdir().unwrap();
    write(target.path(), "blob", 1);
    std::os::unix::fs::symlink(target.path(), root.path().join("data")).unwrap();

    assert_eq!(
        skipped_with_reasons(&root),
        [("data".to_owned(), SkipReason::SymbolicLink)]
    );
}

#[test]
fn symlinked_info_file_returns_io() {
    let root = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    write(target.path(), "info.json", 2);
    fs::create_dir_all(root.path().join("meta")).unwrap();
    std::os::unix::fs::symlink(target.path().join("info.json"), root.path().join(INFO_FILE))
        .unwrap();

    let error = UploadPlan::from_dir(root.path()).unwrap_err();

    assert!(matches!(error, Error::Io { ref path, .. } if *path == root.path().join(INFO_FILE)));
}

#[test]
fn fifo_is_skipped_as_not_a_regular_file() {
    let root = dataset(&[]);
    fs::create_dir_all(root.path().join("data")).unwrap();
    crate::local_file::make_fifo(&root.path().join("data/pipe"));

    assert_eq!(
        skipped_with_reasons(&root),
        [("data/pipe".to_owned(), SkipReason::NotRegularFile)]
    );
}

#[test]
fn each_skipped_entry_carries_its_reason() {
    let root = dataset(&[("README.md", 1), ("data/.cache/x", 1), ("outputs/a", 1)]);

    assert_eq!(
        skipped_with_reasons(&root),
        [
            ("README.md".to_owned(), SkipReason::OutsideDataset),
            ("data/.cache/".to_owned(), SkipReason::Hidden),
            ("outputs/".to_owned(), SkipReason::OutsideDataset),
        ]
    );
}

#[test]
fn skip_reasons_have_stable_names() {
    let names: Vec<&str> = [
        SkipReason::OutsideDataset,
        SkipReason::Hidden,
        SkipReason::SymbolicLink,
        SkipReason::NotRegularFile,
        SkipReason::InvalidName,
    ]
    .into_iter()
    .map(SkipReason::as_str)
    .collect();

    assert_eq!(
        names,
        [
            "outside_dataset",
            "hidden",
            "symbolic_link",
            "not_regular_file",
            "invalid_name"
        ]
    );
}

#[test]
fn opening_a_planned_file_returns_it_while_unchanged() {
    let root = dataset(&[("data/a", 5)]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();

    assert!(open_data_file(&root, &plan).is_ok());
}

#[test]
fn opening_a_planned_file_replaced_by_a_symbolic_link_fails() {
    let root = dataset(&[("data/a", 5)]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    let target = tempfile::tempdir().unwrap();
    write(target.path(), "blob", 5);
    fs::remove_file(root.path().join("data/a")).unwrap();
    std::os::unix::fs::symlink(target.path().join("blob"), root.path().join("data/a")).unwrap();

    let error = open_data_file(&root, &plan).unwrap_err();

    assert!(matches!(error, Error::Io { .. }), "{error:?}");
}

#[test]
fn opening_a_planned_file_replaced_by_another_file_of_the_same_size_fails() {
    let root = dataset(&[("data/a", 5)]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    // The new file exists before the old one is removed, so it cannot reuse the inode.
    write(root.path(), "data/b", 5);
    fs::rename(root.path().join("data/b"), root.path().join("data/a")).unwrap();

    let error = open_data_file(&root, &plan).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!(
            "cannot read {}: the file was replaced since planning",
            root.path().join("data/a").display()
        )
    );
}

#[test]
fn opening_a_planned_file_whose_size_changed_fails() {
    let root = dataset(&[("data/a", 5)]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(root.path().join("data/a"))
        .unwrap()
        .write_all(b"more")
        .unwrap();

    let error = open_data_file(&root, &plan).unwrap_err();

    assert!(
        error
            .to_string()
            .ends_with("file size changed since planning, expected 5 bytes"),
        "{error}"
    );
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
