use std::process::ExitCode;

use clap::Parser;

/// Command-line interface for the imify platform.
#[derive(Debug, Parser)]
#[command(name = "imify", version, about)]
struct Cli {}

fn main() -> ExitCode {
    let _cli = Cli::parse();
    ExitCode::SUCCESS
}
