//! Renderers write results, events and errors in the selected output mode.

use std::io::{self, Write};

use super::error::CliError;
use super::sanitize::printable;

/// The `type` of the event that ends an NDJSON stream and carries the result.
pub const RESULT_EVENT: &str = "result";

/// A value the CLI can print as a result or an event.
pub trait Render {
    /// The NDJSON `type` when the value is written as an event.
    fn kind(&self) -> &'static str;
    /// The JSON form. Events are written as this object with `type` added.
    fn json(&self) -> serde_json::Value;
    /// The human-readable form.
    ///
    /// # Errors
    ///
    /// Returns the error of the underlying writer.
    fn human(&self, w: &mut dyn Write) -> io::Result<()>;
}

/// Writes output in one output mode.
///
/// Results go to stdout, errors to stderr. Events go to stdout in JSON mode and to stderr in
/// human mode. An event of type [`RESULT_EVENT`] is the result of a streaming command: it goes
/// to stdout in both modes and is kept with `--quiet`.
pub trait Renderer {
    /// Writes the result of a command that does not stream events.
    ///
    /// # Errors
    ///
    /// Returns the error of the underlying writer.
    fn result(&mut self, value: &dyn Render) -> io::Result<()>;
    /// Writes one event of a streaming command. With `--quiet`, only the result event is written.
    ///
    /// # Errors
    ///
    /// Returns the error of the underlying writer.
    fn event(&mut self, event: &dyn Render) -> io::Result<()>;
    /// Writes an error. Errors are written with `--quiet` too.
    ///
    /// # Errors
    ///
    /// Returns the error of the underlying writer.
    fn error(&mut self, error: &CliError) -> io::Result<()>;
    /// Writes `warning: <message>` to stderr. With `--quiet`, nothing is written.
    ///
    /// # Errors
    ///
    /// Returns the error of the underlying writer.
    fn warning(&mut self, message: &str) -> io::Result<()>;
}

fn write_warning(stderr: &mut dyn Write, quiet: bool, message: &str) -> io::Result<()> {
    if quiet {
        return Ok(());
    }
    writeln!(stderr, "warning: {}", printable(message))?;
    stderr.flush()
}

/// Writes text for people: results on stdout, events and errors on stderr.
pub struct HumanRenderer {
    stdout: Box<dyn Write>,
    stderr: Box<dyn Write>,
    quiet: bool,
}

impl HumanRenderer {
    pub fn new(stdout: Box<dyn Write>, stderr: Box<dyn Write>, quiet: bool) -> Self {
        Self {
            stdout,
            stderr,
            quiet,
        }
    }
}

impl std::fmt::Debug for HumanRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HumanRenderer")
            .field("quiet", &self.quiet)
            .finish_non_exhaustive()
    }
}

impl Renderer for HumanRenderer {
    fn result(&mut self, value: &dyn Render) -> io::Result<()> {
        value.human(&mut self.stdout)?;
        self.stdout.flush()
    }

    fn event(&mut self, event: &dyn Render) -> io::Result<()> {
        if event.kind() == RESULT_EVENT {
            return self.result(event);
        }
        if self.quiet {
            return Ok(());
        }
        event.human(&mut self.stderr)?;
        self.stderr.flush()
    }

    fn error(&mut self, error: &CliError) -> io::Result<()> {
        writeln!(self.stderr, "error: {}", printable(&error.message))?;
        if let Some(hint) = &error.hint {
            writeln!(self.stderr, "hint: {}", printable(hint))?;
        }
        self.stderr.flush()
    }

    fn warning(&mut self, message: &str) -> io::Result<()> {
        write_warning(&mut self.stderr, self.quiet, message)
    }
}

/// Writes JSON for scripts: one document per result, one line per event, the error envelope on
/// stderr.
pub struct JsonRenderer {
    stdout: Box<dyn Write>,
    stderr: Box<dyn Write>,
    quiet: bool,
}

impl JsonRenderer {
    pub fn new(stdout: Box<dyn Write>, stderr: Box<dyn Write>, quiet: bool) -> Self {
        Self {
            stdout,
            stderr,
            quiet,
        }
    }
}

impl std::fmt::Debug for JsonRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonRenderer")
            .field("quiet", &self.quiet)
            .finish_non_exhaustive()
    }
}

fn write_line(w: &mut dyn Write, value: &impl serde::Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *w, value)?;
    w.write_all(b"\n")?;
    w.flush()
}

/// The event as a JSON object with `type` added. A non-object value goes under `data`.
fn event_json(event: &dyn Render) -> serde_json::Value {
    let mut object = match event.json() {
        serde_json::Value::Object(fields) => fields,
        other => serde_json::Map::from_iter([("data".to_owned(), other)]),
    };
    object.insert("type".to_owned(), event.kind().into());
    serde_json::Value::Object(object)
}

impl Renderer for JsonRenderer {
    fn result(&mut self, value: &dyn Render) -> io::Result<()> {
        write_line(&mut self.stdout, &value.json())
    }

    fn event(&mut self, event: &dyn Render) -> io::Result<()> {
        if self.quiet && event.kind() != RESULT_EVENT {
            return Ok(());
        }
        write_line(&mut self.stdout, &event_json(event))
    }

    fn error(&mut self, error: &CliError) -> io::Result<()> {
        write_line(&mut self.stderr, &error.envelope())
    }

    fn warning(&mut self, message: &str) -> io::Result<()> {
        write_warning(&mut self.stderr, self.quiet, message)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::output::ErrorCode;

    #[derive(Clone, Default)]
    struct Buffer(Rc<RefCell<Vec<u8>>>);

    impl Buffer {
        fn text(&self) -> String {
            String::from_utf8(self.0.borrow().clone()).unwrap()
        }
    }

    impl Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct Streams {
        stdout: Buffer,
        stderr: Buffer,
    }

    fn json_renderer(quiet: bool) -> (JsonRenderer, Streams) {
        let streams = Streams {
            stdout: Buffer::default(),
            stderr: Buffer::default(),
        };
        let renderer = JsonRenderer::new(
            Box::new(streams.stdout.clone()),
            Box::new(streams.stderr.clone()),
            quiet,
        );
        (renderer, streams)
    }

    fn human_renderer(quiet: bool) -> (HumanRenderer, Streams) {
        let streams = Streams {
            stdout: Buffer::default(),
            stderr: Buffer::default(),
        };
        let renderer = HumanRenderer::new(
            Box::new(streams.stdout.clone()),
            Box::new(streams.stderr.clone()),
            quiet,
        );
        (renderer, streams)
    }

    struct Progress {
        files_done: u64,
    }

    impl Render for Progress {
        fn kind(&self) -> &'static str {
            "progress"
        }

        fn json(&self) -> serde_json::Value {
            serde_json::json!({ "files_done": self.files_done })
        }

        fn human(&self, w: &mut dyn Write) -> io::Result<()> {
            writeln!(w, "{} files uploaded", self.files_done)
        }
    }

    struct Uploaded {
        dataset_id: u64,
    }

    impl Render for Uploaded {
        fn kind(&self) -> &'static str {
            RESULT_EVENT
        }

        fn json(&self) -> serde_json::Value {
            serde_json::json!({ "dataset_id": self.dataset_id })
        }

        fn human(&self, w: &mut dyn Write) -> io::Result<()> {
            writeln!(w, "dataset {}", self.dataset_id)
        }
    }

    fn stream(renderer: &mut dyn Renderer) {
        renderer.event(&Progress { files_done: 1 }).unwrap();
        renderer.event(&Progress { files_done: 2 }).unwrap();
        renderer.event(&Uploaded { dataset_id: 42 }).unwrap();
    }

    #[test]
    fn json_result_is_one_document_on_stdout() {
        let (mut renderer, streams) = json_renderer(false);

        renderer.result(&Uploaded { dataset_id: 42 }).unwrap();

        assert_eq!(streams.stdout.text(), "{\"dataset_id\":42}\n");
        assert_eq!(streams.stderr.text(), "");
    }

    #[test]
    fn json_events_are_one_line_each_on_stdout_with_a_type() {
        let (mut renderer, streams) = json_renderer(false);

        stream(&mut renderer);

        assert_eq!(streams.stderr.text(), "");
        insta::with_settings!({ snapshot_path => "../../tests/snapshots" }, {
            insta::assert_snapshot!("event_stream_json", streams.stdout.text());
        });
    }

    #[test]
    fn json_event_with_a_non_object_value_carries_it_under_data() {
        struct Tick;
        impl Render for Tick {
            fn kind(&self) -> &'static str {
                "tick"
            }
            fn json(&self) -> serde_json::Value {
                serde_json::json!(3)
            }
            fn human(&self, _: &mut dyn Write) -> io::Result<()> {
                Ok(())
            }
        }
        let (mut renderer, streams) = json_renderer(false);

        renderer.event(&Tick).unwrap();

        assert_eq!(streams.stdout.text(), "{\"data\":3,\"type\":\"tick\"}\n");
    }

    #[test]
    fn json_quiet_writes_the_result_event_and_drops_the_other_events() {
        let (mut renderer, streams) = json_renderer(true);

        stream(&mut renderer);

        assert_eq!(
            streams.stdout.text(),
            "{\"dataset_id\":42,\"type\":\"result\"}\n"
        );
    }

    #[test]
    fn json_error_is_the_envelope_on_stderr() {
        let (mut renderer, streams) = json_renderer(true);

        renderer
            .error(&CliError::new(
                ErrorCode::NotFound,
                "Dataset 7 does not exist.",
            ))
            .unwrap();

        assert_eq!(streams.stdout.text(), "");
        assert_eq!(
            streams.stderr.text(),
            "{\"error\":{\"code\":\"not_found\",\"message\":\"Dataset 7 does not exist.\",\"hint\":null}}\n"
        );
    }

    #[test]
    fn human_events_go_to_stderr_and_the_result_to_stdout() {
        let (mut renderer, streams) = human_renderer(false);

        stream(&mut renderer);

        assert_eq!(
            streams.stderr.text(),
            "1 files uploaded\n2 files uploaded\n"
        );
        assert_eq!(streams.stdout.text(), "dataset 42\n");
    }

    #[test]
    fn human_quiet_writes_the_result_only() {
        let (mut renderer, streams) = human_renderer(true);

        stream(&mut renderer);

        assert_eq!(streams.stderr.text(), "");
        assert_eq!(streams.stdout.text(), "dataset 42\n");
    }

    #[test]
    fn human_error_writes_the_message_and_the_hint_on_stderr() {
        let (mut renderer, streams) = human_renderer(false);

        renderer
            .error(
                &CliError::new(ErrorCode::AuthRequired, "Not logged in.")
                    .with_hint("Run `imify login`."),
            )
            .unwrap();

        assert_eq!(streams.stdout.text(), "");
        assert_eq!(
            streams.stderr.text(),
            "error: Not logged in.\nhint: Run `imify login`.\n"
        );
    }

    #[test]
    fn warnings_go_to_stderr_in_both_modes() {
        let (mut human, human_streams) = human_renderer(false);
        let (mut json, json_streams) = json_renderer(false);

        human.warning("No keychain is available.").unwrap();
        json.warning("No keychain is available.").unwrap();

        assert_eq!(
            [human_streams.stderr.text(), json_streams.stderr.text()],
            [
                "warning: No keychain is available.\n",
                "warning: No keychain is available.\n"
            ]
        );
        assert_eq!(
            human_streams.stdout.text() + &json_streams.stdout.text(),
            ""
        );
    }

    #[test]
    fn quiet_drops_warnings() {
        let (mut renderer, streams) = human_renderer(true);

        renderer.warning("No keychain is available.").unwrap();

        assert_eq!(streams.stderr.text(), "");
    }
}
