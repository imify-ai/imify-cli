//! The update hint of the `imify` binary with stderr on a pseudo-terminal and a release
//! server on wiremock.
#![cfg(unix)]

use std::fs::File;
use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::process::{Command, ExitStatus, Stdio};

use rustix::fs::{Mode, OFlags};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LATEST: &str = "/api/v3/repos/imify-ai/imify-cli/releases/latest";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A home directory and a release server whose latest release is 99.0.0.
struct Machine {
    home: TempDir,
    server: MockServer,
}

impl Machine {
    async fn new() -> io::Result<Self> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(LATEST))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"tag_name": "v99.0.0"})))
            .mount(&server)
            .await;
        Ok(Self {
            home: tempfile::tempdir()?,
            server,
        })
    }

    /// Runs `imify` with `args` and stderr on a pseudo-terminal; returns the exit status and
    /// the text written to stderr.
    fn run_on_terminal(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> io::Result<(ExitStatus, String)> {
        let (emulator, terminal) = pty()?;
        let reader = std::thread::spawn(move || {
            let mut text = Vec::new();
            // Reading the emulator side fails with EIO once every terminal descriptor is closed.
            let _ = File::from(emulator).read_to_end(&mut text);
            String::from_utf8_lossy(&text).replace("\r\n", "\n")
        });
        let mut command = Command::new(env!("CARGO_BIN_EXE_imify"));
        for variable in [
            "IMIFY_OUTPUT",
            "IMIFY_PROFILE",
            "IMIFY_API_URL",
            "IMIFY_NO_UPDATE_CHECK",
            "CI",
            "BUILD_NUMBER",
            "RUN_ID",
            "XDG_CONFIG_HOME",
            "XDG_STATE_HOME",
            "CARGO_HOME",
        ] {
            command.env_remove(variable);
        }
        command
            .env("HOME", self.home.path())
            .env("IMIFY_CONFIG_DIR", self.home.path().join("config"))
            .env("IMIFY_CLI_INSTALLER_GHE_BASE_URL", self.server.uri())
            .env("NO_COLOR", "1")
            .envs(env.iter().copied())
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(terminal));
        let status = command.status()?;
        drop(command);
        let stderr = reader
            .join()
            .map_err(|_| io::Error::other("the terminal reader panicked"))?;
        Ok((status, stderr))
    }

    async fn requests(&self) -> usize {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .len()
    }
}

/// A pseudo-terminal: the emulator side and a descriptor of the terminal side.
fn pty() -> io::Result<(OwnedFd, OwnedFd)> {
    let emulator = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
    grantpt(&emulator)?;
    unlockpt(&emulator)?;
    let name = ptsname(&emulator, Vec::new())?;
    let terminal = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    Ok((emulator, terminal))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_successful_command_on_a_terminal_ends_with_the_update_hint() {
    let machine = Machine::new().await.unwrap();

    let (status, stderr) = machine.run_on_terminal(&["config", "list"], &[]).unwrap();

    assert_eq!(
        (status.code(), stderr),
        (
            Some(0),
            format!(
                "imify 99.0.0 is available (installed: {VERSION}). Download it from \
                 https://github.com/imify-ai/imify-cli/releases/latest.\n"
            )
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_command_within_24_hours_sends_no_request() {
    let machine = Machine::new().await.unwrap();
    machine.run_on_terminal(&["config", "list"], &[]).unwrap();

    let (_, stderr) = machine.run_on_terminal(&["config", "list"], &[]).unwrap();

    assert_eq!((stderr.as_str(), machine.requests().await), ("", 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_command_shows_no_hint() {
    let machine = Machine::new().await.unwrap();

    let (status, stderr) = machine
        .run_on_terminal(&["config", "list"], &[("IMIFY_PROFILE", "missing")])
        .unwrap();

    assert_eq!(
        (status.code(), stderr.contains("is available")),
        (Some(2), false)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn imify_no_update_check_sends_no_request() {
    let machine = Machine::new().await.unwrap();

    let (_, stderr) = machine
        .run_on_terminal(&["config", "list"], &[("IMIFY_NO_UPDATE_CHECK", "1")])
        .unwrap();

    assert_eq!((stderr.as_str(), machine.requests().await), ("", 0));
}

#[tokio::test(flavor = "multi_thread")]
async fn stderr_not_on_a_terminal_sends_no_request() {
    let machine = Machine::new().await.unwrap();

    let output = assert_cmd::Command::new(env!("CARGO_BIN_EXE_imify"))
        .env_remove("IMIFY_NO_UPDATE_CHECK")
        .env_remove("CI")
        .env_remove("BUILD_NUMBER")
        .env_remove("RUN_ID")
        .env_remove("IMIFY_OUTPUT")
        .env("HOME", machine.home.path())
        .env("IMIFY_CONFIG_DIR", machine.home.path().join("config"))
        .env("IMIFY_CLI_INSTALLER_GHE_BASE_URL", machine.server.uri())
        .args(["config", "list"])
        .output()
        .unwrap();

    assert_eq!(
        (output.status.code(), machine.requests().await),
        (Some(0), 0)
    );
}
