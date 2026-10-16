use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;

const HEADER: &str = r#"{"version":1,"api_url":"https://api.example.com/","root":"/data/pick-place","session":"3f2c9a","name":"pick-place","task_description":"Pick up the red cube"}"#;
const ENTRY: &str = r#"{"path":"meta/info.json","size":10,"modified_ns":1792137600123456789,"sha256":"84d89877f0d4041efb6bf91a16f0248f2fd573e6af05c19f96bedb9f882f7882"}"#;

fn api_url(raw: &str) -> Url {
    Url::parse(raw).unwrap()
}

fn locate(dir: &Path, url: &str, root: &Path) -> PathBuf {
    JournalFile::locate(dir, &api_url(url), root)
        .unwrap()
        .path()
        .to_owned()
}

/// A journal directory and a dataset root next to it.
fn directories() -> (TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let journals = temp.path().join("journals");
    let root = temp.path().join("pick-place");
    fs::create_dir(&root).unwrap();
    (temp, journals, root)
}

fn load(content: &str) -> Result<Option<Recorded>, Error> {
    let (_temp, journals, root) = directories();
    let journal =
        JournalFile::locate(&journals, &api_url("https://api.example.com"), &root).unwrap();
    fs::write(journal.path(), content).unwrap();
    journal.load()
}

#[test]
fn path_is_the_same_for_a_root_reached_through_a_symlink() {
    let (temp, journals, root) = directories();
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&root, &link).unwrap();

    assert_eq!(
        locate(&journals, "https://api.example.com", &link),
        locate(&journals, "https://api.example.com", &root)
    );
}

#[test]
fn path_differs_for_another_base_url() {
    let (_temp, journals, root) = directories();

    assert_ne!(
        locate(&journals, "https://api.example.com", &root),
        locate(&journals, "https://staging.example.com", &root)
    );
}

#[test]
fn path_differs_for_another_root() {
    let (temp, journals, root) = directories();
    let other = temp.path().join("stack-cups");
    fs::create_dir(&other).unwrap();

    assert_ne!(
        locate(&journals, "https://api.example.com", &root),
        locate(&journals, "https://api.example.com", &other)
    );
}

#[test]
fn path_is_a_jsonl_file_in_the_journal_directory() {
    let (_temp, journals, root) = directories();

    let path = locate(&journals, "https://api.example.com", &root);

    assert_eq!(
        (path.parent(), path.extension().and_then(|e| e.to_str())),
        (Some(journals.as_path()), Some("jsonl"))
    );
}

#[test]
fn locating_creates_the_journal_directory_readable_by_the_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let (_temp, journals, root) = directories();

    locate(&journals, "https://api.example.com", &root);

    assert_eq!(
        fs::metadata(&journals).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn missing_journal_loads_as_none() {
    let (_temp, journals, root) = directories();
    let journal =
        JournalFile::locate(&journals, &api_url("https://api.example.com"), &root).unwrap();

    assert!(journal.load().unwrap().is_none());
}

#[test]
fn journal_loads_the_session_and_the_entries() {
    let recorded = load(&format!("{HEADER}\n{ENTRY}\n")).unwrap().unwrap();

    assert_eq!(
        (recorded.session().as_str(), recorded.entries.len()),
        ("3f2c9a", 1)
    );
}

#[test]
fn journal_with_a_partial_last_line_loads_without_that_line() {
    let recorded = load(&format!("{HEADER}\n{ENTRY}\n{{\"path\":\"data/ch"))
        .unwrap()
        .unwrap();

    assert_eq!(recorded.entries.len(), 1);
}

#[test]
fn journal_with_a_malformed_line_before_the_last_fails_with_corrupt() {
    let error = load(&format!("{HEADER}\nnot json\n{ENTRY}\n")).unwrap_err();

    assert!(matches!(
        error,
        Error::Journal {
            reason: JournalError::Corrupt(_),
            ..
        }
    ));
}

#[test]
fn journal_with_an_unknown_version_fails_with_corrupt() {
    let error = load(&format!(
        "{}\n",
        HEADER.replace("\"version\":1", "\"version\":2")
    ))
    .unwrap_err();

    assert!(matches!(
        error,
        Error::Journal {
            reason: JournalError::Corrupt(_),
            ..
        }
    ));
}

#[test]
fn journal_without_a_complete_header_fails_with_corrupt() {
    let error = load(&HEADER[..20]).unwrap_err();

    assert!(matches!(
        error,
        Error::Journal {
            reason: JournalError::Corrupt(_),
            ..
        }
    ));
}
