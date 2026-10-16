mod cli;
mod output;

use std::ffi::OsString;
use std::process::ExitCode;

use cli::Command;
use output::renderer::Renderer;
use output::{CliError, Environment, Settings};

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    let env = Environment::capture();
    let fallback = Settings::fallback(&args, &env);

    let cli = match cli::parse(&args, fallback.color) {
        Ok(cli) => cli,
        Err(error) if !error.use_stderr() => return print_help_or_version(&error),
        Err(error) => return report(fallback.renderer().as_mut(), &error.into()),
    };
    let settings = match Settings::resolve(&cli.global, &env) {
        Ok(settings) => settings,
        Err(error) => return report(fallback.renderer().as_mut(), &error),
    };
    init_tracing(&settings);

    let mut renderer = settings.renderer();
    match run(cli.command, &settings, renderer.as_mut()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => report(renderer.as_mut(), &error),
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "commands return errors; no command exists yet"
)]
fn run(
    command: Option<Command>,
    _settings: &Settings,
    _renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    match command {
        None => Ok(()),
        Some(command) => match command {},
    }
}

/// Prints clap's `--help` or `--version` output to stdout.
fn print_help_or_version(output: &clap::Error) -> ExitCode {
    match output.print() {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn report(renderer: &mut dyn Renderer, error: &CliError) -> ExitCode {
    // stderr is the last channel left; when writing to it fails, the exit code still reports
    // the error.
    let _ = renderer.error(error);
    ExitCode::from(error.code.exit_code())
}

fn init_tracing(settings: &Settings) {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(settings.log_level())
        .with_target(false)
        .without_time()
        .init();
}
