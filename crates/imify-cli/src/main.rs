mod cli;
mod commands;
mod output;
mod tui;

use std::ffi::OsString;
use std::process::ExitCode;

use cli::{Command, DatasetsCommand};
use output::renderer::Renderer;
use output::{CliError, Environment, ErrorCode, Settings};

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
    match run(cli.command.as_ref(), &env, renderer.as_mut()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => report(renderer.as_mut(), &error),
    }
}

fn run(
    command: Option<&Command>,
    env: &Environment,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    match command {
        None => Ok(()),
        Some(Command::Whoami) => runtime()?.block_on(commands::auth::whoami(env, renderer)),
        Some(Command::Datasets {
            command: DatasetsCommand::Ls(args),
        }) => runtime()?.block_on(commands::datasets::ls(env, args, renderer)),
        Some(Command::Datasets {
            command: DatasetsCommand::Validate(args),
        }) => commands::datasets::validate(args, renderer),
    }
}

/// The async runtime for SDK calls. Commands run one request at a time on the main thread.
fn runtime() -> Result<tokio::runtime::Runtime, CliError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            CliError::new(
                ErrorCode::Internal,
                format!("Cannot start the async runtime: {error}"),
            )
        })
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
