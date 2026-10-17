use std::cell::RefCell;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::cli::{ConfigCommand, GlobalArgs, SelfCommand};
use crate::output::UPDATE_CHECK_OPT_OUT_VARIABLES;
use crate::output::renderer::HumanRenderer;

const LATEST: &str = "/api/v3/repos/imify-ai/imify-cli/releases/latest";
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

// Which invocations check.

/// An environment with stderr on a terminal and no opt-out variable.
fn terminal() -> Environment {
    Environment {
        stderr_is_tty: true,
        ..Environment::default()
    }
}

fn config_list() -> Command {
    Command::Config {
        command: ConfigCommand::List,
    }
}

fn applies_with(global: &GlobalArgs, env: &Environment, command: Option<&Command>) -> bool {
    let settings = Settings::resolve(global, env).unwrap();
    applies(command, &settings, env)
}

#[test]
fn a_command_with_human_output_on_a_terminal_checks() {
    assert!(applies_with(
        &GlobalArgs::default(),
        &terminal(),
        Some(&config_list())
    ));
}

#[test]
fn json_output_does_not_check() {
    let global = GlobalArgs {
        json: true,
        ..GlobalArgs::default()
    };

    assert!(!applies_with(&global, &terminal(), Some(&config_list())));
}

#[test]
fn imify_output_json_does_not_check() {
    let env = Environment {
        imify_output: Some("json".to_owned()),
        ..terminal()
    };

    assert!(!applies_with(
        &GlobalArgs::default(),
        &env,
        Some(&config_list())
    ));
}

#[test]
fn quiet_does_not_check() {
    let global = GlobalArgs {
        quiet: true,
        ..GlobalArgs::default()
    };

    assert!(!applies_with(&global, &terminal(), Some(&config_list())));
}

#[test]
fn stderr_not_on_a_terminal_does_not_check() {
    let env = Environment {
        stderr_is_tty: false,
        ..terminal()
    };

    assert!(!applies_with(
        &GlobalArgs::default(),
        &env,
        Some(&config_list())
    ));
}

#[test]
fn each_opt_out_variable_with_a_value_turns_the_check_off() {
    let checks = std::array::from_fn::<bool, 4, _>(|index| {
        let mut env = terminal();
        env.update_check_opt_outs[index] = Some("1".into());
        applies_with(&GlobalArgs::default(), &env, Some(&config_list()))
    });

    assert_eq!(
        (UPDATE_CHECK_OPT_OUT_VARIABLES, checks),
        (
            ["IMIFY_NO_UPDATE_CHECK", "CI", "BUILD_NUMBER", "RUN_ID"],
            [false; 4]
        )
    );
}

#[test]
fn empty_opt_out_variables_keep_the_check_on() {
    let env = Environment {
        update_check_opt_outs: std::array::from_fn(|_| Some("".into())),
        ..terminal()
    };

    assert!(applies_with(
        &GlobalArgs::default(),
        &env,
        Some(&config_list())
    ));
}

#[test]
fn self_update_does_not_check() {
    let command = Command::Itself {
        command: SelfCommand::Update,
    };

    assert!(!applies_with(
        &GlobalArgs::default(),
        &terminal(),
        Some(&command)
    ));
}

#[test]
fn doctor_does_not_check() {
    assert!(!applies_with(
        &GlobalArgs::default(),
        &terminal(),
        Some(&Command::Doctor)
    ));
}

#[test]
fn bare_imify_does_not_check() {
    assert!(!applies_with(&GlobalArgs::default(), &terminal(), None));
}

// The state file.

fn state_dir() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("imify/update-check.json");
    (dir, file)
}

#[test]
fn the_first_check_is_due_and_records_the_time() {
    let (_dir, file) = state_dir();

    let due = claim(&file, at(1_000_000)).unwrap();

    assert_eq!(
        (due, std::fs::read_to_string(&file).unwrap()),
        (true, r#"{"checked_at":1000000}"#.to_owned())
    );
}

#[test]
fn the_state_file_is_readable_only_by_the_user() {
    let (_dir, file) = state_dir();

    claim(&file, at(1_000_000)).unwrap();

    let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn a_check_within_24_hours_of_the_last_is_not_due_and_keeps_the_time() {
    let (_dir, file) = state_dir();
    claim(&file, at(1_000_000)).unwrap();

    let due = claim(&file, at(1_000_000) + DAY - Duration::from_secs(1)).unwrap();

    assert_eq!(
        (due, std::fs::read_to_string(&file).unwrap()),
        (false, r#"{"checked_at":1000000}"#.to_owned())
    );
}

#[test]
fn a_check_24_hours_after_the_last_is_due() {
    let (_dir, file) = state_dir();
    claim(&file, at(1_000_000)).unwrap();

    assert!(claim(&file, at(1_000_000) + DAY).unwrap());
}

#[test]
fn a_last_check_later_than_now_is_due() {
    let (_dir, file) = state_dir();
    claim(&file, at(2_000_000)).unwrap();

    assert!(claim(&file, at(1_000_000)).unwrap());
}

#[test]
fn an_unreadable_state_file_is_due_and_replaced() {
    let (_dir, file) = state_dir();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "not json").unwrap();

    let due = claim(&file, at(1_000_000)).unwrap();

    assert_eq!(
        (due, std::fs::read_to_string(&file).unwrap()),
        (true, r#"{"checked_at":1000000}"#.to_owned())
    );
}

// The check against the release server.

/// A home directory, its state and configuration directories, and a release server.
struct Machine {
    home: TempDir,
    server: MockServer,
}

impl Machine {
    async fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            server: MockServer::start().await,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.home.path().join(relative)
    }

    fn env(&self) -> Environment {
        Environment {
            home: Some(self.home.path().into()),
            installer_ghe_base_url: Some(self.server.uri()),
            ..terminal()
        }
    }

    async fn latest_tag(&self, tag: &str) {
        Mock::given(method("GET"))
            .and(path(LATEST))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"tag_name": tag})))
            .mount(&self.server)
            .await;
    }

    async fn requests(&self) -> usize {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .len()
    }

    /// An executable at `bin/imify` that no install method claims.
    fn executable(&self) -> PathBuf {
        let executable = self.path("bin/imify");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, "binary").unwrap();
        executable
    }

    /// An executable at `prefix/bin/imify` with the installer's receipt for it.
    fn installer_install(&self) -> PathBuf {
        let executable = self.path("prefix/bin/imify");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, "binary").unwrap();
        let receipt = json!({
            "binaries": ["imify"],
            "install_layout": "cargo-home",
            "install_prefix": self.path("prefix"),
            "modify_path": false,
            "source": {
                "app_name": "imify-cli",
                "name": "imify-cli",
                "owner": "imify-ai",
                "release_type": "github"
            },
            "version": "0.1.0"
        });
        let dir = self.path(".config/imify-cli");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("imify-cli-receipt.json"), receipt.to_string()).unwrap();
        executable
    }

    async fn hint(&self, executable: &Path, now: SystemTime) -> Result<Option<String>, CliError> {
        hint(&self.env(), executable, &version("0.1.0"), now).await
    }
}

#[tokio::test]
async fn a_newer_release_gives_a_hint_naming_both_versions_and_the_update_command() {
    let machine = Machine::new().await;
    machine.latest_tag("v0.2.0").await;

    let hint = machine.hint(&machine.executable(), at(1_000_000)).await;

    assert_eq!(
        hint.unwrap().as_deref(),
        Some(
            "imify 0.2.0 is available (installed: 0.1.0). Download it from \
             https://github.com/imify-ai/imify-cli/releases/latest."
        )
    );
}

#[tokio::test]
async fn an_installer_install_is_told_to_run_imify_self_update() {
    let machine = Machine::new().await;
    machine.latest_tag("v0.2.0").await;

    let hint = machine
        .hint(&machine.installer_install(), at(1_000_000))
        .await;

    assert_eq!(
        hint.unwrap().as_deref(),
        Some("imify 0.2.0 is available (installed: 0.1.0). Run `imify self update`.")
    );
}

#[tokio::test]
async fn the_installed_release_gives_no_hint() {
    let machine = Machine::new().await;
    machine.latest_tag("v0.1.0").await;

    let hint = machine.hint(&machine.executable(), at(1_000_000)).await;

    assert_eq!(hint.unwrap(), None);
}

#[tokio::test]
async fn a_check_within_24_hours_of_the_last_sends_no_request() {
    let machine = Machine::new().await;
    machine.latest_tag("v0.2.0").await;
    machine
        .hint(&machine.executable(), at(1_000_000))
        .await
        .unwrap();

    let hint = machine
        .hint(&machine.executable(), at(1_000_000) + DAY / 2)
        .await;

    assert_eq!((hint.unwrap(), machine.requests().await), (None, 1));
}

#[tokio::test]
async fn the_time_is_recorded_before_the_request() {
    let machine = Machine::new().await;
    Mock::given(method("GET"))
        .and(path(LATEST))
        .respond_with(ResponseTemplate::new(500))
        .mount(&machine.server)
        .await;

    let hint = machine.hint(&machine.executable(), at(1_000_000)).await;

    assert_eq!(
        (
            hint.unwrap_err().code,
            std::fs::read_to_string(machine.path(".local/state/imify/update-check.json")).unwrap()
        ),
        (ErrorCode::Server, r#"{"checked_at":1000000}"#.to_owned())
    );
}

#[tokio::test]
async fn a_state_file_that_cannot_be_written_sends_no_request() {
    let machine = Machine::new().await;
    machine.latest_tag("v0.2.0").await;
    std::fs::write(machine.path(".local"), "not a directory").unwrap();

    let hint = machine.hint(&machine.executable(), at(1_000_000)).await;

    assert_eq!((hint.is_err(), machine.requests().await), (true, 0));
}

#[tokio::test]
async fn the_lookup_honors_the_ghe_base_url_variable() {
    let machine = Machine::new().await;
    machine.latest_tag("v0.2.0").await;

    machine
        .hint(&machine.executable(), at(1_000_000))
        .await
        .unwrap();

    let requests = machine.server.received_requests().await.unwrap();
    assert_eq!(requests[0].url.path(), LATEST);
}

// Waiting for the check.

#[derive(Clone, Default)]
struct Buffer(Rc<RefCell<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The text `pending` writes to stderr when it finishes, and how long finishing took.
fn finish(pending: PendingCheck) -> (String, Duration) {
    let stderr = Buffer::default();
    let mut renderer = HumanRenderer::new(Box::new(io::sink()), Box::new(stderr.clone()), false);
    let started = Instant::now();
    pending.finish(&mut renderer);
    let elapsed = started.elapsed();
    let text = String::from_utf8(stderr.0.borrow().clone()).unwrap();
    (text, elapsed)
}

#[test]
fn a_hint_ready_before_the_deadline_is_written_to_stderr() {
    let pending = PendingCheck::spawn(
        || Some("imify 0.2.0 is available.".to_owned()),
        Duration::from_secs(5),
    )
    .unwrap();

    assert_eq!(finish(pending).0, "imify 0.2.0 is available.\n");
}

#[test]
fn a_check_without_a_hint_writes_nothing_and_does_not_wait_for_the_deadline() {
    let pending = PendingCheck::spawn(|| None, Duration::from_secs(5)).unwrap();

    let (text, elapsed) = finish(pending);

    assert_eq!(
        (text.as_str(), elapsed < Duration::from_secs(4)),
        ("", true)
    );
}

#[test]
fn a_check_slower_than_the_deadline_is_dropped_at_the_deadline() {
    let pending = PendingCheck::spawn(
        || {
            std::thread::sleep(Duration::from_secs(5));
            Some("imify 0.2.0 is available.".to_owned())
        },
        Duration::from_millis(50),
    )
    .unwrap();

    let (text, elapsed) = finish(pending);

    assert_eq!(
        (text.as_str(), elapsed < Duration::from_secs(4)),
        ("", true)
    );
}

#[test]
fn the_deadline_counts_from_the_start_of_the_check() {
    let pending = PendingCheck::spawn(
        || {
            std::thread::sleep(Duration::from_secs(5));
            None
        },
        Duration::from_millis(50),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(100));

    let (_, elapsed) = finish(pending);

    assert!(elapsed < Duration::from_millis(40), "{elapsed:?}");
}
