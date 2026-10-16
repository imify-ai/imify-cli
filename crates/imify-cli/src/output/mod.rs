//! Everything that reaches stdout or stderr: output settings, renderers, errors and prompts.

mod error;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "commands that prompt call it; no command exists yet"
    )
)]
pub mod interaction;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "commands render through it; no command exists yet"
    )
)]
pub mod renderer;
pub mod table;

use std::ffi::OsString;
use std::io::{self, IsTerminal};

use tracing::level_filters::LevelFilter;

pub use self::error::{AUTH_REQUIRED_HINT, CliError, ErrorCode};
use self::renderer::{HumanRenderer, JsonRenderer, Renderer};
use crate::cli::GlobalArgs;

/// The output format: text for people, or JSON for scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Human,
    Json,
}

/// The environment values the CLI reads, captured once at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Environment {
    /// `IMIFY_OUTPUT`; `None` when unset or not valid UTF-8.
    pub imify_output: Option<String>,
    /// `NO_COLOR`; `None` when unset.
    pub no_color: Option<OsString>,
    /// `IMIFY_API_URL`; `None` when unset or not valid UTF-8.
    pub api_url: Option<String>,
    pub stdin_is_tty: bool,
}

impl Environment {
    /// Reads `IMIFY_OUTPUT`, `NO_COLOR`, `IMIFY_API_URL` and whether stdin is a terminal.
    pub fn capture() -> Self {
        Self {
            imify_output: std::env::var("IMIFY_OUTPUT").ok(),
            no_color: std::env::var_os("NO_COLOR"),
            api_url: std::env::var("IMIFY_API_URL").ok(),
            stdin_is_tty: io::stdin().is_terminal(),
        }
    }

    fn no_color(&self) -> bool {
        self.no_color
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    }

    fn output_mode(&self) -> Result<Option<OutputMode>, CliError> {
        match self.imify_output.as_deref() {
            None | Some("") => Ok(None),
            Some("json") => Ok(Some(OutputMode::Json)),
            Some("human") => Ok(Some(OutputMode::Human)),
            Some(other) => Err(CliError::new(
                ErrorCode::InvalidArgument,
                format!("IMIFY_OUTPUT has the unsupported value '{other}'."),
            )
            .with_hint("Set IMIFY_OUTPUT to `json` or `human`, or unset it.")),
        }
    }
}

/// The resolved output settings for one invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent setting"
)]
pub struct Settings {
    pub mode: OutputMode,
    pub quiet: bool,
    pub verbosity: u8,
    pub color: bool,
    pub yes: bool,
    pub stdin_is_tty: bool,
}

impl Settings {
    /// Resolves the settings from the parsed global flags and the environment.
    ///
    /// `--json` takes precedence over `IMIFY_OUTPUT`.
    ///
    /// # Errors
    ///
    /// Returns `invalid_argument` when `IMIFY_OUTPUT` is neither `json` nor `human`.
    pub fn resolve(args: &GlobalArgs, env: &Environment) -> Result<Self, CliError> {
        let env_mode = env.output_mode()?;
        let mode = if args.json {
            OutputMode::Json
        } else {
            env_mode.unwrap_or(OutputMode::Human)
        };
        Ok(Self {
            mode,
            quiet: args.quiet,
            verbosity: args.verbose,
            color: !(args.no_color || env.no_color()),
            yes: args.yes,
            stdin_is_tty: env.stdin_is_tty,
        })
    }

    /// The settings for reporting an error that occurs before the global flags are resolved.
    ///
    /// JSON is selected by `--json` before any `--` in `args`, or by `IMIFY_OUTPUT=json`.
    /// An unsupported `IMIFY_OUTPUT` value falls back to human output.
    pub fn fallback(args: &[OsString], env: &Environment) -> Self {
        let json = flag_before_separator(args, "--json")
            || env.output_mode().ok().flatten() == Some(OutputMode::Json);
        Self {
            mode: if json {
                OutputMode::Json
            } else {
                OutputMode::Human
            },
            quiet: false,
            verbosity: 0,
            color: !(flag_before_separator(args, "--no-color") || env.no_color()),
            yes: false,
            stdin_is_tty: env.stdin_is_tty,
        }
    }

    /// The most detailed log level written to stderr.
    pub fn log_level(&self) -> LevelFilter {
        match (self.quiet, self.verbosity) {
            (true, _) => LevelFilter::OFF,
            (false, 0) => LevelFilter::WARN,
            (false, 1) => LevelFilter::DEBUG,
            (false, _) => LevelFilter::TRACE,
        }
    }

    /// The renderer for these settings, writing to the process's stdout and stderr.
    pub fn renderer(&self) -> Box<dyn Renderer> {
        let choice = if self.color {
            anstream::ColorChoice::Auto
        } else {
            anstream::ColorChoice::Never
        };
        let stdout = Box::new(anstream::AutoStream::new(io::stdout(), choice));
        let stderr = Box::new(anstream::AutoStream::new(io::stderr(), choice));
        match self.mode {
            OutputMode::Human => Box::new(HumanRenderer::new(stdout, stderr, self.quiet)),
            OutputMode::Json => Box::new(JsonRenderer::new(stdout, stderr, self.quiet)),
        }
    }
}

/// Whether `flag` appears in `args` before the first `--`.
fn flag_before_separator(args: &[OsString], flag: &str) -> bool {
    args.iter()
        .skip(1)
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == flag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(json: bool, no_color: bool) -> GlobalArgs {
        GlobalArgs {
            json,
            no_color,
            ..GlobalArgs::default()
        }
    }

    fn env_with_output(value: &str) -> Environment {
        Environment {
            imify_output: Some(value.to_owned()),
            ..Environment::default()
        }
    }

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn output_is_human_by_default() {
        let settings = Settings::resolve(&args(false, false), &Environment::default()).unwrap();

        assert_eq!(settings.mode, OutputMode::Human);
    }

    #[test]
    fn imify_output_json_selects_json() {
        let settings = Settings::resolve(&args(false, false), &env_with_output("json")).unwrap();

        assert_eq!(settings.mode, OutputMode::Json);
    }

    #[test]
    fn json_flag_takes_precedence_over_imify_output_human() {
        let settings = Settings::resolve(&args(true, false), &env_with_output("human")).unwrap();

        assert_eq!(settings.mode, OutputMode::Json);
    }

    #[test]
    fn unsupported_imify_output_value_is_an_invalid_argument() {
        let error = Settings::resolve(&args(false, false), &env_with_output("xml")).unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn color_is_enabled_by_default() {
        let settings = Settings::resolve(&args(false, false), &Environment::default()).unwrap();

        assert!(settings.color);
    }

    #[test]
    fn no_color_flag_disables_color() {
        let settings = Settings::resolve(&args(false, true), &Environment::default()).unwrap();

        assert!(!settings.color);
    }

    #[test]
    fn non_empty_no_color_variable_disables_color() {
        let env = Environment {
            no_color: Some("1".into()),
            ..Environment::default()
        };

        let settings = Settings::resolve(&args(false, false), &env).unwrap();

        assert!(!settings.color);
    }

    #[test]
    fn empty_no_color_variable_keeps_color() {
        let env = Environment {
            no_color: Some("".into()),
            ..Environment::default()
        };

        let settings = Settings::resolve(&args(false, false), &env).unwrap();

        assert!(settings.color);
    }

    #[test]
    fn verbosity_selects_the_log_level() {
        let level = |quiet, verbose| {
            let global = GlobalArgs {
                quiet,
                verbose,
                ..GlobalArgs::default()
            };
            Settings::resolve(&global, &Environment::default())
                .unwrap()
                .log_level()
        };

        assert_eq!(
            [
                level(true, 0),
                level(false, 0),
                level(false, 1),
                level(false, 2)
            ],
            [
                LevelFilter::OFF,
                LevelFilter::WARN,
                LevelFilter::DEBUG,
                LevelFilter::TRACE
            ]
        );
    }

    #[test]
    fn fallback_settings_select_json_from_the_raw_json_flag() {
        let settings = Settings::fallback(
            &os_args(&["imify", "--json", "--bad"]),
            &Environment::default(),
        );

        assert_eq!(settings.mode, OutputMode::Json);
    }

    #[test]
    fn fallback_settings_ignore_a_json_flag_after_the_separator() {
        let settings = Settings::fallback(
            &os_args(&["imify", "--bad", "--", "--json"]),
            &Environment::default(),
        );

        assert_eq!(settings.mode, OutputMode::Human);
    }

    #[test]
    fn fallback_settings_fall_back_to_human_for_an_unsupported_imify_output() {
        let settings = Settings::fallback(&os_args(&["imify", "--bad"]), &env_with_output("xml"));

        assert_eq!(settings.mode, OutputMode::Human);
    }
}
