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
/// Canonical mode is off while it reads, so a line longer than the terminal's line buffer
/// (1024 bytes on macOS) arrives whole. Echo stays on; backspace does not edit the line. The
/// terminal state is restored before the function returns, on success and on error.
///
/// # Errors
///
/// Returns the error of reading or changing the terminal state, `InvalidInput` for a line
/// longer than [`MAX_LINE`], `InvalidData` for a line that is not UTF-8, and
/// `UnexpectedEof` when the input ends before any byte.
pub fn read_line(terminal: impl AsFd) -> io::Result<String> {
    let fd = terminal.as_fd();
    let restore = Restore::switch_to_non_canonical(fd)?;
    let line = read_until_line_end(fd);
    restore.finish()?;
    String::from_utf8(line?).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the pasted text is not valid UTF-8",
        )
    })
}

/// Restores the terminal state it saved, at the latest when dropped.
struct Restore<'fd> {
    fd: BorrowedFd<'fd>,
    original: Option<Termios>,
}

impl<'fd> Restore<'fd> {
    fn switch_to_non_canonical(fd: BorrowedFd<'fd>) -> io::Result<Self> {
        let original = termios::tcgetattr(fd)?;
        let mut non_canonical = original.clone();
        non_canonical.local_modes.remove(LocalModes::ICANON);
        non_canonical.special_codes[SpecialCodeIndex::VMIN] = 1;
        non_canonical.special_codes[SpecialCodeIndex::VTIME] = 0;
        let restore = Self {
            fd,
            original: Some(original),
        };
        termios::tcsetattr(fd, OptionalActions::Now, &non_canonical)?;
        Ok(restore)
    }

    fn finish(mut self) -> io::Result<()> {
        match self.original.take() {
            Some(original) => Ok(termios::tcsetattr(
                self.fd,
                OptionalActions::Now,
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
            let _ = termios::tcsetattr(self.fd, OptionalActions::Now, &original);
        }
    }
}

fn read_until_line_end(fd: BorrowedFd<'_>) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = match rustix::io::read(fd, &mut chunk) {
            Ok(read) => read,
            Err(Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
        };
        if read == 0 {
            return if line.is_empty() {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the input ended before a line was pasted",
                ))
            } else {
                Ok(line)
            };
        }
        let received = &chunk[..read];
        let end = received
            .iter()
            .position(|byte| matches!(byte, b'\n' | b'\r'));
        line.extend_from_slice(&received[..end.unwrap_or(read)]);
        if line.len() > MAX_LINE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the pasted text is longer than 16 KiB",
            ));
        }
        if end.is_some() {
            return Ok(line);
        }
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
    fn type_when_non_canonical(pty: &Pty, input: Vec<u8>) {
        let mut user = pty.user.try_clone().unwrap();
        let mut echo = pty.user.try_clone().unwrap();
        let terminal = pty.terminal.try_clone().unwrap();
        std::thread::spawn(move || {
            let mut sink = [0_u8; 4096];
            while echo.read(&mut sink).is_ok_and(|read| read > 0) {}
        });
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
}
