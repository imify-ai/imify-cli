//! `imify config` and the sources of the API URL, with the config directory in a temporary
//! directory.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A config directory in a temporary directory. The directory itself does not exist until a
/// command creates it.
struct ConfigDir {
    _parent: TempDir,
    dir: PathBuf,
}

impl ConfigDir {
    fn new() -> io::Result<Self> {
        let parent = tempfile::tempdir()?;
        let dir = parent.path().join("imify");
        Ok(Self {
            _parent: parent,
            dir,
        })
    }

    fn file(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    /// Writes `content` to the config file, with the directory's mode `0700`.
    fn write(&self, content: &str) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
        std::fs::write(self.file(), content)
    }

    /// Runs `imify` with this config directory, without the variables the CLI reads and with
    /// logins read from a file in this directory, never from the keychain.
    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_imify"));
        command
            .env_remove("IMIFY_OUTPUT")
            .env_remove("NO_COLOR")
            .env_remove("IMIFY_TOKEN")
            .env_remove("IMIFY_API_URL")
            .env_remove("IMIFY_PROFILE")
            .env_remove("IMIFY_OIDC_ISSUER")
            .env_remove("IMIFY_OIDC_CLIENT_ID")
            .env("IMIFY_CREDENTIAL_STORE", "file")
            .env("IMIFY_CONFIG_DIR", &self.dir);
        for (key, value) in env {
            command.env(key, value);
        }
        command.args(args).assert().get_output().clone()
    }

    /// stdout with the config directory replaced by `<config_dir>`.
    fn stdout(&self, output: &Output) -> String {
        text(&output.stdout).replace(&self.dir.display().to_string(), "<config_dir>")
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `bytes` as JSON; `null` when they are not JSON.
fn json_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_default()
}

fn mode_of(path: &Path) -> io::Result<u32> {
    Ok(std::fs::metadata(path)?.permissions().mode() & 0o777)
}

#[test]
fn get_without_flag_variable_or_file_prints_the_default() {
    let config = ConfigDir::new().unwrap();

    let output = config.run(&["config", "get", "api-url", "--json"], &[]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    insta::assert_snapshot!("get_default_json", text(&output.stdout));
}

#[test]
fn get_prints_the_value_and_the_source_for_people() {
    let config = ConfigDir::new().unwrap();

    let output = config.run(&["config", "get", "api-url"], &[]);

    assert_eq!(
        text(&output.stdout),
        "https://platform.imify.ai (default)\n"
    );
}

#[test]
fn get_names_each_source_in_order_of_precedence() {
    let config = ConfigDir::new().unwrap();
    config
        .write("api_url = \"https://file.example.com\"\n")
        .unwrap();
    let source = |args: &[&str], env: &[(&str, &str)]| {
        let mut all = vec!["config", "get", "api-url", "--json"];
        all.extend_from_slice(args);
        let output = config.run(&all, env);
        json_of(&output.stdout)["source"].clone()
    };

    let sources = [
        source(&[], &[]),
        source(&[], &[("IMIFY_API_URL", "https://env.example.com")]),
        source(
            &["--api-url", "https://flag.example.com"],
            &[("IMIFY_API_URL", "https://env.example.com")],
        ),
    ];

    assert_eq!(sources, [json!("config"), json!("env"), json!("flag")]);
}

#[test]
fn set_stores_the_value_that_get_then_reports_from_the_config_file() {
    let config = ConfigDir::new().unwrap();

    let set = config.run(
        &[
            "config",
            "set",
            "api-url",
            "https://api.example.com/",
            "--json",
        ],
        &[],
    );
    let get = config.run(&["config", "get", "api-url", "--json"], &[]);

    assert_eq!(set.status.code(), Some(0), "{}", text(&set.stderr));
    insta::assert_snapshot!("set_json", config.stdout(&set));
    assert_eq!(
        json_of(&get.stdout),
        json!({"key": "api-url", "value": "https://api.example.com", "source": "config", "profile": "default"})
    );
}

#[test]
fn set_writes_the_file_with_mode_0600_in_a_directory_with_mode_0700() {
    let config = ConfigDir::new().unwrap();

    config.run(
        &["config", "set", "api-url", "https://api.example.com"],
        &[],
    );

    assert_eq!(
        (
            mode_of(&config.dir).unwrap(),
            mode_of(&config.file()).unwrap()
        ),
        (0o700, 0o600)
    );
    assert_eq!(
        std::fs::read_to_string(config.file()).unwrap(),
        "api_url = \"https://api.example.com\"\n"
    );
}

/// Runs `imify config set api-url https://api.example.com --json` with `HOME` set to `home`,
/// `IMIFY_CONFIG_DIR` and `XDG_CONFIG_HOME` unset, then `env` applied, and returns the `path`
/// it reports.
fn set_in_home(home: &Path, env: &[(&str, &str)]) -> PathBuf {
    let mut command = Command::new(env!("CARGO_BIN_EXE_imify"));
    command
        .env_remove("IMIFY_OUTPUT")
        .env_remove("IMIFY_API_URL")
        .env_remove("IMIFY_PROFILE")
        .env_remove("IMIFY_CONFIG_DIR")
        .env_remove("XDG_CONFIG_HOME")
        .env("IMIFY_CREDENTIAL_STORE", "file")
        .env("HOME", home);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command
        .args([
            "config",
            "set",
            "api-url",
            "https://api.example.com",
            "--json",
        ])
        .assert()
        .get_output()
        .clone();
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    PathBuf::from(json_of(&output.stdout)["path"].as_str().unwrap_or_default())
}

#[test]
fn set_without_imify_config_dir_writes_below_dot_config_in_home() {
    let home = tempfile::tempdir().unwrap();

    let path = set_in_home(home.path(), &[]);

    let dir = home.path().join(".config/imify");
    assert_eq!(path, dir.join("config.toml"));
    assert_eq!(
        (mode_of(&dir).unwrap(), mode_of(&path).unwrap()),
        (0o700, 0o600)
    );
}

#[test]
fn set_without_imify_config_dir_writes_below_xdg_config_home() {
    let home = tempfile::tempdir().unwrap();
    let config_home = home.path().join("xdg-config");

    let path = set_in_home(
        home.path(),
        &[("XDG_CONFIG_HOME", config_home.to_str().unwrap())],
    );

    assert_eq!(path, config_home.join("imify/config.toml"));
    assert!(path.is_file());
}

#[test]
fn a_relative_home_without_imify_config_dir_is_an_invalid_argument() {
    let output = Command::new(env!("CARGO_BIN_EXE_imify"))
        .env_remove("IMIFY_CONFIG_DIR")
        .env_remove("XDG_CONFIG_HOME")
        .env("HOME", "relative/home")
        .args(["config", "get", "api-url", "--json"])
        .assert()
        .get_output()
        .clone();

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_of(&output.stderr)["error"]["code"], "invalid_argument");
}

#[test]
fn set_rejects_an_invalid_url_without_writing() {
    let config = ConfigDir::new().unwrap();

    let output = config.run(
        &[
            "config",
            "set",
            "api-url",
            "http://api.example.com",
            "--json",
        ],
        &[],
    );

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_of(&output.stderr)["error"]["code"], "invalid_argument");
    assert!(!config.file().exists());
}

#[test]
fn set_warns_when_imify_api_url_takes_precedence() {
    let config = ConfigDir::new().unwrap();

    let output = config.run(
        &["config", "set", "api-url", "https://api.example.com"],
        &[("IMIFY_API_URL", "https://env.example.com")],
    );

    assert_eq!(output.status.code(), Some(0));
    assert!(
        text(&output.stderr).contains("IMIFY_API_URL is set and takes precedence"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn an_unknown_key_on_the_command_line_is_a_usage_error_naming_the_key() {
    let config = ConfigDir::new().unwrap();

    let output = config.run(&["config", "set", "profile", "x", "--json"], &[]);

    assert_eq!(output.status.code(), Some(2));
    let envelope = json_of(&output.stderr);
    assert_eq!(envelope["error"]["code"], "invalid_argument");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("profile")
    );
}

#[test]
fn the_file_name_of_a_key_is_not_accepted_on_the_command_line() {
    let config = ConfigDir::new().unwrap();

    let output = config.run(&["config", "get", "api_url"], &[]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn an_unknown_key_in_the_file_is_an_invalid_argument_naming_the_key_and_the_file() {
    let config = ConfigDir::new().unwrap();
    config
        .write("api_url = \"https://api.example.com\"\nprofile = \"staging\"\n")
        .unwrap();

    let output = config.run(&["config", "list", "--json"], &[]);

    assert_eq!(output.status.code(), Some(2));
    let message = json_of(&output.stderr)["error"]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(message.contains("profile"), "{message}");
    assert!(
        message.contains(&config.file().display().to_string()),
        "{message}"
    );
}

#[test]
fn an_invalid_url_in_the_file_is_an_invalid_argument_naming_the_file() {
    let config = ConfigDir::new().unwrap();
    config
        .write("api_url = \"http://api.example.com\"\n")
        .unwrap();

    let output = config.run(&["auth", "whoami", "--json"], &[("IMIFY_TOKEN", "tok")]);

    assert_eq!(output.status.code(), Some(2));
    let message = json_of(&output.stderr)["error"]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        message.contains(&config.file().display().to_string()),
        "{message}"
    );
}

#[test]
fn unset_removes_the_key_and_get_falls_back_to_the_default() {
    let config = ConfigDir::new().unwrap();
    config.run(
        &["config", "set", "api-url", "https://api.example.com"],
        &[],
    );

    let unset = config.run(&["config", "unset", "api-url", "--json"], &[]);
    let get = config.run(&["config", "get", "api-url", "--json"], &[]);

    assert_eq!(unset.status.code(), Some(0), "{}", text(&unset.stderr));
    insta::assert_snapshot!("unset_json", config.stdout(&unset));
    assert_eq!(json_of(&get.stdout)["source"], "default");
    assert!(!config.file().exists());
}

#[test]
fn unset_of_a_key_that_is_not_set_succeeds_and_reports_nothing_removed() {
    let config = ConfigDir::new().unwrap();

    let output = config.run(&["config", "unset", "api-url", "--json"], &[]);

    assert_eq!(output.status.code(), Some(0));
    insta::assert_snapshot!("unset_not_set_json", config.stdout(&output));
}

#[test]
fn list_prints_every_key_with_value_and_source() {
    let config = ConfigDir::new().unwrap();
    config
        .write("api_url = \"https://api.example.com\"\n")
        .unwrap();

    let json = config.run(&["config", "list", "--json"], &[]);
    let human = config.run(&["config", "list"], &[]);

    insta::assert_snapshot!("list_json", text(&json.stdout));
    insta::assert_snapshot!("list_human", text(&human.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn commands_use_the_api_url_from_the_config_file() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"subject": "user-1234", "name": "Ada Lovelace", "role": "admin"}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let config = ConfigDir::new().unwrap();
    config.run(&["config", "set", "api-url", &server.uri()], &[]);

    let output = config.run(&["auth", "whoami", "--json"], &[("IMIFY_TOKEN", "tok")]);

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
}

#[test]
fn config_help_shows_examples() {
    let config = ConfigDir::new().unwrap();

    for verb in ["get", "set", "unset", "list", "profiles"] {
        let output = config.run(&["config", verb, "--help"], &[]);
        assert!(
            text(&output.stdout).contains(&format!("imify config {verb}")),
            "{verb}"
        );
    }
}

const PROFILES: &str = "api_url = \"https://top.example.com\"\n\n\
    [profiles.staging]\napi_url = \"https://staging.example.com\"\n\n\
    [profiles.local]\napi_url = \"http://localhost:8000\"\n\n\
    [profiles.empty]\n";

impl ConfigDir {
    /// `imify config get api-url --json` with `args` and `env` added, as JSON.
    fn get_api_url(&self, args: &[&str], env: &[(&str, &str)]) -> Value {
        let mut all = vec!["config", "get", "api-url", "--json"];
        all.extend_from_slice(args);
        let output = self.run(&all, env);
        assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
        json_of(&output.stdout)
    }

    fn content(&self) -> String {
        std::fs::read_to_string(self.file()).unwrap_or_default()
    }
}

/// The `code` and `message` of the error envelope on stderr.
fn error_of(output: &Output) -> (String, String) {
    let envelope = json_of(&output.stderr);
    let field = |name: &str| {
        envelope["error"][name]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    (field("code"), field("message"))
}

#[test]
fn get_from_a_file_with_only_top_level_keys_reports_the_default_profile() {
    let config = ConfigDir::new().unwrap();
    config
        .write("api_url = \"https://file.example.com\"\n")
        .unwrap();

    let get = config.get_api_url(&[], &[]);

    assert_eq!(
        (&get["source"], &get["profile"]),
        (&json!("config"), &json!("default"))
    );
}

#[test]
fn the_profile_flag_selects_the_api_url_of_the_profile() {
    let config = ConfigDir::new().unwrap();
    config.write(PROFILES).unwrap();

    let get = config.get_api_url(&["--profile", "staging"], &[]);

    assert_eq!(
        get,
        json!({"key": "api-url", "value": "https://staging.example.com", "source": "config", "profile": "staging"})
    );
}

#[test]
fn imify_profile_selects_the_api_url_of_the_profile() {
    let config = ConfigDir::new().unwrap();
    config.write(PROFILES).unwrap();

    let get = config.get_api_url(&[], &[("IMIFY_PROFILE", "staging")]);

    assert_eq!(get["value"], "https://staging.example.com");
}

#[test]
fn the_profile_flag_takes_precedence_over_imify_profile() {
    let config = ConfigDir::new().unwrap();
    config.write(PROFILES).unwrap();

    let get = config.get_api_url(&["--profile", "local"], &[("IMIFY_PROFILE", "staging")]);

    assert_eq!(get["value"], "http://localhost:8000");
}

#[test]
fn an_empty_imify_profile_selects_the_default_profile() {
    let config = ConfigDir::new().unwrap();
    config.write(PROFILES).unwrap();

    let get = config.get_api_url(&[], &[("IMIFY_PROFILE", "")]);

    assert_eq!(get["value"], "https://top.example.com");
}

#[test]
fn a_profile_without_api_url_falls_back_to_the_top_level_api_url() {
    let config = ConfigDir::new().unwrap();
    config.write(PROFILES).unwrap();

    let get = config.get_api_url(&["--profile", "empty"], &[]);

    assert_eq!(
        (&get["value"], &get["profile"]),
        (&json!("https://top.example.com"), &json!("default"))
    );
}

#[test]
fn a_profile_without_api_url_and_no_top_level_api_url_falls_back_to_the_default() {
    let config = ConfigDir::new().unwrap();
    config.write("[profiles.empty]\n").unwrap();

    let get = config.get_api_url(&["--profile", "empty"], &[]);

    assert_eq!(
        get,
        json!({"key": "api-url", "value": "https://platform.imify.ai", "source": "default", "profile": null})
    );
}

#[test]
fn imify_api_url_takes_precedence_over_the_selected_profile() {
    let config = ConfigDir::new().unwrap();
    config.write(PROFILES).unwrap();

    let get = config.get_api_url(
        &["--profile", "staging"],
        &[("IMIFY_API_URL", "https://env.example.com")],
    );

    assert_eq!(
        get,
        json!({"key": "api-url", "value": "https://env.example.com", "source": "env", "profile": null})
    );
}

#[test]
fn the_api_url_flag_takes_precedence_over_the_selected_profile() {
    let config = ConfigDir::new().unwrap();
    config.write(PROFILES).unwrap();

    let get = config.get_api_url(
        &[
            "--profile",
            "staging",
            "--api-url",
            "https://flag.example.com",
        ],
        &[],
    );

    assert_eq!(
        (&get["value"], &get["source"]),
        (&json!("https://flag.example.com"), &json!("flag"))
    );
}

#[test]
fn the_default_profile_selects_the_top_level_keys() {
    let config = ConfigDir::new().unwrap();
    config.write(PROFILES).unwrap();

    let get = config.get_api_url(&["--profile", "default"], &[]);

    assert_eq!(get["value"], "https://top.example.com");
}

#[test]
fn an_unknown_profile_fails_get_naming_the_profile_and_the_source() {
    let config = ConfigDir::new().unwrap();
    config.write(PROFILES).unwrap();

    let output = config.run(
        &["config", "get", "api-url", "--json"],
        &[("IMIFY_PROFILE", "prod")],
    );

    assert_eq!(output.status.code(), Some(2));
    let (code, message) = error_of(&output);
    assert_eq!(code, "invalid_argument");
    assert!(
        message.starts_with("IMIFY_PROFILE selects the profile 'prod'"),
        "{message}"
    );
}

#[test]
fn an_unknown_profile_fails_even_when_the_api_url_flag_is_set() {
    let config = ConfigDir::new().unwrap();

    let output = config.run(
        &[
            "config",
            "get",
            "api-url",
            "--json",
            "--profile",
            "prod",
            "--api-url",
            "https://flag.example.com",
        ],
        &[],
    );

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(error_of(&output).0, "invalid_argument");
}

#[test]
fn an_invalid_profile_name_from_the_flag_fails_and_writes_nothing() {
    let config = ConfigDir::new().unwrap();

    let output = config.run(
        &[
            "config",
            "set",
            "api-url",
            "https://api.example.com",
            "--profile",
            "a.b",
            "--json",
        ],
        &[],
    );

    assert_eq!(output.status.code(), Some(2));
    let (code, message) = error_of(&output);
    assert_eq!(code, "invalid_argument");
    assert!(
        message.starts_with("--profile has the invalid profile name 'a.b'"),
        "{message}"
    );
    assert!(!config.file().exists());
}

#[test]
fn an_invalid_profile_table_name_in_the_file_fails_naming_the_file() {
    let config = ConfigDir::new().unwrap();
    config.write("[profiles.\"a b\"]\n").unwrap();

    let output = config.run(&["config", "list", "--json"], &[]);

    assert_eq!(output.status.code(), Some(2));
    let (code, message) = error_of(&output);
    assert_eq!(code, "invalid_argument");
    assert!(
        message.starts_with(&config.file().display().to_string()),
        "{message}"
    );
}

#[test]
fn a_profiles_default_table_fails_with_invalid_argument() {
    let config = ConfigDir::new().unwrap();
    config
        .write("[profiles.default]\napi_url = \"https://api.example.com\"\n")
        .unwrap();

    let output = config.run(&["config", "list", "--json"], &[]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(error_of(&output).0, "invalid_argument");
}

#[test]
fn an_unknown_key_in_a_profile_fails_naming_the_key_the_profile_and_the_file() {
    let config = ConfigDir::new().unwrap();
    config.write("[profiles.staging]\nhost = \"x\"\n").unwrap();

    let output = config.run(&["config", "list", "--json"], &[]);

    assert_eq!(output.status.code(), Some(2));
    let message = error_of(&output).1;
    for part in [
        config.file().display().to_string().as_str(),
        "host",
        "staging",
    ] {
        assert!(message.contains(part), "{part}: {message}");
    }
}

#[test]
fn an_invalid_api_url_in_a_profile_fails_naming_the_profile_and_the_file() {
    let config = ConfigDir::new().unwrap();
    config
        .write("[profiles.staging]\napi_url = \"http://staging.example.com\"\n")
        .unwrap();

    let output = config.run(
        &["auth", "whoami", "--json", "--profile", "staging"],
        &[("IMIFY_TOKEN", "tok")],
    );

    assert_eq!(output.status.code(), Some(2));
    let message = error_of(&output).1;
    assert!(
        message.starts_with(&format!(
            "api_url in profile staging in {} is invalid",
            config.file().display()
        )),
        "{message}"
    );
}

#[test]
fn set_with_the_profile_flag_creates_the_profile_and_keeps_the_top_level_keys() {
    let config = ConfigDir::new().unwrap();
    config
        .write("api_url = \"https://top.example.com\"\n")
        .unwrap();

    let output = config.run(
        &[
            "config",
            "set",
            "api-url",
            "https://staging.example.com",
            "--profile",
            "staging",
            "--json",
        ],
        &[],
    );

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    insta::assert_snapshot!("set_profile_json", config.stdout(&output));
    assert_eq!(
        config.content(),
        "api_url = \"https://top.example.com\"\n\n\
         [profiles.staging]\napi_url = \"https://staging.example.com\"\n"
    );
}

#[test]
fn set_with_imify_profile_writes_into_the_profile() {
    let config = ConfigDir::new().unwrap();

    config.run(
        &["config", "set", "api-url", "https://staging.example.com"],
        &[("IMIFY_PROFILE", "staging")],
    );

    assert_eq!(
        config.content(),
        "[profiles.staging]\napi_url = \"https://staging.example.com\"\n"
    );
}

#[test]
fn set_and_unset_name_a_named_profile_for_people() {
    let config = ConfigDir::new().unwrap();

    let set = config.run(
        &[
            "config",
            "set",
            "api-url",
            "https://staging.example.com",
            "--profile",
            "staging",
        ],
        &[],
    );
    let get = config.run(&["config", "get", "api-url", "--profile", "staging"], &[]);
    let unset = config.run(&["config", "unset", "api-url", "--profile", "staging"], &[]);

    assert_eq!(
        [
            config.stdout(&set),
            text(&get.stdout),
            config.stdout(&unset)
        ],
        [
            "Set api-url to https://staging.example.com for profile staging in <config_dir>/config.toml.\n",
            "https://staging.example.com (config, profile staging)\n",
            "Removed api-url from profile staging in <config_dir>/config.toml.\n",
        ]
    );
}

#[test]
fn unset_with_a_profile_removes_the_key_and_the_empty_profile_table() {
    let config = ConfigDir::new().unwrap();
    config
        .write(
            "api_url = \"https://top.example.com\"\n\n\
             [profiles.staging]\napi_url = \"https://staging.example.com\"\n",
        )
        .unwrap();

    let output = config.run(
        &[
            "config",
            "unset",
            "api-url",
            "--profile",
            "staging",
            "--json",
        ],
        &[],
    );

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    insta::assert_snapshot!("unset_profile_json", config.stdout(&output));
    assert_eq!(config.content(), "api_url = \"https://top.example.com\"\n");
}

#[test]
fn unset_with_an_unknown_profile_fails_with_invalid_argument() {
    let config = ConfigDir::new().unwrap();
    config.write(PROFILES).unwrap();

    let output = config.run(
        &["config", "unset", "api-url", "--profile", "prod", "--json"],
        &[],
    );

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(error_of(&output).0, "invalid_argument");
    assert_eq!(config.content(), PROFILES);
}

#[test]
fn list_with_a_profile_reports_the_values_of_the_profile() {
    let config = ConfigDir::new().unwrap();
    config.write(PROFILES).unwrap();

    let json = config.run(&["config", "list", "--json", "--profile", "staging"], &[]);
    let human = config.run(&["config", "list", "--profile", "staging"], &[]);

    assert_eq!(
        json_of(&json.stdout),
        json!([{"key": "api-url", "value": "https://staging.example.com", "source": "config", "profile": "staging"}])
    );
    insta::assert_snapshot!("list_profile_human", text(&human.stdout));
}

#[test]
fn get_and_list_report_no_profile_when_the_source_is_not_the_config_file() {
    let config = ConfigDir::new().unwrap();
    config.write(PROFILES).unwrap();
    let env = [("IMIFY_API_URL", "https://env.example.com")];

    let get = config.get_api_url(&[], &env);
    let list = config.run(&["config", "list", "--json"], &env);

    assert_eq!(
        [&get["profile"], &json_of(&list.stdout)[0]["profile"]],
        [&Value::Null, &Value::Null]
    );
}

#[test]
fn profiles_lists_default_first_then_the_others_by_name_and_marks_the_selected_one() {
    let config = ConfigDir::new().unwrap();
    config.write(PROFILES).unwrap();

    let json = config.run(
        &["config", "profiles", "--json", "--profile", "staging"],
        &[],
    );
    let human = config.run(&["config", "profiles"], &[("IMIFY_PROFILE", "staging")]);

    assert_eq!(json.status.code(), Some(0), "{}", text(&json.stderr));
    insta::assert_snapshot!("profiles_json", text(&json.stdout));
    insta::assert_snapshot!("profiles_human", text(&human.stdout));
}

#[test]
fn profiles_without_a_config_file_lists_only_the_default_profile() {
    let config = ConfigDir::new().unwrap();

    let output = config.run(&["config", "profiles", "--json"], &[]);

    assert_eq!(
        json_of(&output.stdout),
        json!([{"name": "default", "active": true, "api_url": "https://platform.imify.ai"}])
    );
}

#[test]
fn profiles_with_an_unknown_selected_profile_marks_none_active_and_warns() {
    let config = ConfigDir::new().unwrap();
    config.write(PROFILES).unwrap();

    let output = config.run(
        &["config", "profiles", "--json"],
        &[("IMIFY_PROFILE", "prod")],
    );

    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let active: Vec<Value> = json_of(&output.stdout)
        .as_array()
        .unwrap()
        .iter()
        .map(|profile| profile["active"].clone())
        .collect();
    assert_eq!(active, vec![Value::Bool(false); 4]);
    assert_eq!(
        text(&output.stderr),
        "warning: The profile 'prod' selected by IMIFY_PROFILE does not exist.\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn two_profiles_with_the_same_host_use_one_stored_login() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"subject": "user-1234", "name": "Ada Lovelace", "role": "admin"}),
        ))
        .expect(2)
        .mount(&server)
        .await;
    let config = ConfigDir::new().unwrap();
    config
        .write(&format!(
            "[profiles.a]\napi_url = \"{0}\"\n\n[profiles.b]\napi_url = \"{0}\"\n",
            server.uri()
        ))
        .unwrap();
    write_login(&config.dir, &server.uri()).unwrap();

    let statuses = ["a", "b"].map(|profile| {
        let output = config.run(&["auth", "whoami", "--json", "--profile", profile], &[]);
        (output.status.code(), text(&output.stderr))
    });

    assert_eq!(
        statuses,
        [(Some(0), String::new()), (Some(0), String::new())]
    );
}

/// Stores a login for `host` in `credentials.json` in `dir`, valid for an hour.
fn write_login(dir: &Path, host: &str) -> io::Result<()> {
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
        + 3600;
    let issuer = "https://id.example.com";
    let file = json!({
        host: {
            "version": 1,
            "issuer": issuer,
            "client_id": "client-1",
            "token_endpoint": format!("{issuer}/oauth2/token"),
            "revocation_endpoint": format!("{issuer}/oauth2/revoke"),
            "id_token": "header.payload.signature",
            "id_token_expires_at": expires_at,
            "refresh_token": null,
        }
    });
    let path = dir.join("credentials.json");
    std::fs::write(&path, file.to_string())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
}
