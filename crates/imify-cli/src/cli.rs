//! The command-line grammar: global flags and the top-level commands.

use std::ffi::OsString;

use clap::{ArgAction, Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};

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
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show the user the access token belongs to
    #[command(
        after_help = "Example:\n  IMIFY_TOKEN=<token> IMIFY_API_URL=https://api.example.com imify whoami"
    )]
    Whoami,
    /// Work with datasets
    Datasets {
        #[command(subcommand)]
        command: DatasetsCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum DatasetsCommand {
    /// List the datasets visible to the access token's user
    #[command(after_help = "Examples:\n  \
        imify datasets ls\n  \
        imify datasets ls --embodiment so100 --sort episodes --reverse\n  \
        imify datasets ls --task cube --json")]
    Ls(LsArgs),
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
