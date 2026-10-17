use std::path::{Path, PathBuf};

use serde_json::json;
use tempfile::TempDir;

use super::*;

/// A temporary home directory with a Cargo home at `.cargo`.
struct Home {
    dir: TempDir,
}

impl Home {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn cargo_home(&self) -> PathBuf {
        self.path().join(".cargo")
    }

    /// Creates an executable file at `relative` below the home directory.
    fn binary(&self, relative: &str) -> PathBuf {
        let path = self.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "binary").unwrap();
        path
    }

    /// Writes `.crates2.json` in the Cargo home with one install of `package`.
    fn cargo_install(&self, package: &str) {
        let crates = json!({"installs": {package: {"bins": ["imify"], "version_req": null}}});
        std::fs::create_dir_all(self.cargo_home()).unwrap();
        std::fs::write(self.cargo_home().join(".crates2.json"), crates.to_string()).unwrap();
    }

    /// An environment with `HOME` set to this directory and `CARGO_HOME` unset.
    fn env(&self) -> Environment {
        Environment {
            home: Some(self.path().into()),
            ..Environment::default()
        }
    }
}

const REGISTRY: &str = "imify-cli 0.1.0 (registry+https://github.com/rust-lang/crates.io-index)";

#[test]
fn an_executable_in_a_homebrew_cellar_is_a_homebrew_install() {
    let home = Home::new();
    let executable = home.binary("homebrew/Cellar/imify-cli/0.2.0/bin/imify");

    assert_eq!(
        InstallMethod::detect(&executable, &home.env()),
        InstallMethod::Homebrew
    );
}

#[test]
fn a_link_into_a_homebrew_cellar_is_a_homebrew_install() {
    let home = Home::new();
    let target = home.binary("homebrew/Cellar/imify-cli/0.2.0/bin/imify");
    let link = home.path().join("homebrew/bin/imify");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    assert_eq!(
        InstallMethod::detect(&link, &home.env()),
        InstallMethod::Homebrew
    );
}

#[test]
fn a_cellar_of_another_formula_is_not_a_homebrew_install() {
    let home = Home::new();
    let executable = home.binary("homebrew/Cellar/other/0.2.0/bin/imify");

    assert_eq!(
        InstallMethod::detect(&executable, &home.env()),
        InstallMethod::Unknown
    );
}

#[test]
fn cargo_install_from_crates_io_is_detected_in_the_default_cargo_home() {
    let home = Home::new();
    let executable = home.binary(".cargo/bin/imify");
    home.cargo_install(REGISTRY);

    assert_eq!(
        InstallMethod::detect(&executable, &home.env()),
        InstallMethod::CargoRegistry
    );
}

#[test]
fn cargo_install_is_detected_in_the_cargo_home_variable() {
    let home = Home::new();
    let executable = home.binary(".cargo/bin/imify");
    home.cargo_install(REGISTRY);
    let env = Environment {
        home: Some("/nonexistent".into()),
        cargo_home: Some(home.cargo_home().into()),
        ..Environment::default()
    };

    assert_eq!(
        InstallMethod::detect(&executable, &env),
        InstallMethod::CargoRegistry
    );
}

#[test]
fn cargo_install_from_git_keeps_the_repository_url_without_query_and_revision() {
    let home = Home::new();
    let executable = home.binary(".cargo/bin/imify");
    home.cargo_install(
        "imify-cli 0.1.0 (git+https://github.com/imify-ai/imify-cli?branch=dev#9f1c2ab)",
    );

    assert_eq!(
        InstallMethod::detect(&executable, &home.env()),
        InstallMethod::CargoGit {
            url: "https://github.com/imify-ai/imify-cli".to_owned()
        }
    );
}

#[test]
fn a_cargo_home_without_an_imify_cli_install_is_unknown() {
    let home = Home::new();
    let executable = home.binary(".cargo/bin/imify");
    home.cargo_install("ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)");

    assert_eq!(
        InstallMethod::detect(&executable, &home.env()),
        InstallMethod::Unknown
    );
}

#[test]
fn an_executable_outside_the_cargo_home_is_unknown_even_with_a_cargo_install() {
    let home = Home::new();
    let executable = home.binary("bin/imify");
    home.cargo_install(REGISTRY);

    assert_eq!(
        InstallMethod::detect(&executable, &home.env()),
        InstallMethod::Unknown
    );
}

#[test]
fn each_install_method_hints_at_its_update_command() {
    let hints = [
        InstallMethod::Homebrew,
        InstallMethod::CargoRegistry,
        InstallMethod::CargoGit {
            url: "https://github.com/imify-ai/imify-cli".to_owned(),
        },
        InstallMethod::Unknown,
    ]
    .map(|method| method.hint());

    assert_eq!(
        hints,
        [
            "Run `brew upgrade imify-cli`.",
            "Run `cargo install --locked imify-cli`.",
            "Run `cargo install --locked --git https://github.com/imify-ai/imify-cli imify-cli`.",
            "Download it from https://github.com/imify-ai/imify-cli/releases/latest.",
        ]
    );
}
