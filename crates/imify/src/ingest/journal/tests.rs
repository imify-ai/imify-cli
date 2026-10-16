use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;

const HEADER: &str = r#"{"version":1,"api_url":"https://api.example.com/","root":"/data/pick-place","session":"3f2c9a","name":"pick-place","task_description":"Pick up the red cube"}"#;
const ENTRY: &str = r#"{"path":"meta/info.json","size":10,"modified_ns":1792137600123456789,"sha256":"84d89877f0d4041efb6bf91a16f0248f2fd573e6af05c19f96bedb9f882f7882"}"#;

fn api_url(raw: &str) -> Url {
    Url::parse(raw).unwrap()
}

async fn locate(dir: &Path, url: &str, root: &Path) -> PathBuf {
    JournalFile::locate(dir, &api_url(url), root)
        .await
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

/// The dataset root that [`HEADER`] names; [`load`] writes the located root in its place.
const HEADER_ROOT: &str = "/data/pick-place";

/// Writes `content` as the journal of a new dataset root and loads it.
async fn load(content: &str) -> Result<Option<Recorded>, Error> {
    let (_temp, journals, root) = directories();
    let journal = JournalFile::locate(&journals, &api_url("https://api.example.com"), &root)
        .await
        .unwrap();
    let content = content.replace(HEADER_ROOT, &journal.root.display().to_string());
    fs::write(journal.path(), content).unwrap();
    journal.load().await
}

fn is_corrupt(result: &Result<Option<Recorded>, Error>) -> bool {
    matches!(
        result,
        Err(Error::Journal {
            reason: JournalError::Corrupt(_),
            ..
        })
    )
}

#[tokio::test]
async fn journal_whose_header_names_another_api_fails_with_corrupt() {
    let content = format!(
        "{}\n",
        HEADER.replace("https://api.example.com/", "https://other.example.com/")
    );

    assert!(is_corrupt(&load(&content).await));
}

#[tokio::test]
async fn journal_whose_header_names_another_root_fails_with_corrupt() {
    let content = format!("{}\n", HEADER.replace(HEADER_ROOT, "/data/other"));
    let (_temp, journals, root) = directories();
    let journal = JournalFile::locate(&journals, &api_url("https://api.example.com"), &root)
        .await
        .unwrap();
    fs::write(journal.path(), content).unwrap();

    assert!(is_corrupt(&journal.load().await));
}

#[tokio::test]
async fn journal_larger_than_64_mib_fails_with_corrupt() {
    let (_temp, journals, root) = directories();
    let journal = JournalFile::locate(&journals, &api_url("https://api.example.com"), &root)
        .await
        .unwrap();
    fs::File::create(journal.path())
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();

    assert!(is_corrupt(&journal.load().await));
}

#[tokio::test]
async fn journal_that_is_a_symbolic_link_fails_with_corrupt() {
    let (temp, journals, root) = directories();
    let journal = JournalFile::locate(&journals, &api_url("https://api.example.com"), &root)
        .await
        .unwrap();
    let elsewhere = temp.path().join("elsewhere.jsonl");
    fs::write(&elsewhere, format!("{HEADER}\n")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, journal.path()).unwrap();

    assert!(is_corrupt(&journal.load().await));
}

#[tokio::test]
async fn creating_a_journal_does_not_write_through_a_link_at_the_temporary_path() {
    let (temp, journals, root) = directories();
    let journal = JournalFile::locate(&journals, &api_url("https://api.example.com"), &root)
        .await
        .unwrap();
    let victim = temp.path().join("victim");
    fs::write(&victim, b"unchanged").unwrap();
    std::os::unix::fs::symlink(&victim, journal.path().with_extension("jsonl.tmp")).unwrap();

    journal
        .create(&SessionId::new("3f2c9a"), &NewSession::new("pick-place"))
        .await
        .unwrap();

    assert_eq!(
        (
            fs::read(&victim).unwrap(),
            journal.load().await.unwrap().is_some()
        ),
        (b"unchanged".to_vec(), true)
    );
}

#[tokio::test]
async fn creating_a_journal_replaces_a_temporary_file_left_by_a_crash() {
    let (_temp, journals, root) = directories();
    let journal = JournalFile::locate(&journals, &api_url("https://api.example.com"), &root)
        .await
        .unwrap();
    fs::write(journal.path().with_extension("jsonl.tmp"), b"partial").unwrap();

    journal
        .create(&SessionId::new("3f2c9a"), &NewSession::new("pick-place"))
        .await
        .unwrap();

    assert_eq!(
        journal.load().await.unwrap().unwrap().session().as_str(),
        "3f2c9a"
    );
}

#[tokio::test]
async fn an_existing_journal_directory_writable_by_others_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    for mode in [0o777, 0o770, 0o707] {
        let (_temp, journals, root) = directories();
        fs::create_dir(&journals).unwrap();
        fs::set_permissions(&journals, fs::Permissions::from_mode(mode)).unwrap();

        let error = JournalFile::locate(&journals, &api_url("https://api.example.com"), &root)
            .await
            .unwrap_err();

        assert!(
            matches!(
                error,
                Error::Journal { reason: JournalError::Io(ref source), .. }
                    if source.kind() == io::ErrorKind::PermissionDenied
            ),
            "{mode:o}: {error:?}"
        );
    }
}

#[tokio::test]
async fn an_existing_journal_directory_readable_by_others_is_used() {
    use std::os::unix::fs::PermissionsExt;
    let (_temp, journals, root) = directories();
    fs::create_dir(&journals).unwrap();
    fs::set_permissions(&journals, fs::Permissions::from_mode(0o755)).unwrap();

    let located = JournalFile::locate(&journals, &api_url("https://api.example.com"), &root).await;

    assert!(located.is_ok());
}

#[tokio::test]
async fn path_is_the_same_for_a_root_reached_through_a_symlink() {
    let (temp, journals, root) = directories();
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&root, &link).unwrap();

    assert_eq!(
        locate(&journals, "https://api.example.com", &link).await,
        locate(&journals, "https://api.example.com", &root).await
    );
}

#[tokio::test]
async fn path_differs_for_another_base_url() {
    let (_temp, journals, root) = directories();

    assert_ne!(
        locate(&journals, "https://api.example.com", &root).await,
        locate(&journals, "https://staging.example.com", &root).await
    );
}

#[tokio::test]
async fn path_differs_for_another_root() {
    let (temp, journals, root) = directories();
    let other = temp.path().join("stack-cups");
    fs::create_dir(&other).unwrap();

    assert_ne!(
        locate(&journals, "https://api.example.com", &root).await,
        locate(&journals, "https://api.example.com", &other).await
    );
}

#[tokio::test]
async fn path_is_a_jsonl_file_in_the_journal_directory() {
    let (_temp, journals, root) = directories();

    let path = locate(&journals, "https://api.example.com", &root).await;

    assert_eq!(
        (path.parent(), path.extension().and_then(|e| e.to_str())),
        (Some(journals.as_path()), Some("jsonl"))
    );
}

#[tokio::test]
async fn locating_creates_the_journal_directory_readable_by_the_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let (_temp, journals, root) = directories();

    locate(&journals, "https://api.example.com", &root).await;

    assert_eq!(
        fs::metadata(&journals).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[tokio::test]
async fn missing_journal_loads_as_none() {
    let (_temp, journals, root) = directories();
    let journal = JournalFile::locate(&journals, &api_url("https://api.example.com"), &root)
        .await
        .unwrap();

    assert!(journal.load().await.unwrap().is_none());
}

#[tokio::test]
async fn journal_loads_the_session_and_the_entries() {
    let recorded = load(&format!("{HEADER}\n{ENTRY}\n"))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        (recorded.session().as_str(), recorded.entries.len()),
        ("3f2c9a", 1)
    );
}

#[tokio::test]
async fn journal_with_a_partial_last_line_loads_without_that_line() {
    let recorded = load(&format!("{HEADER}\n{ENTRY}\n{{\"path\":\"data/ch"))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(recorded.entries.len(), 1);
}

#[tokio::test]
async fn journal_with_a_malformed_line_before_the_last_fails_with_corrupt() {
    let error = load(&format!("{HEADER}\nnot json\n{ENTRY}\n"))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        Error::Journal {
            reason: JournalError::Corrupt(_),
            ..
        }
    ));
}

#[tokio::test]
async fn journal_with_an_unknown_version_fails_with_corrupt() {
    let error = load(&format!(
        "{}\n",
        HEADER.replace("\"version\":1", "\"version\":2")
    ))
    .await
    .unwrap_err();

    assert!(matches!(
        error,
        Error::Journal {
            reason: JournalError::Corrupt(_),
            ..
        }
    ));
}

#[tokio::test]
async fn journal_without_a_complete_header_fails_with_corrupt() {
    let error = load(&HEADER[..20]).await.unwrap_err();

    assert!(matches!(
        error,
        Error::Journal {
            reason: JournalError::Corrupt(_),
            ..
        }
    ));
}
