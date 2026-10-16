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
        json!({"key": "api-url", "value": "https://api.example.com", "source": "config"})
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

    for verb in ["get", "set", "unset", "list"] {
        let output = config.run(&["config", verb, "--help"], &[]);
        assert!(
            text(&output.stdout).contains(&format!("imify config {verb}")),
            "{verb}"
        );
    }
}
