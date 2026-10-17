//! Behavior of the cursor-position query against a scripted terminal, and of the raw mode of
//! the terminal device on a pseudo-terminal.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use ratatui::layout::Position;
use rustix::fs::{Mode, OFlags};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{self, ControlModes, InputModes, LocalModes, OutputModes, SpecialCodeIndex};

use super::*;

/// What the fake terminal does when the query waits for input.
enum Step {
    /// These bytes become readable at once.
    Input(&'static [u8]),
    /// These bytes become readable after the given time.
    Late(Duration, &'static [u8]),
    /// The wait times out; the fake clock advances by the timeout.
    Silence,
    /// The terminal closes: the next `read` returns 0.
    Closed,
}

/// A terminal that follows a script and records what the query does.
struct FakeTty {
    script: VecDeque<Step>,
    readable: VecDeque<u8>,
    closed: bool,
    written: Vec<u8>,
    written_before_first_read: Option<Vec<u8>>,
    waits: Vec<Duration>,
    start: Instant,
    elapsed: Duration,
}

impl FakeTty {
    fn new(script: impl IntoIterator<Item = Step>) -> Self {
        Self {
            script: script.into_iter().collect(),
            readable: VecDeque::new(),
            closed: false,
            written: Vec::new(),
            written_before_first_read: None,
            waits: Vec::new(),
            start: Instant::now(),
            elapsed: Duration::ZERO,
        }
    }

    fn answering(replies: &[&'static [u8]]) -> Self {
        Self::new(replies.iter().map(|reply| Step::Input(reply)))
    }
}

impl Read for FakeTty {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.written_before_first_read.is_none() {
            self.written_before_first_read = Some(self.written.clone());
        }
        if self.closed {
            return Ok(0);
        }
        let n = buf.len().min(self.readable.len());
        for (slot, byte) in buf.iter_mut().zip(self.readable.drain(..n)) {
            *slot = byte;
        }
        Ok(n)
    }
}

impl Write for FakeTty {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Tty for FakeTty {
    fn wait_readable(&mut self, timeout: Duration) -> io::Result<bool> {
        self.waits.push(timeout);
        match self.script.pop_front() {
            Some(Step::Input(bytes)) => {
                self.readable.extend(bytes);
                Ok(true)
            }
            Some(Step::Late(delay, bytes)) => {
                self.elapsed += delay.min(timeout);
                self.readable.extend(bytes);
                Ok(delay <= timeout)
            }
            Some(Step::Closed) => {
                self.closed = true;
                Ok(true)
            }
            Some(Step::Silence) | None => {
                self.elapsed += timeout;
                Ok(false)
            }
        }
    }

    fn now(&self) -> Instant {
        self.start + self.elapsed
    }
}

const BUDGET: Duration = Duration::from_millis(250);

#[test]
fn query_writes_the_cursor_position_request_before_reading() {
    let mut tty = FakeTty::answering(&[b"\x1b[1;1R"]);

    query_position(&mut tty, BUDGET).unwrap();

    assert_eq!(
        tty.written_before_first_read.as_deref(),
        Some(&b"\x1b[6n"[..])
    );
}

#[test]
fn reply_is_parsed_as_a_zero_based_position() {
    let mut tty = FakeTty::answering(&[b"\x1b[12;5R"]);

    let position = query_position(&mut tty, BUDGET).unwrap();

    assert_eq!(position, Position::new(4, 11));
}

#[test]
fn reply_split_over_several_reads_is_assembled() {
    let mut tty = FakeTty::answering(&[b"\x1b", b"[3", b"0;", b"12", b"R"]);

    let position = query_position(&mut tty, BUDGET).unwrap();

    assert_eq!(position, Position::new(11, 29));
}

#[test]
fn input_before_the_reply_is_skipped() {
    let mut tty = FakeTty::answering(&[b"ls\x1b[A\x1b", b"[7;2R"]);

    let position = query_position(&mut tty, BUDGET).unwrap();

    assert_eq!(position, Position::new(1, 6));
}

#[test]
fn terminal_that_never_answers_yields_timed_out() {
    let mut tty = FakeTty::new([]);

    let error = query_position(&mut tty, BUDGET).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
}

#[test]
fn waits_for_the_reply_never_exceed_the_timeout() {
    let after_100_ms = Duration::from_millis(100);
    let mut tty = FakeTty::new([
        Step::Late(after_100_ms, b"x"),
        Step::Late(after_100_ms, b"\x1b["),
        Step::Silence,
    ]);

    let _ = query_position(&mut tty, BUDGET);

    assert!(tty.elapsed <= BUDGET, "waits: {:?}", tty.waits);
}

#[test]
fn malformed_reply_yields_invalid_data() {
    let mut tty = FakeTty::answering(&[b"\x1b[12R"]);

    let error = query_position(&mut tty, BUDGET).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn closed_terminal_yields_unexpected_eof() {
    let mut tty = FakeTty::new([Step::Closed]);

    let error = query_position(&mut tty, BUDGET).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
}

/// A pseudo-terminal: `emulator` stands for the terminal emulator, `observer` is a second
/// descriptor of the terminal the query runs on.
///
/// `emulator` is declared first, so it closes first: closing the terminal while the emulator
/// is open waits for the emulator to read the request.
struct Pty {
    emulator: File,
    observer: OwnedFd,
}

/// A pseudo-terminal and a descriptor of its terminal side, for the query.
fn pty() -> (Pty, OwnedFd) {
    let emulator = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
    grantpt(&emulator).unwrap();
    unlockpt(&emulator).unwrap();
    let name = ptsname(&emulator, Vec::new()).unwrap();
    let device = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .unwrap();
    let pty = Pty {
        emulator: File::from(emulator),
        observer: device.try_clone().unwrap(),
    };
    (pty, device)
}

/// The attributes of the terminal the query changes, without `PENDIN`, which the kernel sets
/// when it switches to canonical mode with unread input.
fn attributes(device: &OwnedFd) -> (InputModes, OutputModes, ControlModes, LocalModes, u8, u8) {
    let state = termios::tcgetattr(device).unwrap();
    (
        state.input_modes,
        state.output_modes,
        state.control_modes,
        state.local_modes - LocalModes::PENDIN,
        state.special_codes[SpecialCodeIndex::VMIN],
        state.special_codes[SpecialCodeIndex::VTIME],
    )
}

#[test]
fn terminal_attributes_are_restored_after_a_reply() {
    let (pty, device) = pty();
    let before = attributes(&pty.observer);
    let mut tty = DevTty::from_fd(device).unwrap();
    (&pty.emulator).write_all(b"\x1b[3;7R").unwrap();

    query_position(&mut tty, Duration::from_secs(5)).unwrap();
    drop(tty);

    assert_eq!(attributes(&pty.observer), before);
}

#[test]
fn terminal_attributes_are_restored_after_a_timeout() {
    let (pty, device) = pty();
    let before = attributes(&pty.observer);
    let mut tty = DevTty::from_fd(device).unwrap();

    let error = query_position(&mut tty, Duration::from_millis(1)).unwrap_err();
    drop(tty);

    assert_eq!(
        (error.kind(), attributes(&pty.observer)),
        (io::ErrorKind::TimedOut, before)
    );
}
