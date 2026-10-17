//! The cursor-position query of the live view, on the terminal device with a timeout.

#[cfg(test)]
mod tests;

use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use ratatui::layout::Position;
use rustix::fs::{Mode, OFlags};
use rustix::termios::{self, OptionalActions, SpecialCodeIndex, Termios};

/// The time the terminal has to report the cursor position.
pub const QUERY_TIMEOUT: Duration = Duration::from_millis(250);

/// The terminal the cursor-position query talks to.
pub trait Tty: Read + Write {
    /// Waits until input is readable or `timeout` passes. Returns `false` on timeout.
    ///
    /// # Errors
    ///
    /// Returns the error of waiting on the terminal.
    fn wait_readable(&mut self, timeout: Duration) -> io::Result<bool>;

    /// The current time, which the query measures its timeout against.
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// A terminal device in raw mode. The saved terminal attributes are restored on drop.
///
/// Raw mode turns off echo and line buffering, so the terminal's reply to a query is readable
/// at once and does not appear on the screen.
pub struct DevTty {
    fd: OwnedFd,
    saved: Termios,
    raw: Termios,
    /// Input [`Tty::wait_readable`] received and [`Read::read`] has not returned yet.
    pending: Vec<u8>,
}

impl std::fmt::Debug for DevTty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevTty")
            .field("fd", &self.fd)
            .finish_non_exhaustive()
    }
}

impl DevTty {
    /// Opens `/dev/tty` and switches it to raw mode.
    ///
    /// # Errors
    ///
    /// Returns the error of opening the device, which fails when the process has no
    /// controlling terminal, or of changing its attributes.
    pub fn open() -> io::Result<Self> {
        let fd = rustix::fs::open(
            "/dev/tty",
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        Self::from_fd(fd)
    }

    /// Switches the terminal `fd` to raw mode.
    ///
    /// # Errors
    ///
    /// Returns the error of reading or changing the terminal attributes, which fails when `fd`
    /// is not a terminal.
    pub fn from_fd(fd: OwnedFd) -> io::Result<Self> {
        let saved = termios::tcgetattr(&fd)?;
        let mut raw = saved.clone();
        raw.make_raw();
        raw.special_codes[SpecialCodeIndex::VMIN] = 1;
        raw.special_codes[SpecialCodeIndex::VTIME] = 0;
        termios::tcsetattr(&fd, OptionalActions::Now, &raw)?;
        Ok(Self {
            fd,
            saved,
            raw,
            pending: Vec::new(),
        })
    }
}

impl Read for DevTty {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() {
            return Ok(rustix::io::read(&self.fd, buf)?);
        }
        let read = buf.len().min(self.pending.len());
        buf[..read].copy_from_slice(&self.pending[..read]);
        self.pending.drain(..read);
        Ok(read)
    }
}

impl Write for DevTty {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(rustix::io::write(&self.fd, buf)?)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Tty for DevTty {
    /// Waits with a read that the terminal ends after `timeout`, rounded down to tenths of a
    /// second and at most 25.5 s. A `timeout` below 100 ms returns `false` without waiting.
    fn wait_readable(&mut self, timeout: Duration) -> io::Result<bool> {
        if !self.pending.is_empty() {
            return Ok(true);
        }
        let tenths = u8::try_from(timeout.as_millis() / 100).unwrap_or(u8::MAX);
        if tenths == 0 {
            return Ok(false);
        }
        // `poll` does not report input on `/dev/tty` on macOS. A read with the terminal's read
        // timeout (`VMIN` 0, `VTIME` in tenths of a second) works on macOS and Linux.
        let mut timed = self.raw.clone();
        timed.special_codes[SpecialCodeIndex::VMIN] = 0;
        timed.special_codes[SpecialCodeIndex::VTIME] = tenths;
        termios::tcsetattr(&self.fd, OptionalActions::Now, &timed)?;
        let mut chunk = [0_u8; 64];
        let read = rustix::io::read(&self.fd, &mut chunk);
        termios::tcsetattr(&self.fd, OptionalActions::Now, &self.raw)?;
        let read = read?;
        self.pending.extend_from_slice(&chunk[..read]);
        Ok(read > 0)
    }
}

impl Drop for DevTty {
    fn drop(&mut self) {
        // A drop cannot return an error; the terminal stays in raw mode only if this fails.
        let _ = termios::tcsetattr(&self.fd, OptionalActions::Now, &self.saved);
    }
}

/// Writes `ESC[6n` to `tty`, reads until the reply and returns the 0-based position.
///
/// # Errors
///
/// Returns `ErrorKind::TimedOut` when no complete reply arrives within `timeout`,
/// `ErrorKind::UnexpectedEof` when the terminal closes, `ErrorKind::InvalidData` on a malformed
/// reply, and the error of reading or writing the terminal otherwise.
pub fn query_position(tty: &mut impl Tty, timeout: Duration) -> io::Result<Position> {
    tty.write_all(b"\x1b[6n")?;
    tty.flush()?;
    let start = tty.now();
    let mut input = Vec::new();
    let mut chunk = [0_u8; 64];
    loop {
        if let Some(position) = scan(&input)? {
            return Ok(position);
        }
        let remaining = timeout.saturating_sub(tty.now().saturating_duration_since(start));
        if remaining.is_zero() {
            return Err(timed_out(timeout));
        }
        match tty.wait_readable(remaining) {
            Ok(true) => {}
            Ok(false) => return Err(timed_out(timeout)),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
        match tty.read(&mut chunk) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the terminal closed before it reported the cursor position",
                ));
            }
            Ok(read) => input.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

fn timed_out(timeout: Duration) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!(
            "the terminal did not report the cursor position within {} ms",
            timeout.as_millis()
        ),
    )
}

const ESC: u8 = 0x1b;

/// Finds the cursor position report `ESC [ row ; column R` in `input`, the bytes the terminal
/// sent so far. Other bytes and other control sequences before it are skipped. Returns `None`
/// while the report is incomplete.
fn scan(input: &[u8]) -> io::Result<Option<Position>> {
    let mut rest = input;
    while let Some(start) = rest.windows(2).position(|pair| pair == [ESC, b'[']) {
        let sequence = &rest[start + 2..];
        // Parameter bytes are 0x30 to 0x3F, intermediate bytes 0x20 to 0x2F; the first byte
        // outside both ranges ends the sequence.
        let Some(end) = sequence
            .iter()
            .position(|byte| !(0x20..=0x3f).contains(byte))
        else {
            return Ok(None);
        };
        if sequence[end] == b'R' {
            return parse_report(&sequence[..end]).map(Some);
        }
        rest = &sequence[end..];
    }
    Ok(None)
}

/// Parses the parameters `row;column` of a cursor position report, both 1-based.
fn parse_report(parameters: &[u8]) -> io::Result<Position> {
    let malformed = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "the terminal sent a malformed cursor position report: {:?}",
                String::from_utf8_lossy(parameters)
            ),
        )
    };
    let one_based = |field: &[u8]| {
        std::str::from_utf8(field)
            .ok()
            .and_then(|text| text.parse::<u16>().ok())
            .and_then(|value| value.checked_sub(1))
    };
    let mut fields = parameters.split(|&byte| byte == b';');
    match (fields.next(), fields.next(), fields.next()) {
        (Some(row), Some(column), None) => match (one_based(row), one_based(column)) {
            (Some(y), Some(x)) => Ok(Position::new(x, y)),
            _ => Err(malformed()),
        },
        _ => Err(malformed()),
    }
}
