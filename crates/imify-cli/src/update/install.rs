//! How the running binary was installed, when the imify installer did not install it, and the
//! command that updates such an install.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::output::Environment;

/// The name of the package on crates.io and of the Homebrew formula.
const PACKAGE: &str = "imify-cli";

/// Where to download a release when no update command applies.
const RELEASES_URL: &str = "https://github.com/imify-ai/imify-cli/releases/latest";

/// An install method other than the imify installer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallMethod {
    /// The binary lies in the Homebrew Cellar of the `imify-cli` formula.
    Homebrew,
    /// `cargo install imify-cli` from crates.io.
    CargoRegistry,
    /// `cargo install --git <url> imify-cli`.
    CargoGit { url: String },
    /// Any other install, such as a downloaded archive.
    Unknown,
}

impl InstallMethod {
    /// Detects how `executable` was installed. Homebrew: `executable`, with symbolic links
    /// resolved, lies below `Cellar/imify-cli/`. Cargo: `executable` is `bin/imify` in
    /// `CARGO_HOME` (default `$HOME/.cargo`), and the `.crates2.json` there lists an
    /// `imify-cli` install of the binary `imify`. Unreadable files count as no match.
    pub fn detect(executable: &Path, env: &Environment) -> Self {
        let resolved = std::fs::canonicalize(executable).unwrap_or_else(|_| executable.into());
        if in_homebrew_cellar(&resolved) {
            return Self::Homebrew;
        }
        cargo_install(&resolved, env).unwrap_or(Self::Unknown)
    }

    /// What to do to update this install, for an error hint.
    pub fn hint(&self) -> String {
        match self {
            Self::Homebrew => format!("Run `brew upgrade {PACKAGE}`."),
            Self::CargoRegistry => format!("Run `cargo install --locked {PACKAGE}`."),
            Self::CargoGit { url } => {
                format!("Run `cargo install --locked --git {url} {PACKAGE}`.")
            }
            Self::Unknown => format!("Download it from {RELEASES_URL}."),
        }
    }
}

/// Whether `path` contains the components `Cellar/imify-cli`.
fn in_homebrew_cellar(path: &Path) -> bool {
    let names: Vec<Component<'_>> = path.components().collect();
    names
        .windows(2)
        .any(|pair| pair[0].as_os_str() == "Cellar" && pair[1].as_os_str() == PACKAGE)
}

/// The Cargo install method of `executable`; `None` when Cargo did not install it.
fn cargo_install(executable: &Path, env: &Environment) -> Option<InstallMethod> {
    let cargo_home = cargo_home(env)?;
    let binary = std::fs::canonicalize(cargo_home.join("bin/imify")).ok()?;
    if binary != executable {
        return None;
    }
    let text = std::fs::read(cargo_home.join(".crates2.json")).ok()?;
    let crates: Crates = serde_json::from_slice(&text).ok()?;
    crates
        .installs
        .into_iter()
        .filter(|(_, install)| install.bins.iter().any(|bin| bin == "imify"))
        .find_map(|(id, _)| method_of(&id))
}

/// `CARGO_HOME` when set and not empty, otherwise `$HOME/.cargo`.
fn cargo_home(env: &Environment) -> Option<PathBuf> {
    match env.cargo_home.as_ref().filter(|home| !home.is_empty()) {
        Some(home) => Some(PathBuf::from(home)),
        None => env
            .home
            .as_ref()
            .filter(|home| !home.is_empty())
            .map(|home| Path::new(home).join(".cargo")),
    }
}

/// The install method of a `.crates2.json` key such as
/// `imify-cli 0.1.0 (registry+https://github.com/rust-lang/crates.io-index)`.
fn method_of(id: &str) -> Option<InstallMethod> {
    let (name, rest) = id.split_once(' ')?;
    if name != PACKAGE {
        return None;
    }
    let source = rest.split_once('(')?.1.strip_suffix(')')?;
    if source.starts_with("registry+") || source.starts_with("sparse+") {
        return Some(InstallMethod::CargoRegistry);
    }
    let url = source.strip_prefix("git+")?;
    let end = url.find(['?', '#']).unwrap_or(url.len());
    Some(InstallMethod::CargoGit {
        url: url[..end].to_owned(),
    })
}

/// The installs Cargo records in `.crates2.json`; other fields are ignored.
#[derive(Deserialize)]
struct Crates {
    installs: BTreeMap<String, CrateInstall>,
}

#[derive(Deserialize)]
struct CrateInstall {
    bins: Vec<String>,
}

#[cfg(test)]
mod tests;
