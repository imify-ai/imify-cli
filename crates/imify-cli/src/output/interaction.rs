//! Prompts. The CLI never prompts when stdin is not a terminal.

use std::io::{BufRead, Write};

use super::Settings;
use super::error::{CliError, ErrorCode};

/// Asks `question` on `prompt` and reads a yes/no answer from `stdin`.
///
/// Returns `Ok` without reading when `--yes` is set. Accepts `y` and `yes` in any case.
///
/// # Errors
///
/// Returns `invalid_argument` with a hint naming `--yes` when stdin is not a terminal, and
/// `cancelled` when the answer is anything other than yes.
pub fn confirm(
    question: &str,
    settings: &Settings,
    prompt: &mut dyn Write,
    stdin: &mut dyn BufRead,
) -> Result<(), CliError> {
    if settings.yes {
        return Ok(());
    }
    if !settings.stdin_is_tty {
        return Err(CliError::new(
            ErrorCode::InvalidArgument,
            format!("Confirmation required: {question}"),
        )
        .with_hint("Pass `--yes` to confirm without a prompt."));
    }

    let io_error = |error: std::io::Error| {
        CliError::new(
            ErrorCode::Internal,
            format!("Could not read the answer: {error}"),
        )
    };
    write!(prompt, "{question} [y/N] ").map_err(io_error)?;
    prompt.flush().map_err(io_error)?;
    let mut answer = String::new();
    stdin.read_line(&mut answer).map_err(io_error)?;

    let answer = answer.trim();
    if answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes") {
        Ok(())
    } else {
        Err(CliError::new(
            ErrorCode::Cancelled,
            "Cancelled at the confirmation prompt.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::output::OutputMode;

    fn settings(yes: bool, stdin_is_tty: bool) -> Settings {
        Settings {
            mode: OutputMode::Human,
            quiet: false,
            verbosity: 0,
            color: false,
            yes,
            stdin_is_tty,
            live_view: false,
        }
    }

    #[test]
    fn without_a_terminal_and_without_yes_fails_with_a_hint_and_reads_nothing() {
        let mut stdin = Cursor::new(b"y\n".to_vec());

        let error = confirm(
            "Delete dataset 42?",
            &settings(false, false),
            &mut Vec::new(),
            &mut stdin,
        )
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.hint.unwrap().contains("--yes"));
        assert_eq!(stdin.position(), 0);
    }

    #[test]
    fn yes_confirms_without_reading_stdin() {
        let mut stdin = Cursor::new(b"n\n".to_vec());

        let result = confirm(
            "Delete dataset 42?",
            &settings(true, false),
            &mut Vec::new(),
            &mut stdin,
        );

        assert_eq!(result, Ok(()));
        assert_eq!(stdin.position(), 0);
    }

    #[test]
    fn on_a_terminal_y_confirms() {
        let mut prompt = Vec::new();

        let result = confirm(
            "Delete dataset 42?",
            &settings(false, true),
            &mut prompt,
            &mut Cursor::new(b"y\n".to_vec()),
        );

        assert_eq!(result, Ok(()));
        assert_eq!(prompt, b"Delete dataset 42? [y/N] ");
    }

    #[test]
    fn on_a_terminal_an_empty_answer_cancels() {
        let error = confirm(
            "Delete dataset 42?",
            &settings(false, true),
            &mut Vec::new(),
            &mut Cursor::new(b"\n".to_vec()),
        )
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::Cancelled);
    }

    #[test]
    fn on_a_terminal_n_cancels() {
        let error = confirm(
            "Delete dataset 42?",
            &settings(false, true),
            &mut Vec::new(),
            &mut Cursor::new(b"n\n".to_vec()),
        )
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::Cancelled);
    }
}
