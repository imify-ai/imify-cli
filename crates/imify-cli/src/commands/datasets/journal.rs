//! The directory of the upload journals.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::output::{CliError, Environment, ErrorCode};

/// The directory below the platform state directory that holds the upload journals.
const UPLOADS: &str = "imify/uploads";

/// The upload journal directory of this platform: `~/Library/Application Support/imify/uploads`
/// on macOS, `$XDG_STATE_HOME/imify/uploads` or `~/.local/state/imify/uploads` elsewhere.
///
/// # Errors
///
/// Returns `invalid_argument` when the directory depends on `HOME` and `HOME` is unset, empty or
/// not an absolute path.
pub fn journal_dir(env: &Environment) -> Result<PathBuf, CliError> {
    let dir = if cfg!(target_os = "macos") {
        macos_journal_dir(env.home.as_deref())
    } else {
        xdg_journal_dir(env.state_home.as_deref(), env.home.as_deref())
    };
    dir.ok_or_else(|| {
        CliError::new(
            ErrorCode::InvalidArgument,
            "Cannot locate the upload journal directory: HOME is not set to an absolute path.",
        )
        .with_hint("Set `HOME` to your home directory.")
    })
}

/// `$HOME/Library/Application Support/imify/uploads`.
fn macos_journal_dir(home: Option<&OsStr>) -> Option<PathBuf> {
    Some(
        absolute(home)?
            .join("Library/Application Support")
            .join(UPLOADS),
    )
}

/// `$XDG_STATE_HOME/imify/uploads`, or `$HOME/.local/state/imify/uploads` when `XDG_STATE_HOME`
/// is unset, empty or relative.
fn xdg_journal_dir(state_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let state_home = match absolute(state_home) {
        Some(state_home) => state_home.to_owned(),
        None => absolute(home)?.join(".local/state"),
    };
    Some(state_home.join(UPLOADS))
}

/// `value` as a path when it is an absolute path.
fn absolute(value: Option<&OsStr>) -> Option<&Path> {
    value.map(Path::new).filter(|path| path.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_journal_dir_is_below_xdg_state_home() {
        assert_eq!(
            xdg_journal_dir(Some(OsStr::new("/state")), Some(OsStr::new("/home/robot"))),
            Some(PathBuf::from("/state/imify/uploads"))
        );
    }

    #[test]
    fn xdg_journal_dir_falls_back_to_local_state_in_home_when_xdg_state_home_is_unset_empty_or_relative()
     {
        let dirs = [None, Some(OsStr::new("")), Some(OsStr::new("state"))]
            .map(|state_home| xdg_journal_dir(state_home, Some(OsStr::new("/home/robot"))));

        let expected = Some(PathBuf::from("/home/robot/.local/state/imify/uploads"));
        assert_eq!(dirs, [expected.clone(), expected.clone(), expected]);
    }

    #[test]
    fn xdg_journal_dir_without_xdg_state_home_and_an_absolute_home_is_none() {
        let dirs = [None, Some(OsStr::new("")), Some(OsStr::new("home"))]
            .map(|home| xdg_journal_dir(None, home));

        assert_eq!(dirs, [None, None, None]);
    }

    #[test]
    fn macos_journal_dir_is_in_application_support() {
        assert_eq!(
            macos_journal_dir(Some(OsStr::new("/Users/robot"))),
            Some(PathBuf::from(
                "/Users/robot/Library/Application Support/imify/uploads"
            ))
        );
    }

    #[test]
    fn macos_journal_dir_without_an_absolute_home_is_none() {
        let dirs = [None, Some(OsStr::new("")), Some(OsStr::new("robot"))].map(macos_journal_dir);

        assert_eq!(dirs, [None, None, None]);
    }

    #[test]
    fn journal_dir_without_home_is_an_invalid_argument_naming_home() {
        let error = journal_dir(&Environment::default()).unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert_eq!(
            error.hint.as_deref(),
            Some("Set `HOME` to your home directory.")
        );
    }

    #[test]
    fn journal_dir_is_below_home_on_this_platform() {
        let env = Environment {
            home: Some("/home/robot".into()),
            ..Environment::default()
        };

        let dir = journal_dir(&env).unwrap();

        assert!(dir.starts_with("/home/robot"), "{}", dir.display());
        assert!(dir.ends_with("imify/uploads"), "{}", dir.display());
    }
}
