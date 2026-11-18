use std::fs;

use super::*;
use crate::ingest::journal::hex;

fn dataset(files: &[(&str, &[u8])]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (path, content) in [("meta/info.json", &b"0123456789"[..])].iter().chain(files) {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    root
}

#[tokio::test]
async fn all_returns_the_sha256_of_every_file() {
    let root = dataset(&[("data/a", b"")]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();

    let digests = all(&plan, &HashMap::new()).await.unwrap();

    assert_eq!(
        (hex(&digests["meta/info.json"]), hex(&digests["data/a"])),
        (
            "84d89877f0d4041efb6bf91a16f0248f2fd573e6af05c19f96bedb9f882f7882".to_owned(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_owned()
        )
    );
}

#[tokio::test]
async fn all_takes_a_known_digest_without_reading_the_file() {
    let root = dataset(&[]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    fs::remove_file(root.path().join("meta/info.json")).unwrap();
    let known = HashMap::from([("meta/info.json".to_owned(), [7_u8; 32])]);

    let digests = all(&plan, &known).await.unwrap();

    assert_eq!(digests["meta/info.json"], [7_u8; 32]);
}

#[tokio::test]
async fn all_fails_with_io_for_a_file_that_shrank_since_planning() {
    let root = dataset(&[]);
    let plan = UploadPlan::from_dir(root.path()).unwrap();
    fs::write(root.path().join("meta/info.json"), b"012").unwrap();

    let error = all(&plan, &HashMap::new()).await.unwrap_err();

    assert!(matches!(error, Error::Io { .. }), "{error:?}");
}
