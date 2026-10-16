use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::*;
use crate::output::ErrorCode;

fn env_in(dir: &Path) -> Environment {
    Environment {
        config_dir: Some(dir.to_path_buf()),
        ..Environment::default()
    }
}

fn write_config(dir: &Path, content: &str) -> std::path::PathBuf {
    let path = dir.join("config.toml");
    std::fs::write(&path, content).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    path
}

fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn resolved(env: &Environment) -> (String, Source) {
    let api_url = resolve_api_url(env).unwrap();
    (api_url.to_string(), api_url.source)
}

#[test]
fn without_flag_variable_or_file_the_api_url_is_the_default() {
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(
        resolved(&env_in(dir.path())),
        (DEFAULT_API_URL.to_owned(), Source::Default)
    );
}

#[test]
fn the_config_file_takes_precedence_over_the_default() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), "api_url = \"https://file.example.com\"\n");

    assert_eq!(
        resolved(&env_in(dir.path())),
        ("https://file.example.com".to_owned(), Source::Config)
    );
}

#[test]
fn imify_api_url_takes_precedence_over_the_config_file() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), "api_url = \"https://file.example.com\"\n");
    let env = Environment {
        api_url: Some("https://env.example.com".to_owned()),
        ..env_in(dir.path())
    };

    assert_eq!(
        resolved(&env),
        ("https://env.example.com".to_owned(), Source::Env)
    );
}

#[test]
fn the_flag_takes_precedence_over_imify_api_url() {
    let dir = tempfile::tempdir().unwrap();
    let env = Environment {
        api_url_flag: Some("https://flag.example.com".to_owned()),
        api_url: Some("https://env.example.com".to_owned()),
        ..env_in(dir.path())
    };

    assert_eq!(
        resolved(&env),
        ("https://flag.example.com".to_owned(), Source::Flag)
    );
}

#[test]
fn an_empty_imify_api_url_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let env = Environment {
        api_url: Some("  ".to_owned()),
        ..env_in(dir.path())
    };

    assert_eq!(resolved(&env).1, Source::Default);
}

#[test]
fn a_set_flag_or_variable_does_not_read_the_config_file() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), "unknown = 1\n");
    let env = Environment {
        api_url: Some("https://env.example.com".to_owned()),
        ..env_in(dir.path())
    };

    assert_eq!(resolved(&env).1, Source::Env);
}

#[test]
fn every_source_rejects_plain_http_on_a_remote_host_and_names_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "api_url = \"http://file.example.com\"\n");
    let base = env_in(dir.path());
    let flag = Environment {
        api_url_flag: Some("http://flag.example.com".to_owned()),
        ..base.clone()
    };
    let variable = Environment {
        api_url: Some("http://env.example.com".to_owned()),
        ..base.clone()
    };

    let messages = [flag, variable, base].map(|env| {
        let error = resolve_api_url(&env).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        error.message
    });

    assert!(
        messages[0].starts_with("--api-url is invalid"),
        "{messages:?}"
    );
    assert!(
        messages[1].starts_with("IMIFY_API_URL is invalid"),
        "{messages:?}"
    );
    assert!(
        messages[2].contains(&path.display().to_string()),
        "{messages:?}"
    );
}

#[test]
fn every_source_rejects_user_info_query_and_fragment() {
    let dir = tempfile::tempdir().unwrap();
    for raw in [
        "https://ada:s3cret@api.example.com",
        "https://api.example.com/?debug=1",
        "https://api.example.com/#top",
    ] {
        write_config(dir.path(), &format!("api_url = \"{raw}\"\n"));
        let base = env_in(dir.path());
        let flag = Environment {
            api_url_flag: Some(raw.to_owned()),
            ..base.clone()
        };
        let variable = Environment {
            api_url: Some(raw.to_owned()),
            ..base.clone()
        };

        for env in [flag, variable, base] {
            let error = resolve_api_url(&env).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidArgument, "{raw}");
            assert!(!error.message.contains("s3cret"), "{}", error.message);
        }
    }
}

#[test]
fn http_is_accepted_for_loopback_hosts() {
    let accepted = [
        "http://localhost:8080",
        "http://127.0.0.1:8080",
        "http://[::1]:8080",
    ]
    .map(|raw| check_api_url(raw, "api-url", "Pass a URL.").is_ok());

    assert_eq!(accepted, [true; 3]);
}

#[test]
fn a_scheme_other_than_http_or_https_is_an_invalid_argument() {
    assert_eq!(
        check_api_url("ftp://api.example.com", "api-url", "Pass a URL.")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn an_unknown_key_in_the_file_is_an_invalid_argument_naming_the_key_and_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        "api_url = \"https://a.example.com\"\nprofile = \"x\"\n",
    );

    let error = resolve_api_url(&env_in(dir.path())).unwrap_err();

    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("profile"), "{}", error.message);
    assert!(
        error.message.contains(&path.display().to_string()),
        "{}",
        error.message
    );
}

#[test]
fn a_file_that_is_not_toml_is_an_invalid_argument_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "api_url = \n");

    let error = resolve_api_url(&env_in(dir.path())).unwrap_err();

    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains(&path.display().to_string()));
}

#[test]
fn a_file_other_users_can_write_to_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "api_url = \"https://a.example.com\"\n");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();

    let error = resolve_api_url(&env_in(dir.path())).unwrap_err();

    assert_eq!(error.code, ErrorCode::Internal);
    assert!(error.message.contains("chmod 600"), "{}", error.message);
}

#[test]
fn a_file_other_users_can_read_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "api_url = \"https://a.example.com\"\n");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(resolved(&env_in(dir.path())).1, Source::Config);
}

#[test]
fn a_directory_other_users_can_write_to_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), "api_url = \"https://a.example.com\"\n");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).unwrap();

    let error = resolve_api_url(&env_in(dir.path())).unwrap_err();

    assert_eq!(error.code, ErrorCode::Internal);
    assert!(error.message.contains("chmod 700"), "{}", error.message);
}

#[test]
fn saving_creates_the_file_0600_and_the_directory_0700() {
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("imify");
    let file = ConfigFile::open(&env_in(&dir)).unwrap();

    file.save(&Values {
        api_url: Some("https://a.example.com".to_owned()),
    })
    .unwrap();

    assert_eq!((mode_of(&dir), mode_of(file.path())), (0o700, 0o600));
}

#[test]
fn saved_values_load_back() {
    let dir = tempfile::tempdir().unwrap();
    let file = ConfigFile::open(&env_in(dir.path())).unwrap();
    let values = Values {
        api_url: Some("https://a.example.com".to_owned()),
    };

    file.save(&values).unwrap();

    assert_eq!(file.load().unwrap(), values);
}

#[test]
fn saving_no_values_removes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = ConfigFile::open(&env_in(dir.path())).unwrap();
    file.save(&Values {
        api_url: Some("https://a.example.com".to_owned()),
    })
    .unwrap();

    file.save(&Values::default()).unwrap();

    assert!(!file.path().exists());
}

#[test]
fn key_names_use_dashes_on_the_command_line_and_underscores_in_the_file() {
    assert_eq!(
        (Key::ApiUrl.name(), Key::ApiUrl.file_name()),
        ("api-url", "api_url")
    );
}
