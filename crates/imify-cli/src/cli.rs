//! The command-line grammar: global flags and the top-level commands.

use std::ffi::OsString;

use clap::{ArgAction, Args, CommandFactory, FromArgMatches, Parser, Subcommand};

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
pub enum Command {}

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
