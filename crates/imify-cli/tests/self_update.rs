//! `imify self update` with a hard link to the binary in a temporary install prefix, a fake install
//! receipt, a release server on wiremock and a fake installer script.
#![cfg(unix)]

use std::io;
use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LATEST: &str = "/api/v3/repos/imify-ai/imify-cli/releases/latest";
const NEWER_TAG: &str = "v99.0.0";
const INSTALLER: &str = "/imify-ai/imify-cli/releases/download/v99.0.0/imify-cli-installer.sh";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A home directory with the configuration in `config`, a Cargo home in `cargo`, and a
/// release server.
struct Machine {
    home: TempDir,
    server: MockServer,
}

impl Machine {
    async fn new() -> io::Result<Self> {
        Ok(Self {
            home: tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR"))?,
            server: MockServer::start().await,
        })
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.home.path().join(relative)
    }

    /// Hard-links the `imify` binary to `relative` below the home directory.
    ///
    /// A copy is open for writing until `copy` returns. A test thread that forks in that
    /// window keeps the file open, and running the copy then fails with `ETXTBSY` on Linux.
    /// A hard link opens no file. The home directory is below the target directory so that
    /// both are on one file system.
    fn install_binary(&self, relative: &str) -> io::Result<PathBuf> {
        let target = self.path(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::hard_link(env!("CARGO_BIN_EXE_imify"), &target)?;
        Ok(target)
    }

    /// Copies the binary to `prefix/bin/imify` and writes the receipt of an installer install
    /// into `prefix`, as the 0.1.0 installer writes it.
    fn installer_install(&self) -> io::Result<PathBuf> {
        let executable = self.install_binary("prefix/bin/imify")?;
        let receipt = json!({
            "binaries": ["imify"],
            "binary_aliases": {},
            "cdylibs": [],
            "cstaticlibs": [],
            "install_layout": "cargo-home",
            "install_prefix": self.path("prefix"),
            "modify_path": false,
            "provider": {"source": "cargo-dist", "version": "0.33.0"},
            "source": {
                "app_name": "imify-cli",
                "name": "imify-cli",
                "owner": "imify-ai",
                "release_type": "github"
            },
            "version": "0.1.0"
        });
        let dir = self.path("config/imify-cli");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("imify-cli-receipt.json"), receipt.to_string())?;
        Ok(executable)
    }

    async fn latest_release(&self, response: ResponseTemplate) {
        Mock::given(method("GET"))
            .and(path(LATEST))
            .respond_with(response)
            .mount(&self.server)
            .await;
    }

    async fn latest_tag(&self, tag: &str) {
        self.latest_release(ResponseTemplate::new(200).set_body_json(json!({"tag_name": tag})))
            .await;
    }

    /// Serves an installer that writes its environment to `installer-env` and exits with
    /// `status`.
    async fn installer(&self, status: i32) {
        let script = format!(
            "#!/bin/sh\nenv > '{}'\necho 'installing imify-cli'\nexit {status}\n",
            self.path("installer-env").display()
        );
        Mock::given(method("GET"))
            .and(path(INSTALLER))
            .respond_with(ResponseTemplate::new(200).set_body_string(script))
            .mount(&self.server)
            .await;
    }

    fn installer_ran(&self) -> bool {
        self.path("installer-env").exists()
    }

    fn installer_env(&self) -> String {
        std::fs::read_to_string(self.path("installer-env")).unwrap_or_default()
    }

    /// Runs `executable` with this home, its configuration and Cargo home, and the release
    /// server as GitHub Enterprise Server.
    fn run(&self, executable: &Path, args: &[&str]) -> Output {
        Command::new(executable)
            .env_remove("IMIFY_OUTPUT")
            .env_remove("NO_COLOR")
            .env_remove("IMIFY_CONFIG_DIR")
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("CARGO_HOME", self.path("cargo"))
            .env("IMIFY_CLI_INSTALLER_GHE_BASE_URL", self.server.uri())
            .args(args)
            .assert()
            .get_output()
            .clone()
    }

    /// `text` with the home directory replaced by `<home>` and the running version by
    /// `<version>`.
    fn redact(&self, text: &[u8]) -> String {
        let home =
            std::fs::canonicalize(self.home.path()).unwrap_or_else(|_| self.home.path().to_owned());
        String::from_utf8_lossy(text)
            .replace(&home.display().to_string(), "<home>")
            .replace(&self.home.path().display().to_string(), "<home>")
            .replace(VERSION, "<version>")
    }

    async fn requests(&self) -> usize {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .len()
    }
}

fn json_of(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn update_to_a_newer_release_runs_its_installer_for_the_receipts_prefix() {
    let machine = Machine::new().await.unwrap();
    let imify = machine.installer_install().unwrap();
    machine.latest_tag(NEWER_TAG).await;
    machine.installer(0).await;

    let output = machine.run(&imify, &["self", "update"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        machine.redact(&output.stderr)
    );
    insta::assert_snapshot!("self_update_human", machine.redact(&output.stdout));
    let env = machine.installer_env();
    assert!(env.contains(&format!(
        "IMIFY_CLI_INSTALL_DIR={}",
        machine.path("prefix").display()
    )));
    assert!(env.contains("IMIFY_CLI_NO_MODIFY_PATH=1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn update_to_a_newer_release_reports_both_versions_as_json() {
    let machine = Machine::new().await.unwrap();
    let imify = machine.installer_install().unwrap();
    machine.latest_tag(NEWER_TAG).await;
    machine.installer(0).await;

    let output = machine.run(&imify, &["self", "update", "--json"]);

    assert_eq!(output.status.code(), Some(0));
    insta::assert_snapshot!("self_update_json", machine.redact(&output.stdout));
}

#[tokio::test(flavor = "multi_thread")]
async fn update_on_the_latest_release_downloads_no_installer() {
    let machine = Machine::new().await.unwrap();
    let imify = machine.installer_install().unwrap();
    machine.latest_tag(&format!("v{VERSION}")).await;
    machine.installer(0).await;

    let output = machine.run(&imify, &["self", "update", "--json"]);

    assert_eq!(output.status.code(), Some(0));
    insta::assert_snapshot!("self_update_latest_json", machine.redact(&output.stdout));
    assert_eq!(machine.requests().await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn update_on_the_latest_release_says_so() {
    let machine = Machine::new().await.unwrap();
    let imify = machine.installer_install().unwrap();
    machine.latest_tag(&format!("v{VERSION}")).await;

    let output = machine.run(&imify, &["self", "update"]);

    assert_eq!(
        machine.redact(&output.stdout),
        "imify <version> is the latest release.\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn update_without_a_receipt_is_unsupported_and_sends_no_request() {
    let machine = Machine::new().await.unwrap();
    let imify = machine.install_binary("bin/imify").unwrap();
    machine.latest_tag(NEWER_TAG).await;

    let output = machine.run(&imify, &["self", "update", "--json"]);

    assert_eq!(output.status.code(), Some(2));
    insta::assert_snapshot!(
        "self_update_unsupported_json",
        machine.redact(&output.stderr)
    );
    assert_eq!(machine.requests().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn update_of_a_cargo_install_hints_at_cargo_install() {
    let machine = Machine::new().await.unwrap();
    let imify = machine.install_binary("cargo/bin/imify").unwrap();
    let crates = json!({"installs": {
        "imify-cli 0.1.0 (registry+https://github.com/rust-lang/crates.io-index)": {"bins": ["imify"]}
    }});
    std::fs::write(machine.path("cargo/.crates2.json"), crates.to_string()).unwrap();

    let output = machine.run(&imify, &["self", "update", "--json"]);

    assert_eq!(
        (
            output.status.code(),
            json_of(&output.stderr)["error"]["hint"].clone()
        ),
        (Some(2), json!("Run `cargo install --locked imify-cli`."))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn update_of_a_homebrew_install_hints_at_brew_upgrade() {
    let machine = Machine::new().await.unwrap();
    let imify = machine
        .install_binary("homebrew/Cellar/imify-cli/0.2.0/bin/imify")
        .unwrap();

    let output = machine.run(&imify, &["self", "update", "--json"]);

    assert_eq!(
        (
            output.status.code(),
            json_of(&output.stderr)["error"]["code"].clone(),
            json_of(&output.stderr)["error"]["hint"].clone()
        ),
        (
            Some(2),
            json!("self_update_unsupported"),
            json!("Run `brew upgrade imify-cli`.")
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn update_with_a_failing_installer_is_self_update_failed() {
    let machine = Machine::new().await.unwrap();
    let imify = machine.installer_install().unwrap();
    machine.latest_tag(NEWER_TAG).await;
    machine.installer(1).await;

    let output = machine.run(&imify, &["self", "update", "--json"]);

    assert_eq!(output.status.code(), Some(1));
    insta::assert_snapshot!("self_update_failed_json", machine.redact(&output.stderr));
}

#[tokio::test(flavor = "multi_thread")]
async fn update_with_a_failing_installer_logs_its_output_with_verbose() {
    let machine = Machine::new().await.unwrap();
    let imify = machine.installer_install().unwrap();
    machine.latest_tag(NEWER_TAG).await;
    machine.installer(1).await;

    let output = machine.run(&imify, &["self", "update", "-v"]);

    assert!(
        machine
            .redact(&output.stderr)
            .contains("installing imify-cli"),
        "{}",
        machine.redact(&output.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn update_against_a_rate_limited_lookup_is_rate_limited_and_runs_no_installer() {
    let machine = Machine::new().await.unwrap();
    let imify = machine.installer_install().unwrap();
    machine
        .latest_release(
            ResponseTemplate::new(403).set_body_json(json!({"message": "API rate limit exceeded"})),
        )
        .await;
    machine.installer(0).await;

    let output = machine.run(&imify, &["self", "update", "--json"]);

    assert_eq!(
        (
            output.status.code(),
            json_of(&output.stderr)["error"]["code"].clone()
        ),
        (Some(7), json!("rate_limited"))
    );
    assert!(!machine.installer_ran());
}
