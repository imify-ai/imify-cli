use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use imify::auth::{CredentialStore, Credentials};
use serde_json::json;

use super::*;
use crate::output::{CliError, ErrorCode};

const HOST: &str = "https://api.example.com";

/// Credentials whose ID token is `id_token`.
fn credentials(id_token: &str) -> Credentials {
    serde_json::from_value(json!({
        "version": 1,
        "issuer": "https://login.example.com",
        "client_id": "client-1",
        "token_endpoint": "https://login.example.com/oauth2/token",
        "revocation_endpoint": "https://login.example.com/oauth2/revoke",
        "id_token": id_token,
        "id_token_expires_at": 4_000_000_000_u64,
        "refresh_token": "rt_secret",
    }))
    .unwrap()
}

fn id_token_of(credentials: &Credentials) -> String {
    serde_json::to_value(credentials).unwrap()["id_token"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn url(raw: &str) -> url::Url {
    url::Url::parse(raw).unwrap()
}

fn env_with(store: Option<&str>, dir: &Path) -> Environment {
    Environment {
        credential_store: store.map(str::to_owned),
        config_dir: Some(dir.into()),
        ..Environment::default()
    }
}

fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn host_key_ignores_case_default_port_and_trailing_slash() {
    assert_eq!(host_key(&url("https://API.example.com:443/")), HOST);
}

#[test]
fn host_key_keeps_a_non_default_port_and_the_path() {
    assert_eq!(
        host_key(&url("http://127.0.0.1:8080/imify/")),
        "http://127.0.0.1:8080/imify"
    );
}

#[test]
fn store_choice_defaults_to_auto() {
    assert_eq!(
        [StoreChoice::parse(None), StoreChoice::parse(Some(""))],
        [Ok(StoreChoice::Auto), Ok(StoreChoice::Auto)]
    );
}

#[test]
fn store_choice_accepts_keychain_and_file() {
    assert_eq!(
        [
            StoreChoice::parse(Some("keychain")),
            StoreChoice::parse(Some("file"))
        ],
        [Ok(StoreChoice::Keychain), Ok(StoreChoice::File)]
    );
}

#[test]
fn an_unsupported_store_choice_is_an_invalid_argument_naming_the_variable() {
    let error = StoreChoice::parse(Some("vault")).unwrap_err();

    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("IMIFY_CREDENTIAL_STORE"));
}

#[test]
fn file_store_round_trips_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path(), HOST);

    store.save(&credentials("id-1")).unwrap();

    assert_eq!(id_token_of(&store.load().unwrap().unwrap()), "id-1");
}

#[test]
fn file_store_load_without_a_file_returns_none() {
    let dir = tempfile::tempdir().unwrap();

    assert!(FileStore::new(dir.path(), HOST).load().unwrap().is_none());
}

#[test]
fn file_store_creates_the_file_0600_and_the_directory_0700() {
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("imify");

    FileStore::new(&dir, HOST)
        .save(&credentials("id-1"))
        .unwrap();

    assert_eq!(
        (mode_of(&dir), mode_of(&dir.join("credentials.json"))),
        (0o700, 0o600)
    );
}

#[test]
fn file_store_rejects_a_file_readable_by_others_and_names_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path(), HOST);
    store.save(&credentials("id-1")).unwrap();
    let path = dir.path().join("credentials.json");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let error = store.load().unwrap_err();

    assert!(error.to_string().contains(&path.display().to_string()));
}

#[test]
fn file_store_keeps_one_entry_per_host() {
    let dir = tempfile::tempdir().unwrap();
    let first = FileStore::new(dir.path(), HOST);
    let second = FileStore::new(dir.path(), "https://other.example.com");

    first.save(&credentials("id-1")).unwrap();
    second.save(&credentials("id-2")).unwrap();

    assert_eq!(
        [
            id_token_of(&first.load().unwrap().unwrap()),
            id_token_of(&second.load().unwrap().unwrap())
        ],
        ["id-1", "id-2"]
    );
}

#[test]
fn file_store_delete_removes_only_the_entry_of_its_host() {
    let dir = tempfile::tempdir().unwrap();
    let first = FileStore::new(dir.path(), HOST);
    let second = FileStore::new(dir.path(), "https://other.example.com");
    first.save(&credentials("id-1")).unwrap();
    second.save(&credentials("id-2")).unwrap();

    let removed = first.delete().unwrap();

    assert!(removed);
    assert!(first.load().unwrap().is_none());
    assert!(second.load().unwrap().is_some());
}

#[test]
fn file_store_delete_without_an_entry_returns_false() {
    let dir = tempfile::tempdir().unwrap();

    assert!(!FileStore::new(dir.path(), HOST).delete().unwrap());
}

#[test]
fn keychain_store_round_trips_credentials() {
    let store = KeychainStore::new(Keychain::in_memory(), HOST);

    store.try_save(&credentials("id-1")).unwrap();

    assert_eq!(id_token_of(&store.try_load().unwrap().unwrap()), "id-1");
}

#[test]
fn keychain_store_delete_reports_whether_credentials_existed() {
    let store = KeychainStore::new(Keychain::in_memory(), HOST);
    store.try_save(&credentials("id-1")).unwrap();

    assert_eq!(
        [store.try_delete().unwrap(), store.try_delete().unwrap()],
        [true, false]
    );
}

fn host_store(choice: Option<&str>, keychain: Keychain, dir: &Path) -> HostStore {
    HostStore::open(&env_with(choice, dir), HOST, keychain).unwrap()
}

#[test]
fn auto_saves_in_the_keychain_when_one_is_available() {
    let dir = tempfile::tempdir().unwrap();
    let store = host_store(None, Keychain::in_memory(), dir.path());

    let saved = store.save_reporting(&credentials("id-1")).unwrap().saved;

    assert_eq!(saved, Saved::Keychain);
    assert!(!dir.path().join("credentials.json").exists());
}

#[test]
fn auto_falls_back_to_the_file_when_no_keychain_is_available() {
    let dir = tempfile::tempdir().unwrap();
    let store = host_store(None, Keychain::unavailable(), dir.path());

    let saved = store.save_reporting(&credentials("id-1")).unwrap().saved;

    assert!(matches!(
        saved,
        Saved::FileFallback { ref path, .. } if *path == dir.path().join("credentials.json")
    ));
    assert_eq!(id_token_of(&store.load().unwrap().unwrap()), "id-1");
}

#[test]
fn auto_loads_from_the_file_when_the_keychain_holds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    FileStore::new(dir.path(), HOST)
        .save(&credentials("id-file"))
        .unwrap();
    let store = host_store(None, Keychain::in_memory(), dir.path());

    assert_eq!(id_token_of(&store.load().unwrap().unwrap()), "id-file");
}

#[test]
fn auto_prefers_the_keychain_over_the_file() {
    let dir = tempfile::tempdir().unwrap();
    FileStore::new(dir.path(), HOST)
        .save(&credentials("id-file"))
        .unwrap();
    let store = host_store(None, Keychain::in_memory(), dir.path());
    store.save(&credentials("id-keychain")).unwrap();

    assert_eq!(id_token_of(&store.load().unwrap().unwrap()), "id-keychain");
}

#[test]
fn auto_delete_removes_the_login_from_keychain_and_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = FileStore::new(dir.path(), HOST);
    file.save(&credentials("id-file")).unwrap();
    let store = host_store(None, Keychain::in_memory(), dir.path());
    store.save(&credentials("id-keychain")).unwrap();

    assert!(store.delete().unwrap());
    assert!(store.load().unwrap().is_none());
    assert!(file.load().unwrap().is_none());
}

#[test]
fn keychain_choice_does_not_fall_back_to_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = host_store(Some("keychain"), Keychain::unavailable(), dir.path());

    let error = CliError::from(store.save_reporting(&credentials("id-1")).unwrap_err());

    assert!(!dir.path().join("credentials.json").exists());
    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(
        error.hint.as_deref(),
        Some("Set `IMIFY_CREDENTIAL_STORE=file` to store the login in a file.")
    );
}

#[test]
fn file_choice_does_not_touch_the_keychain() {
    let dir = tempfile::tempdir().unwrap();
    let store = host_store(Some("file"), Keychain::unavailable(), dir.path());

    let saved = store.save_reporting(&credentials("id-1")).unwrap().saved;

    assert_eq!(saved, Saved::File(dir.path().join("credentials.json")));
}

#[test]
fn a_file_store_error_has_no_keychain_hint() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path(), HOST);
    store.save(&credentials("id-1")).unwrap();
    let path = dir.path().join("credentials.json");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

    let error = CliError::from(store.load().unwrap_err());

    assert_eq!((error.code, error.hint), (ErrorCode::Internal, None));
}

#[test]
fn stores_do_not_print_tokens_in_debug_output() {
    let dir = tempfile::tempdir().unwrap();
    let store = host_store(None, Keychain::in_memory(), dir.path());
    store.save(&credentials("id-secret")).unwrap();

    assert!(!format!("{store:?}").contains("secret"));
}

fn credentials_file(dir: &Path) -> std::path::PathBuf {
    dir.join("credentials.json")
}

#[test]
fn auto_with_a_locked_keyring_fails_instead_of_writing_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = host_store(None, Keychain::locked(HOST), dir.path());

    let error = CliError::from(store.save_reporting(&credentials("id-1")).unwrap_err());

    assert!(!credentials_file(dir.path()).exists());
    assert_eq!(
        (error.code, error.hint.as_deref()),
        (
            ErrorCode::Internal,
            Some("Unlock your keyring, or set `IMIFY_CREDENTIAL_STORE=file`.")
        )
    );
}

#[test]
fn auto_with_a_locked_keyring_does_not_read_an_older_login_from_the_file() {
    let dir = tempfile::tempdir().unwrap();
    FileStore::new(dir.path(), HOST)
        .save(&credentials("id-stale"))
        .unwrap();
    let store = host_store(None, Keychain::locked(HOST), dir.path());

    let error = CliError::from(store.load().unwrap_err());

    assert_eq!(error.code, ErrorCode::Internal);
}

#[test]
fn auto_removes_the_file_entry_after_saving_in_the_keychain() {
    let dir = tempfile::tempdir().unwrap();
    let file = FileStore::new(dir.path(), HOST);
    file.save(&credentials("id-file")).unwrap();
    let store = host_store(None, Keychain::in_memory(), dir.path());

    let report = store.save_reporting(&credentials("id-keychain")).unwrap();

    assert_eq!(report.cleanup_failure, None);
    assert!(file.load().unwrap().is_none());
}

#[test]
fn auto_delete_removes_the_file_entry_and_then_reports_the_keychain_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = FileStore::new(dir.path(), HOST);
    file.save(&credentials("id-file")).unwrap();
    let store = host_store(None, Keychain::locked(HOST), dir.path());

    let error = store.delete().unwrap_err();

    assert!(file.load().unwrap().is_none());
    assert!(error.to_string().contains("refuses access"), "{error}");
}

#[test]
fn file_store_does_not_follow_a_symbolic_link() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    FileStore::new(elsewhere.path(), HOST)
        .save(&credentials("id-1"))
        .unwrap();
    std::os::unix::fs::symlink(
        credentials_file(elsewhere.path()),
        credentials_file(dir.path()),
    )
    .unwrap();

    let error = FileStore::new(dir.path(), HOST).load().unwrap_err();

    assert!(error.to_string().contains("symbolic link"), "{error}");
}

#[test]
fn file_store_rejects_a_directory_other_users_can_write_to() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).unwrap();

    let error = FileStore::new(dir.path(), HOST)
        .save(&credentials("id-1"))
        .unwrap_err();

    assert!(error.to_string().contains("chmod 700"), "{error}");
    assert!(!credentials_file(dir.path()).exists());
}

#[test]
fn file_store_rejects_a_file_larger_than_1_mib() {
    let dir = tempfile::tempdir().unwrap();
    let path = credentials_file(dir.path());
    std::fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let error = FileStore::new(dir.path(), HOST).load().unwrap_err();

    assert!(error.to_string().contains("1 MiB"), "{error}");
}

#[test]
fn file_store_delete_removes_the_entry_from_a_file_readable_by_others() {
    let dir = tempfile::tempdir().unwrap();
    let first = FileStore::new(dir.path(), HOST);
    let second = FileStore::new(dir.path(), "https://other.example.com");
    first.save(&credentials("id-1")).unwrap();
    second.save(&credentials("id-2")).unwrap();
    let path = credentials_file(dir.path());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    assert!(first.delete().unwrap());
    assert_eq!(mode_of(&path), 0o600);
    assert!(second.load().unwrap().is_some());
}

#[test]
fn file_store_delete_removes_a_file_that_is_not_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = credentials_file(dir.path());
    std::fs::write(&path, "not json").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

    assert!(FileStore::new(dir.path(), HOST).delete().unwrap());
    assert!(!path.exists());
}

#[test]
fn file_store_delete_of_the_only_entry_removes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path(), HOST);
    store.save(&credentials("id-1")).unwrap();

    assert!(store.delete().unwrap());
    assert!(!credentials_file(dir.path()).exists());
}

#[test]
fn concurrent_saves_of_two_hosts_keep_both_entries() {
    let dir = tempfile::tempdir().unwrap();
    let hosts = [HOST, "https://other.example.com"];

    std::thread::scope(|scope| {
        for host in hosts {
            let store = FileStore::new(dir.path(), host);
            scope.spawn(move || {
                for round in 0..20 {
                    store.save(&credentials(&format!("id-{round}"))).unwrap();
                }
            });
        }
    });

    for host in hosts {
        assert_eq!(
            id_token_of(&FileStore::new(dir.path(), host).load().unwrap().unwrap()),
            "id-19"
        );
    }
}

/// Where `store` reports the login, `None` when it holds none.
fn found_in(store: &HostStore) -> Option<Location> {
    store
        .load_reporting()
        .unwrap()
        .map(|(_, location)| location)
}

#[test]
fn auto_reports_a_login_in_the_keychain_as_found_in_the_keychain() {
    let dir = tempfile::tempdir().unwrap();
    let store = host_store(None, Keychain::in_memory(), dir.path());
    store.save(&credentials("id-keychain")).unwrap();

    assert_eq!(found_in(&store), Some(Location::Keychain));
}

#[test]
fn auto_reports_a_login_in_the_file_with_its_path_when_the_keychain_holds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    FileStore::new(dir.path(), HOST)
        .save(&credentials("id-file"))
        .unwrap();
    let store = host_store(None, Keychain::in_memory(), dir.path());

    assert_eq!(
        found_in(&store),
        Some(Location::File(credentials_file(dir.path())))
    );
}

#[test]
fn auto_reports_a_login_in_the_file_when_no_keychain_is_available() {
    let dir = tempfile::tempdir().unwrap();
    FileStore::new(dir.path(), HOST)
        .save(&credentials("id-file"))
        .unwrap();
    let store = host_store(None, Keychain::unavailable(), dir.path());

    assert_eq!(
        found_in(&store),
        Some(Location::File(credentials_file(dir.path())))
    );
}

#[test]
fn file_choice_reports_the_login_as_found_in_the_file_with_its_path() {
    let dir = tempfile::tempdir().unwrap();
    FileStore::new(dir.path(), HOST)
        .save(&credentials("id-file"))
        .unwrap();
    let store = host_store(Some("file"), Keychain::unavailable(), dir.path());

    assert_eq!(
        found_in(&store),
        Some(Location::File(credentials_file(dir.path())))
    );
}

#[test]
fn load_reporting_without_a_login_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = host_store(None, Keychain::in_memory(), dir.path());

    assert_eq!(found_in(&store), None);
}

#[test]
fn auto_searches_the_keychain_then_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = host_store(None, Keychain::in_memory(), dir.path());

    assert_eq!(
        store.locations(),
        [
            Location::Keychain,
            Location::File(credentials_file(dir.path()))
        ]
    );
}

#[test]
fn location_names_the_keychain_or_the_file_path() {
    assert_eq!(
        [
            Location::Keychain.to_string(),
            Location::File("/home/robot/.config/imify/credentials.json".into()).to_string()
        ],
        ["the keychain", "/home/robot/.config/imify/credentials.json"]
    );
}
