use std::process::Output;

use assert_cmd::Command;

fn imify(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_imify"));
    command.env_remove("IMIFY_OUTPUT").env_remove("NO_COLOR");
    for (key, value) in env {
        command.env(key, value);
    }
    command.args(args).assert().get_output().clone()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn version_flag_prints_the_package_version() {
    let output = imify(&["--version"], &[]);

    assert!(output.status.success());
    assert_eq!(
        text(&output.stdout),
        format!("imify {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_argument_exits_with_usage_error() {
    let output = imify(&["--no-such-flag"], &[]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn json_flag_renders_a_usage_error_as_an_envelope_on_stderr() {
    let output = imify(&["--json", "--no-such-flag"], &[]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(text(&output.stdout), "");
    insta::assert_snapshot!("usage_error_json", text(&output.stderr));
}

#[test]
fn imify_output_json_renders_the_same_envelope_as_the_json_flag() {
    let from_flag = imify(&["--json", "--no-such-flag"], &[]);
    let from_env = imify(&["--no-such-flag"], &[("IMIFY_OUTPUT", "json")]);

    assert_eq!(from_env.status.code(), Some(2));
    assert_eq!(text(&from_env.stderr), text(&from_flag.stderr));
}

#[test]
fn human_mode_renders_a_usage_error_as_text_on_stderr() {
    let output = imify(&["--no-such-flag"], &[]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(text(&output.stdout), "");
    insta::assert_snapshot!("usage_error_human", text(&output.stderr));
}

#[test]
fn json_flag_after_the_separator_does_not_select_json_output() {
    let output = imify(&["--no-such-flag", "--", "--json"], &[]);

    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).starts_with("error: "));
}

#[test]
fn unsupported_imify_output_value_is_a_usage_error() {
    let output = imify(&["--json"], &[("IMIFY_OUTPUT", "xml")]);

    assert_eq!(output.status.code(), Some(2));
    insta::assert_snapshot!("invalid_imify_output_json", text(&output.stderr));
}

#[test]
fn quiet_and_verbose_together_are_a_usage_error() {
    let output = imify(&["--quiet", "-v"], &[]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn global_flags_are_accepted_without_a_command() {
    let output = imify(&["--json", "--yes", "--no-color", "-vv"], &[]);

    assert!(output.status.success());
    assert_eq!(text(&output.stdout), "");
}
