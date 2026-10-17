//! The command-line grammar: global flags and the top-level commands.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{ArgAction, Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};

use crate::config::Key;

/// Command-line interface for the imify platform.
#[derive(Debug, Parser)]
#[command(name = "imify", version, about)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Flags accepted before or after any command.
#[derive(Debug, Default, Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent command-line flag"
)]
pub struct GlobalArgs {
    #[arg(
        long,
        global = true,
        help = "Write JSON instead of text [env: IMIFY_OUTPUT=json]"
    )]
    pub json: bool,
    /// Write the result only
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,
    #[arg(
        short,
        long,
        global = true,
        action = ArgAction::Count,
        help = "Log more detail to stderr: -v for debug, -vv for trace"
    )]
    pub verbose: u8,
    /// Confirm without prompting
    #[arg(long, global = true)]
    pub yes: bool,
    #[arg(long, global = true, help = "Disable color [env: NO_COLOR]")]
    pub no_color: bool,
    #[arg(
        long,
        global = true,
        value_name = "URL",
        value_parser = not_blank,
        help = "API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]"
    )]
    pub api_url: Option<String>,
    #[arg(
        long,
        global = true,
        value_name = "NAME",
        value_parser = not_blank,
        help = "Settings profile in the config file [env: IMIFY_PROFILE] [default: default]"
    )]
    pub profile: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(
        about = "Check the configuration, the API and the access token",
        long_about = "Check the configuration, the API and the access token.

Writes a report to stdout. Exits with the error code of the first failing check, or 0 when no check fails. Contacts only the API host.",
        after_help = "Example:
  imify doctor --api-url https://api.example.com"
    )]
    Doctor,
    /// Sign in, sign out and show the signed-in user
    #[command(after_help = "Examples:
  imify auth login
  imify auth whoami
  imify auth logout")]
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Read and change the settings in the config file
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Work with datasets
    Datasets {
        #[command(subcommand)]
        command: DatasetsCommand,
    },
    /// Manage this imify installation
    #[command(name = "self", after_help = "Example:\n  imify self update")]
    Itself {
        #[command(subcommand)]
        command: SelfCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum SelfCommand {
    #[command(
        about = "Update imify to the latest release",
        long_about = "Update imify to the latest release.

Works for an install by the imify installer: looks up the latest release on GitHub and, when it is newer, runs its installer for the install directory recorded in the install receipt. Other installs fail with a hint naming their update command.",
        after_help = "Example:\n  imify self update"
    )]
    Update,
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Sign in through the browser and store the login for the host
    #[command(
        long_about = "Sign in through the browser and store the login for the API host.

Opens the browser and waits up to 5 minutes. The login is stored in the OS keychain, or in a file readable only by the user when no keychain is available.",
        after_help = "Examples:
  imify auth login
  imify auth login --api-url https://api.example.com
  imify auth login --no-browser"
    )]
    Login {
        /// Print the sign-in URL and read the redirect URL from the terminal instead of opening a browser
        #[arg(long)]
        no_browser: bool,
    },
    /// Revoke and remove the stored login for the host
    #[command(after_help = "Example:
  imify auth logout")]
    Logout,
    /// Show the user the access token belongs to
    #[command(after_help = "Example:\n  imify auth whoami")]
    Whoami,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print a setting's value and where it comes from: flag, env, config or default
    #[command(after_help = "Example:\n  imify config get api-url")]
    Get {
        /// The setting
        key: Key,
    },
    /// Check a value and store it in the config file
    #[command(after_help = "Example:\n  imify config set api-url https://api.example.com")]
    Set {
        /// The setting
        key: Key,
        /// The new value
        value: String,
    },
    /// Remove a setting from the config file
    #[command(after_help = "Example:\n  imify config unset api-url")]
    Unset {
        /// The setting
        key: Key,
    },
    /// Print every setting with its value and where it comes from
    #[command(after_help = "Example:\n  imify config list --json")]
    List,
    /// List the profiles in the config file with the API URL of each
    #[command(after_help = "Example:\n  imify config profiles")]
    Profiles,
}

#[derive(Debug, Subcommand)]
pub enum DatasetsCommand {
    /// List the datasets visible to the access token's user
    #[command(after_help = "Examples:\n  \
        imify datasets ls\n  \
        imify datasets ls --embodiment so100 --sort episodes --reverse\n  \
        imify datasets ls --task cube --json")]
    Ls(LsArgs),
    #[command(
        about = "Check a local LeRobot dataset before an upload",
        long_about = "Check a local LeRobot dataset before an upload.\n\nWrites the report to stdout and exits with 5 when it has an error. Needs no network access and no token.",
        after_help = "Example:\n  imify datasets validate ./pick-cube"
    )]
    Validate(ValidateArgs),
    #[command(
        about = "Upload a local LeRobot dataset into an ingest session",
        long_about = "Upload a local LeRobot dataset into an ingest session.\n\nValidates the dataset first and uploads nothing when the report has an error. Uploads meta/, data/ and videos/, with meta/info.json first. Writes the ingest session to stdout. Ctrl-C stops the upload and leaves the session open. Running the same upload again continues that session and sends only the files it lacks.",
        after_help = "Examples:\n  \
            imify datasets upload ./pick-cube --task \"Pick up the red cube\"\n  \
            imify datasets upload ./pick-cube --task \"Pick up the red cube\" --dry-run\n  \
            imify datasets upload ./pick-cube --task \"Pick up the red cube\" --restart"
    )]
    Upload(UploadArgs),
}

/// Filters and order of `imify datasets ls`.
#[derive(Debug, Default, Args)]
pub struct LsArgs {
    /// Keep datasets whose task contains TEXT, ignoring case
    #[arg(long, value_name = "TEXT")]
    pub task: Option<String>,
    /// Keep datasets with exactly this format, such as `lerobot_v3`
    #[arg(long)]
    pub format: Option<String>,
    /// Keep datasets with exactly this embodiment label, such as `so100`
    #[arg(long)]
    pub embodiment: Option<String>,
    /// Sort by FIELD, ties by id. Without it: by task, then embodiment, then id
    #[arg(long, value_name = "FIELD")]
    pub sort: Option<SortField>,
    /// Reverse the order
    #[arg(long)]
    pub reverse: bool,
}

/// A field `imify datasets ls` sorts by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SortField {
    Id,
    Task,
    Format,
    Embodiment,
    Episodes,
    Accepted,
}

/// The argument of `imify datasets validate`.
#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// The dataset root: the directory that contains meta/info.json
    pub path: PathBuf,
}

/// The arguments of `imify datasets upload`.
#[derive(Debug, Args)]
pub struct UploadArgs {
    /// The dataset root: the directory that contains meta/info.json
    pub path: PathBuf,
    /// Task description of the recorded episodes
    #[arg(long, value_name = "TEXT", value_parser = not_blank)]
    pub task: String,
    /// Validate and list the files to upload; needs no token and no network
    #[arg(long)]
    pub dry_run: bool,
    /// Continue the interrupted upload of PATH; fail when there is none
    #[arg(long, conflicts_with = "restart")]
    pub resume: bool,
    /// Discard the interrupted upload of PATH and start a new ingest session
    #[arg(long)]
    pub restart: bool,
}

/// Accepts `value` unless it is empty or whitespace only.
fn not_blank(value: &str) -> Result<String, &'static str> {
    if value.trim().is_empty() {
        Err("the value is empty")
    } else {
        Ok(value.to_owned())
    }
}

/// Parses `args`, including the program name. `color` enables color in help and errors.
///
/// # Errors
///
/// Returns the clap error for invalid arguments, `--help` and `--version`.
pub fn parse(args: &[OsString], color: bool) -> Result<Cli, clap::Error> {
    let color = if color {
        clap::ColorChoice::Auto
    } else {
        clap::ColorChoice::Never
    };
    let matches = Cli::command().color(color).try_get_matches_from(args)?;
    Cli::from_arg_matches(&matches)
}
