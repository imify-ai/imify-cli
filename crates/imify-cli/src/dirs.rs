//! The directories of imify below the user's home: configuration, upload journals, the
//! update check's state file and the installer's receipt directory. All follow the XDG base directory layout on every platform.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::output::{CliError, Environment, ErrorCode};

/// The variable that sets the configuration directory.
pub const CONFIG_DIR_VARIABLE: &str = "IMIFY_CONFIG_DIR";

/// The directory of `config.toml` and `credentials.json`: `IMIFY_CONFIG_DIR`, otherwise
/// `$XDG_CONFIG_HOME/imify`, otherwise `$HOME/.config/imify`.
///
/// An empty `IMIFY_CONFIG_DIR` counts as unset. An `XDG_CONFIG_HOME` that is empty or not an
/// absolute path is ignored.
///
/// # Errors
///
/// Returns `invalid_argument` when the directory depends on `HOME` and `HOME` is unset, empty
/// or not an absolute path.
pub fn config_dir(env: &Environment) -> Result<PathBuf, CliError> {
    if let Some(dir) = env
        .config_dir
        .as_ref()
        .filter(|dir| !dir.as_os_str().is_empty())
    {
        return Ok(dir.clone());
    }
    base_dir(env.config_home.as_deref(), env.home.as_deref(), ".config")
        .map(|base| base.join("imify"))
        .ok_or_else(|| {
            unknown_home("configuration directory").with_hint(format!(
                "Set `HOME` to your home directory, or `{CONFIG_DIR_VARIABLE}` to a directory."
            ))
        })
}

/// The directory of the upload journals: `$XDG_STATE_HOME/imify/uploads`, otherwise
/// `$HOME/.local/state/imify/uploads`. `IMIFY_CONFIG_DIR` does not move it.
///
/// An `XDG_STATE_HOME` that is empty or not an absolute path is ignored.
///
/// # Errors
///
/// Returns `invalid_argument` when the directory depends on `HOME` and `HOME` is unset, empty
/// or not an absolute path.
pub fn journal_dir(env: &Environment) -> Result<PathBuf, CliError> {
    base_dir(
        env.state_home.as_deref(),
        env.home.as_deref(),
        ".local/state",
    )
    .map(|base| base.join("imify/uploads"))
    .ok_or_else(|| {
        unknown_home("upload journal directory").with_hint("Set `HOME` to your home directory.")
    })
}

/// The file that records the time of the last update check:
/// `$XDG_STATE_HOME/imify/update-check.json`, otherwise
/// `$HOME/.local/state/imify/update-check.json`. `None` when neither `XDG_STATE_HOME` nor
/// `HOME` is an absolute path.
pub fn update_check_file(env: &Environment) -> Option<PathBuf> {
    base_dir(
        env.state_home.as_deref(),
        env.home.as_deref(),
        ".local/state",
    )
    .map(|base| base.join("imify/update-check.json"))
}

/// The user's configuration base directory, where the imify installer writes its install
/// receipt: `$XDG_CONFIG_HOME`, otherwise `$HOME/.config`. `IMIFY_CONFIG_DIR` does not move it.
/// `None` when neither `XDG_CONFIG_HOME` nor `HOME` is an absolute path.
pub fn config_home(env: &Environment) -> Option<PathBuf> {
    base_dir(env.config_home.as_deref(), env.home.as_deref(), ".config")
}

/// The XDG base directory `xdg_home` when it is an absolute path, otherwise `in_home` below
/// `home`; `None` when neither is an absolute path.
fn base_dir(xdg_home: Option<&OsStr>, home: Option<&OsStr>, in_home: &str) -> Option<PathBuf> {
    match absolute(xdg_home) {
        Some(base) => Some(base.to_path_buf()),
        None => Some(absolute(home)?.join(in_home)),
    }
}

/// `value` as a path when it is an absolute path.
fn absolute(value: Option<&OsStr>) -> Option<&Path> {
    value.map(Path::new).filter(|path| path.is_absolute())
}

/// The error for a directory below `HOME` when `HOME` is not an absolute path.
fn unknown_home(directory: &str) -> CliError {
    CliError::new(
        ErrorCode::InvalidArgument,
        format!("Cannot locate the {directory}: HOME is not set to an absolute path."),
    )
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    /// An environment with `HOME` set to `/home/robot`.
    fn home_robot() -> Environment {
        Environment {
            home: Some("/home/robot".into()),
            ..Environment::default()
        }
    }

    /// `HOME` values that do not name a directory: unset, empty and relative.
    fn unusable_homes() -> [Option<OsString>; 3] {
        [None, Some("".into()), Some("robot".into())]
    }

    #[test]
    fn config_dir_is_imify_config_dir_when_set() {
        let env = Environment {
            config_dir: Some("/etc/imify".into()),
            config_home: Some("/config".into()),
            ..home_robot()
        };

        assert_eq!(config_dir(&env).unwrap(), PathBuf::from("/etc/imify"));
    }

    #[test]
    fn config_dir_is_below_xdg_config_home_when_imify_config_dir_is_unset_or_empty() {
        let dirs = [None, Some(PathBuf::new())].map(|dir| {
            config_dir(&Environment {
                config_dir: dir,
                config_home: Some("/config".into()),
                ..home_robot()
            })
            .unwrap()
        });

        let expected = PathBuf::from("/config/imify");
        assert_eq!(dirs, [expected.clone(), expected]);
    }

    #[test]
    fn config_dir_falls_back_to_dot_config_in_home_when_xdg_config_home_is_unset_empty_or_relative()
    {
        let dirs = [None, Some("".into()), Some("config".into())].map(|config_home| {
            config_dir(&Environment {
                config_home,
                ..home_robot()
            })
            .unwrap()
        });

        let expected = PathBuf::from("/home/robot/.config/imify");
        assert_eq!(dirs, [expected.clone(), expected.clone(), expected]);
    }

    #[test]
    fn config_dir_with_xdg_config_home_does_not_need_home() {
        let env = Environment {
            config_home: Some("/config".into()),
            ..Environment::default()
        };

        assert_eq!(config_dir(&env).unwrap(), PathBuf::from("/config/imify"));
    }

    #[test]
    fn config_dir_without_an_absolute_home_is_an_invalid_argument_naming_both_variables() {
        let errors = unusable_homes().map(|home| {
            config_dir(&Environment {
                home,
                ..Environment::default()
            })
            .unwrap_err()
        });

        for error in errors {
            assert_eq!(error.code, ErrorCode::InvalidArgument);
            assert_eq!(
                error.hint.as_deref(),
                Some("Set `HOME` to your home directory, or `IMIFY_CONFIG_DIR` to a directory.")
            );
        }
    }

    #[test]
    fn config_home_is_xdg_config_home_and_ignores_imify_config_dir() {
        let env = Environment {
            config_home: Some("/config".into()),
            config_dir: Some("/etc/imify".into()),
            ..home_robot()
        };

        assert_eq!(config_home(&env), Some(PathBuf::from("/config")));
    }

    #[test]
    fn config_home_falls_back_to_dot_config_in_home() {
        assert_eq!(
            config_home(&home_robot()),
            Some(PathBuf::from("/home/robot/.config"))
        );
    }

    #[test]
    fn config_home_without_an_absolute_home_is_none() {
        let homes = unusable_homes().map(|home| {
            config_home(&Environment {
                home,
                ..Environment::default()
            })
        });

        assert_eq!(homes, [None, None, None]);
    }

    #[test]
    fn journal_dir_is_below_xdg_state_home() {
        let env = Environment {
            state_home: Some("/state".into()),
            ..home_robot()
        };

        assert_eq!(
            journal_dir(&env).unwrap(),
            PathBuf::from("/state/imify/uploads")
        );
    }

    #[test]
    fn journal_dir_falls_back_to_local_state_in_home_when_xdg_state_home_is_unset_empty_or_relative()
     {
        let dirs = [None, Some("".into()), Some("state".into())].map(|state_home| {
            journal_dir(&Environment {
                state_home,
                ..home_robot()
            })
            .unwrap()
        });

        let expected = PathBuf::from("/home/robot/.local/state/imify/uploads");
        assert_eq!(dirs, [expected.clone(), expected.clone(), expected]);
    }

    #[test]
    fn journal_dir_ignores_imify_config_dir() {
        let env = Environment {
            config_dir: Some("/etc/imify".into()),
            ..home_robot()
        };

        assert_eq!(
            journal_dir(&env).unwrap(),
            PathBuf::from("/home/robot/.local/state/imify/uploads")
        );
    }

    #[test]
    fn update_check_file_is_below_xdg_state_home() {
        let env = Environment {
            state_home: Some("/state".into()),
            ..home_robot()
        };

        assert_eq!(
            update_check_file(&env),
            Some(PathBuf::from("/state/imify/update-check.json"))
        );
    }

    #[test]
    fn update_check_file_falls_back_to_local_state_in_home() {
        assert_eq!(
            update_check_file(&home_robot()),
            Some(PathBuf::from(
                "/home/robot/.local/state/imify/update-check.json"
            ))
        );
    }

    #[test]
    fn update_check_file_without_an_absolute_home_is_none() {
        let files = unusable_homes().map(|home| {
            update_check_file(&Environment {
                home,
                ..Environment::default()
            })
        });

        assert_eq!(files, [None, None, None]);
    }

    #[test]
    fn journal_dir_without_an_absolute_home_is_an_invalid_argument_naming_home() {
        let errors = unusable_homes().map(|home| {
            journal_dir(&Environment {
                home,
                ..Environment::default()
            })
            .unwrap_err()
        });

        for error in errors {
            assert_eq!(error.code, ErrorCode::InvalidArgument);
            assert_eq!(
                error.hint.as_deref(),
                Some("Set `HOME` to your home directory.")
            );
        }
    }
}
