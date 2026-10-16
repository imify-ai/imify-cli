//! Reads a pasted line from the terminal.

use std::io;
use std::os::fd::{AsFd, BorrowedFd};

use rustix::io::Errno;
use rustix::termios::{self, LocalModes, OptionalActions, SpecialCodeIndex, Termios};

/// The longest line [`read_line`] accepts, in bytes.
pub const MAX_LINE: usize = 16 * 1024;

/// Reads one line from the terminal `terminal`, up to [`MAX_LINE`] bytes, without the line
/// end (`\n` or `\r`).
///
/// Canonical mode and signal generation are off while it reads, so a line longer than the
/// terminal's line buffer (1024 bytes on macOS) arrives whole. Echo stays on; backspace does
/// not edit the line. The terminal's interrupt character (Ctrl-C) cancels the read, and its
/// end-of-file character (Ctrl-D) on an empty line ends the input. The terminal state is
/// restored before the function returns, on success and on error, and unread input is
/// discarded.
///
/// # Errors
///
/// Returns the error of reading or changing the terminal state, `Interrupted` for the
/// interrupt character, `InvalidInput` for a line longer than [`MAX_LINE`], `InvalidData`
/// for a line that is not UTF-8, and `UnexpectedEof` when the input ends before any byte.
pub fn read_line(terminal: impl AsFd) -> io::Result<String> {
    let fd = terminal.as_fd();
    let restore = Restore::switch_to_raw_line(fd)?;
    let line = read_until_line_end(fd, &restore.controls);
    restore.finish()?;
    String::from_utf8(line?).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the pasted text is not valid UTF-8",
        )
    })
}

/// The characters that cancel the read or end the input, `None` when disabled.
struct Controls {
    interrupt: Option<u8>,
    end_of_file: Option<u8>,
}

impl Controls {
    fn of(termios: &Termios) -> Self {
        // A special character is disabled with 0 on Linux and 0xff on macOS.
        let enabled = |byte: u8| (byte != 0 && byte != 0xff).then_some(byte);
        Self {
            interrupt: enabled(termios.special_codes[SpecialCodeIndex::VINTR]),
            end_of_file: enabled(termios.special_codes[SpecialCodeIndex::VEOF]),
        }
    }
}

/// Restores the terminal state it saved, at the latest when dropped, and discards input that
/// was not read.
struct Restore<'fd> {
    fd: BorrowedFd<'fd>,
    original: Option<Termios>,
    controls: Controls,
}

impl<'fd> Restore<'fd> {
    fn switch_to_raw_line(fd: BorrowedFd<'fd>) -> io::Result<Self> {
        let original = termios::tcgetattr(fd)?;
        let mut raw_line = original.clone();
        raw_line
            .local_modes
            .remove(LocalModes::ICANON | LocalModes::ISIG);
        raw_line.special_codes[SpecialCodeIndex::VMIN] = 1;
        raw_line.special_codes[SpecialCodeIndex::VTIME] = 0;
        let restore = Self {
            fd,
            controls: Controls::of(&original),
            original: Some(original),
        };
        termios::tcsetattr(fd, OptionalActions::Now, &raw_line)?;
        Ok(restore)
    }

    fn finish(mut self) -> io::Result<()> {
        match self.original.take() {
            Some(original) => Ok(termios::tcsetattr(
                self.fd,
                OptionalActions::Flush,
                &original,
            )?),
            None => Ok(()),
        }
    }
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        if let Some(original) = self.original.take() {
            // Dropping without `finish` means an earlier error is already being returned.
            let _ = termios::tcsetattr(self.fd, OptionalActions::Flush, &original);
        }
    }
}

fn read_until_line_end(fd: BorrowedFd<'_>, controls: &Controls) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = match rustix::io::read(fd, &mut chunk) {
            Ok(read) => read,
            Err(Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
        };
        if read == 0 {
            return end_of_input(line);
        }
        for &byte in &chunk[..read] {
            if Some(byte) == controls.interrupt {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "the paste was cancelled",
                ));
            }
            if Some(byte) == controls.end_of_file {
                return end_of_input(line);
            }
            if matches!(byte, b'\n' | b'\r') {
                return Ok(line);
            }
            line.push(byte);
            if line.len() > MAX_LINE {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the pasted text is longer than 16 KiB",
                ));
            }
        }
    }
}

fn end_of_input(line: Vec<u8>) -> io::Result<Vec<u8>> {
    if line.is_empty() {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the input ended before a line was pasted",
        ))
    } else {
        Ok(line)
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

    use super::*;

    /// A pseudo-terminal: `user` stands for the person typing, `terminal` is what the CLI reads.
    struct Pty {
        user: File,
        terminal: File,
    }

    fn pty() -> Pty {
        let user = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        grantpt(&user).unwrap();
        unlockpt(&user).unwrap();
        let name = ptsname(&user, Vec::new()).unwrap();
        let terminal = OpenOptions::new()
            .read(true)
            .write(true)
            .open(name.to_str().unwrap())
            .unwrap();
        Pty {
            user: File::from(user),
            terminal,
        }
    }

    /// The local modes without `PENDIN`, which the kernel sets when it switches back to
    /// canonical mode with unread input.
    fn local_modes(terminal: &File) -> LocalModes {
        termios::tcgetattr(terminal).unwrap().local_modes - LocalModes::PENDIN
    }

    /// Types `input` once the terminal is in non-canonical mode, and discards the echo.
    /// Reads and drops the terminal's output, so the echo never fills the output queue and
    /// restoring the state with `Flush` does not wait for it.
    fn discard_echo(pty: &Pty) {
        let mut echo = pty.user.try_clone().unwrap();
        std::thread::spawn(move || {
            let mut sink = [0_u8; 4096];
            while echo.read(&mut sink).is_ok_and(|read| read > 0) {}
        });
    }

    fn type_when_non_canonical(pty: &Pty, input: Vec<u8>) {
        let mut user = pty.user.try_clone().unwrap();
        let terminal = pty.terminal.try_clone().unwrap();
        discard_echo(pty);
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while local_modes(&terminal).contains(LocalModes::ICANON) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            user.write_all(&input).unwrap();
        });
    }

    #[test]
    fn a_line_longer_than_the_canonical_buffer_is_returned_unchanged() {
        let pty = pty();
        let url = format!("http://localhost:28753/callback?code={}", "c".repeat(2000));
        type_when_non_canonical(&pty, format!("{url}\n").into_bytes());

        let line = read_line(&pty.terminal).unwrap();

        assert_eq!(line, url);
    }

    #[test]
    fn a_carriage_return_ends_the_line() {
        let pty = pty();
        let mut terminal_state = termios::tcgetattr(&pty.terminal).unwrap();
        terminal_state
            .input_modes
            .remove(termios::InputModes::ICRNL);
        termios::tcsetattr(&pty.terminal, OptionalActions::Now, &terminal_state).unwrap();
        type_when_non_canonical(&pty, b"http://localhost/callback\r".to_vec());

        assert_eq!(
            read_line(&pty.terminal).unwrap(),
            "http://localhost/callback"
        );
    }

    #[test]
    fn the_terminal_state_is_restored_after_a_line() {
        let pty = pty();
        let before = local_modes(&pty.terminal);
        type_when_non_canonical(&pty, b"http://localhost/callback\n".to_vec());

        read_line(&pty.terminal).unwrap();

        assert_eq!(local_modes(&pty.terminal), before);
    }

    #[test]
    fn the_terminal_state_is_restored_after_an_error() {
        let pty = pty();
        let before = local_modes(&pty.terminal);
        type_when_non_canonical(&pty, vec![b'x'; MAX_LINE + 1]);

        let error = read_line(&pty.terminal).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(local_modes(&pty.terminal), before);
    }

    #[test]
    fn a_descriptor_that_is_not_a_terminal_is_an_error() {
        let file = tempfile::tempfile().unwrap();

        assert!(read_line(&file).is_err());
    }

    #[test]
    fn the_interrupt_character_cancels_the_read_and_restores_the_terminal() {
        let pty = pty();
        let before = local_modes(&pty.terminal);
        type_when_non_canonical(&pty, b"http://localhost/call\x03".to_vec());

        let error = read_line(&pty.terminal).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(local_modes(&pty.terminal), before);
    }

    #[test]
    fn the_end_of_file_character_on_an_empty_line_ends_the_input() {
        let pty = pty();
        type_when_non_canonical(&pty, b"\x04".to_vec());

        let error = read_line(&pty.terminal).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn input_that_was_not_read_is_discarded_when_the_terminal_is_restored() {
        let pty = pty();
        discard_echo(&pty);
        let restore = Restore::switch_to_raw_line(pty.terminal.as_fd()).unwrap();
        (&pty.user).write_all(b"leftover\nsecond line\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while rustix::io::ioctl_fionread(&pty.terminal).unwrap() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(rustix::io::ioctl_fionread(&pty.terminal).unwrap() > 0);

        restore.finish().unwrap();

        assert_eq!(rustix::io::ioctl_fionread(&pty.terminal).unwrap(), 0);
    }
}
