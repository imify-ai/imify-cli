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

    file.save(&contents_with(&[("default", "https://a.example.com")]))
        .unwrap();

    assert_eq!((mode_of(&dir), mode_of(file.path())), (0o700, 0o600));
}

#[test]
fn saved_contents_load_back() {
    let dir = tempfile::tempdir().unwrap();
    let file = ConfigFile::open(&env_in(dir.path())).unwrap();
    let contents = contents_with(&[
        ("default", "https://a.example.com"),
        ("staging", "https://s.example.com"),
    ]);

    file.save(&contents).unwrap();

    assert_eq!(file.load().unwrap(), contents);
}

#[test]
fn saving_no_values_removes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = ConfigFile::open(&env_in(dir.path())).unwrap();
    file.save(&contents_with(&[("default", "https://a.example.com")]))
        .unwrap();

    file.save(&Contents::default()).unwrap();

    assert!(!file.path().exists());
}

#[test]
fn key_names_use_dashes_on_the_command_line_and_underscores_in_the_file() {
    assert_eq!(
        (Key::ApiUrl.name(), Key::ApiUrl.file_name()),
        ("api-url", "api_url")
    );
}

/// Contents with `api_url` set in each named profile; `default` sets the top-level key.
fn contents_with(api_urls: &[(&str, &str)]) -> Contents {
    let mut contents = Contents::default();
    for (name, url) in api_urls {
        contents
            .profile_or_insert(ProfileName::parse(name).unwrap())
            .set(Key::ApiUrl, Some((*url).to_owned()));
    }
    contents
}

fn env_with_profile(dir: &Path, flag: Option<&str>, variable: Option<&str>) -> Environment {
    Environment {
        profile_flag: flag.map(str::to_owned),
        profile: variable.map(str::to_owned),
        ..env_in(dir)
    }
}

const PROFILES: &str = "api_url = \"https://top.example.com\"\n\n\
    [profiles.staging]\napi_url = \"https://staging.example.com\"\n\n\
    [profiles.empty]\n";

#[test]
fn without_flag_or_variable_the_default_profile_is_selected() {
    let selected = select_profile(&Environment::default()).unwrap();

    assert_eq!(
        (selected.name.as_str(), selected.source),
        ("default", ProfileSource::Default)
    );
}

#[test]
fn imify_profile_selects_a_profile() {
    let env = Environment {
        profile: Some("staging".to_owned()),
        ..Environment::default()
    };

    let selected = select_profile(&env).unwrap();

    assert_eq!(
        (selected.name.as_str(), selected.source),
        ("staging", ProfileSource::Env)
    );
}

#[test]
fn the_profile_flag_takes_precedence_over_imify_profile() {
    let env = Environment {
        profile_flag: Some("local".to_owned()),
        profile: Some("staging".to_owned()),
        ..Environment::default()
    };

    let selected = select_profile(&env).unwrap();

    assert_eq!(
        (selected.name.as_str(), selected.source),
        ("local", ProfileSource::Flag)
    );
}

#[test]
fn an_empty_imify_profile_selects_the_default_profile() {
    let env = Environment {
        profile: Some(String::new()),
        ..Environment::default()
    };

    assert_eq!(select_profile(&env).unwrap().name.as_str(), "default");
}

#[test]
fn profile_names_have_1_to_64_letters_digits_dashes_and_underscores() {
    let long = "a".repeat(64);
    let too_long = "a".repeat(65);
    let names = [
        "staging", "a", "Prod-2", "9_x", &long, "", "-a", "_a", "a.b", "a b", "ä", &too_long,
    ];

    let accepted = names.map(|name| ProfileName::parse(name).is_ok());

    assert_eq!(
        accepted,
        [
            true, true, true, true, true, false, false, false, false, false, false, false
        ]
    );
}

#[test]
fn an_invalid_profile_name_from_the_flag_names_the_flag_and_escapes_control_characters() {
    let env = Environment {
        profile_flag: Some("a\u{1b}[2J".to_owned()),
        ..Environment::default()
    };

    let error = select_profile(&env).unwrap_err();

    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(
        error.message,
        "--profile has the invalid profile name 'a\\u{1b}[2J': use 1 to 64 letters, digits, \
         '-' and '_', starting with a letter or digit."
    );
}

#[test]
fn an_invalid_profile_name_from_the_variable_names_the_variable() {
    let env = Environment {
        profile: Some("a.b".to_owned()),
        ..Environment::default()
    };

    let error = select_profile(&env).unwrap_err();

    assert!(
        error.message.starts_with("IMIFY_PROFILE has the invalid"),
        "{}",
        error.message
    );
    assert!(
        error
            .hint
            .as_deref()
            .unwrap_or_default()
            .contains("IMIFY_PROFILE"),
        "{:?}",
        error.hint
    );
}

#[test]
fn a_file_with_only_top_level_keys_resolves_from_the_default_profile() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), "api_url = \"https://file.example.com\"\n");

    let api_url = resolve_api_url(&env_in(dir.path())).unwrap();

    assert_eq!(
        (
            api_url.to_string(),
            api_url.source,
            api_url.profile.as_deref()
        ),
        (
            "https://file.example.com".to_owned(),
            Source::Config,
            Some("default")
        )
    );
}

#[test]
fn a_selected_profile_supplies_its_api_url() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), PROFILES);

    let api_url = resolve_api_url(&env_with_profile(dir.path(), Some("staging"), None)).unwrap();

    assert_eq!(
        (api_url.to_string(), api_url.profile.as_deref()),
        ("https://staging.example.com".to_owned(), Some("staging"))
    );
}

#[test]
fn a_profile_without_api_url_falls_back_to_the_top_level_api_url() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), PROFILES);

    let api_url = resolve_api_url(&env_with_profile(dir.path(), None, Some("empty"))).unwrap();

    assert_eq!(
        (api_url.to_string(), api_url.profile.as_deref()),
        ("https://top.example.com".to_owned(), Some("default"))
    );
}

#[test]
fn a_profile_without_api_url_and_no_top_level_api_url_falls_back_to_the_default() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), "[profiles.empty]\n");

    let api_url = resolve_api_url(&env_with_profile(dir.path(), Some("empty"), None)).unwrap();

    assert_eq!(
        (api_url.to_string(), api_url.source, api_url.profile),
        (DEFAULT_API_URL.to_owned(), Source::Default, None)
    );
}

#[test]
fn imify_api_url_takes_precedence_over_the_selected_profile() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), PROFILES);
    let env = Environment {
        api_url: Some("https://env.example.com".to_owned()),
        ..env_with_profile(dir.path(), Some("staging"), None)
    };

    assert_eq!(
        resolved(&env),
        ("https://env.example.com".to_owned(), Source::Env)
    );
}

#[test]
fn the_api_url_flag_takes_precedence_over_the_selected_profile() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), PROFILES);
    let env = Environment {
        api_url_flag: Some("https://flag.example.com".to_owned()),
        ..env_with_profile(dir.path(), Some("staging"), None)
    };

    assert_eq!(
        resolved(&env),
        ("https://flag.example.com".to_owned(), Source::Flag)
    );
}

#[test]
fn an_api_url_from_the_flag_or_the_variable_names_no_profile() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), PROFILES);
    let base = env_with_profile(dir.path(), Some("staging"), None);
    let flag = Environment {
        api_url_flag: Some("https://flag.example.com".to_owned()),
        ..base.clone()
    };
    let variable = Environment {
        api_url: Some("https://env.example.com".to_owned()),
        ..base
    };

    let profiles = [flag, variable].map(|env| resolve_api_url(&env).unwrap().profile);

    assert_eq!(profiles, [None, None]);
}

#[test]
fn the_default_profile_selects_the_top_level_keys() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), PROFILES);

    assert_eq!(
        resolved(&env_with_profile(dir.path(), Some("default"), None)),
        ("https://top.example.com".to_owned(), Source::Config)
    );
}

#[test]
fn an_unknown_profile_is_an_invalid_argument_naming_the_profile_the_source_and_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), PROFILES);

    let error = resolve_api_url(&env_with_profile(dir.path(), None, Some("prod"))).unwrap_err();

    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(
        error.message,
        format!(
            "IMIFY_PROFILE selects the profile 'prod', which is not in {}.",
            path.display()
        )
    );
    assert_eq!(
        error.hint.as_deref(),
        Some(
            "Run `imify config profiles` to list the profiles, `imify config set api-url <URL> \
             --profile prod` to create it, or unset IMIFY_PROFILE."
        )
    );
}

#[test]
fn an_unknown_profile_fails_even_when_the_api_url_flag_is_set() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), PROFILES);
    let env = Environment {
        api_url_flag: Some("https://flag.example.com".to_owned()),
        ..env_with_profile(dir.path(), Some("prod"), None)
    };

    let error = resolve_api_url(&env).unwrap_err();

    assert!(
        error
            .message
            .starts_with("--profile selects the profile 'prod'"),
        "{}",
        error.message
    );
}

#[test]
fn an_invalid_profile_table_name_is_an_invalid_argument_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "[profiles.\"a.b\"]\n");

    let error = resolve_api_url(&env_in(dir.path())).unwrap_err();

    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(
        error.message.starts_with(&path.display().to_string()) && error.message.contains("a.b"),
        "{}",
        error.message
    );
}

#[test]
fn a_profiles_default_table_is_an_invalid_argument() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), "[profiles.default]\n");

    let error = resolve_api_url(&env_in(dir.path())).unwrap_err();

    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(
        error.message.contains("profiles.default"),
        "{}",
        error.message
    );
}

#[test]
fn an_unknown_key_in_a_profile_names_the_key_the_profile_and_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "[profiles.staging]\nhost = \"x\"\n");

    let error = resolve_api_url(&env_in(dir.path())).unwrap_err();

    assert_eq!(error.code, ErrorCode::InvalidArgument);
    for part in [path.display().to_string().as_str(), "host", "staging"] {
        assert!(error.message.contains(part), "{part}: {}", error.message);
    }
}

#[test]
fn an_invalid_api_url_in_a_profile_names_the_profile_and_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        "[profiles.staging]\napi_url = \"http://staging.example.com\"\n",
    );

    let error = resolve_api_url(&env_with_profile(dir.path(), Some("staging"), None)).unwrap_err();

    assert!(
        error.message.starts_with(&format!(
            "api_url in profile staging in {} is invalid",
            path.display()
        )),
        "{}",
        error.message
    );
    assert_eq!(
        error.hint.as_deref(),
        Some(
            "Run `imify config set api-url <URL> --profile staging`, or `imify config unset \
             api-url --profile staging`."
        )
    );
}

#[test]
fn saving_without_profiles_writes_no_profiles_table() {
    let dir = tempfile::tempdir().unwrap();
    let file = ConfigFile::open(&env_in(dir.path())).unwrap();

    file.save(&contents_with(&[("default", "https://a.example.com")]))
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(file.path()).unwrap(),
        "api_url = \"https://a.example.com\"\n"
    );
}

#[test]
fn saving_writes_the_top_level_keys_then_the_profiles_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let file = ConfigFile::open(&env_in(dir.path())).unwrap();

    file.save(&contents_with(&[
        ("staging", "https://s.example.com"),
        ("default", "https://d.example.com"),
        ("local", "http://localhost:8000"),
    ]))
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(file.path()).unwrap(),
        "api_url = \"https://d.example.com\"\n\n\
         [profiles.local]\napi_url = \"http://localhost:8000\"\n\n\
         [profiles.staging]\napi_url = \"https://s.example.com\"\n"
    );
}

#[test]
fn an_empty_profile_table_is_kept_when_saving() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), PROFILES);
    let file = ConfigFile::open(&env_in(dir.path())).unwrap();
    let contents = file.load().unwrap();

    file.save(&contents).unwrap();

    let loaded = file.load().unwrap();
    assert_eq!(loaded, contents);
    assert!(
        loaded
            .profile(&ProfileName::parse("empty").unwrap())
            .is_some()
    );
}

#[test]
fn removing_the_last_key_of_a_profile_removes_its_table() {
    let mut contents = contents_with(&[("staging", "https://s.example.com")]);

    contents.remove(&ProfileName::parse("staging").unwrap(), Key::ApiUrl);

    assert_eq!(contents, Contents::default());
}

#[test]
fn profile_names_list_default_first_then_the_others_by_name() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), PROFILES);
    let contents = ConfigFile::open(&env_in(dir.path()))
        .unwrap()
        .load()
        .unwrap();

    let names: Vec<&str> = contents.profile_names().map(ProfileName::as_str).collect();

    assert_eq!(names, ["default", "empty", "staging"]);
}
