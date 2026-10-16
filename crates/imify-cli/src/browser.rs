//! Opens a URL in the user's web browser.

use std::io;
use std::process::{Command, Stdio};

/// Opens URLs in a web browser.
pub trait Browser {
    /// Starts opening `url` and returns without waiting for the browser.
    ///
    /// # Errors
    ///
    /// Returns the error of starting the opener.
    fn open(&self, url: &str) -> io::Result<()>;
}

/// The system's default browser, through `open` on macOS and `xdg-open` on Linux. The URL is
/// one argument; no shell is involved. The opener's output is discarded.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemBrowser;

#[cfg(target_os = "macos")]
const OPENER: &str = "open";

#[cfg(not(target_os = "macos"))]
const OPENER: &str = "xdg-open";

impl Browser for SystemBrowser {
    fn open(&self, url: &str) -> io::Result<()> {
        let mut child = Command::new(OPENER)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // Reaps the opener when it exits; the login does not wait for it.
        std::thread::spawn(move || child.wait());
        Ok(())
    }
}
