//! `imify browse` against a fake browser opener on `PATH`.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Output;
use std::time::{Duration, Instant};

use assert_cmd::Command;

/// The `imify` binary without the environment variables the CLI reads, with `PATH` set to
/// `path`. Logins are read from a file in a directory that does not exist.
fn imify(args: &[&str], path: &OsString) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_imify"));
    command
        .env_remove("IMIFY_OUTPUT")
        .env_remove("NO_COLOR")
        .env_remove("IMIFY_TOKEN")
        .env_remove("IMIFY_API_URL")
        .env_remove("IMIFY_PROFILE")
        .env("IMIFY_CREDENTIAL_STORE", "file")
        .env("IMIFY_CONFIG_DIR", "/nonexistent/imify-browse-tests")
        .env("PATH", path)
        .args(args);
    command.assert().get_output().clone()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A directory with `open` and `xdg-open` scripts, which append their argument to a log file.
struct FakeOpener {
    dir: tempfile::TempDir,
    log: PathBuf,
}

impl FakeOpener {
    fn new() -> io::Result<Self> {
        let dir = tempfile::tempdir()?;
        let log = dir.path().join("opened.log");
        for name in ["open", "xdg-open"] {
            let script = dir.path().join(name);
            fs::write(
                &script,
                format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n", log.display()),
            )?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
            }
        }
        Ok(Self { dir, log })
    }

    /// `PATH` with the scripts first.
    fn path(&self) -> OsString {
        let mut path = self.dir.path().as_os_str().to_owned();
        if let Some(inherited) = std::env::var_os("PATH") {
            path.push(":");
            path.push(inherited);
        }
        path
    }

    /// The URLs the scripts were started with. Waits up to 5 s for the first line, because the
    /// CLI does not wait for the opener.
    fn opened(&self) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(log) = fs::read_to_string(&self.log) {
                return log.lines().map(str::to_owned).collect();
            }
            assert!(Instant::now() < deadline, "the opener was not started");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// `true` when no script was started.
    fn nothing_opened(&self) -> bool {
        !self.log.exists()
    }
}

/// A `PATH` without any opener.
fn empty_path() -> io::Result<(tempfile::TempDir, OsString)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().as_os_str().to_owned();
    Ok((dir, path))
}

#[test]
fn browse_opens_the_default_url_in_the_browser() {
    let opener = FakeOpener::new().unwrap();

    let output = imify(&["browse"], &opener.path());

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("browse_human", text(&output.stdout));
    assert_eq!(opener.opened(), ["https://platform.imify.ai"]);
}

#[test]
fn browse_json_reports_the_url_and_that_the_browser_was_opened() {
    let opener = FakeOpener::new().unwrap();

    let output = imify(&["browse", "--json"], &opener.path());

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("browse_json", text(&output.stdout));
    assert_eq!(opener.opened(), ["https://platform.imify.ai"]);
}

#[test]
fn browse_opens_the_api_url_of_the_flag_without_a_trailing_slash() {
    let opener = FakeOpener::new().unwrap();

    let output = imify(
        &["browse", "--api-url", "https://api.example.com/"],
        &opener.path(),
    );

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "Opening https://api.example.com in the browser.\n"
    );
    assert_eq!(opener.opened(), ["https://api.example.com"]);
}

#[test]
fn no_browser_prints_the_url_and_opens_nothing() {
    let opener = FakeOpener::new().unwrap();

    let output = imify(&["browse", "--no-browser"], &opener.path());

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "https://platform.imify.ai\n");
    assert_eq!(text(&output.stderr), "");
    assert!(opener.nothing_opened());
}

#[test]
fn no_browser_json_reports_that_no_browser_was_opened() {
    let opener = FakeOpener::new().unwrap();

    let output = imify(&["browse", "--no-browser", "--json"], &opener.path());

    assert!(output.status.success(), "{}", text(&output.stderr));
    insta::assert_snapshot!("browse_no_browser_json", text(&output.stdout));
    assert!(opener.nothing_opened());
}

#[test]
fn browse_without_an_opener_fails_with_internal_and_names_the_url() {
    let (_dir, path) = empty_path().unwrap();

    let output = imify(&["browse"], &path);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stdout), "");
    let stderr = text(&output.stderr);
    assert!(
        stderr.starts_with("error: Cannot open the browser: "),
        "{stderr}"
    );
    assert!(
        stderr.ends_with(
            "hint: Open https://platform.imify.ai in a browser, or run `imify browse --no-browser` to print the URL.\n"
        ),
        "{stderr}"
    );
}

#[test]
fn browse_without_an_opener_fails_with_the_internal_envelope() {
    let (_dir, path) = empty_path().unwrap();

    let output = imify(&["browse", "--json"], &path);

    assert_eq!(output.status.code(), Some(1));
    let envelope: serde_json::Value = serde_json::from_str(&text(&output.stderr)).unwrap();
    assert_eq!(envelope["error"]["code"], "internal");
    assert_eq!(
        envelope["error"]["hint"],
        "Open https://platform.imify.ai in a browser, or run `imify browse --no-browser` to print the URL."
    );
}

#[test]
fn browse_with_an_invalid_api_url_is_a_usage_error_and_opens_nothing() {
    let opener = FakeOpener::new().unwrap();

    let output = imify(
        &["browse", "--api-url", "ftp://platform.imify.ai", "--json"],
        &opener.path(),
    );

    assert_eq!(output.status.code(), Some(2));
    let envelope: serde_json::Value = serde_json::from_str(&text(&output.stderr)).unwrap();
    assert_eq!(envelope["error"]["code"], "invalid_argument");
    assert!(opener.nothing_opened());
}
