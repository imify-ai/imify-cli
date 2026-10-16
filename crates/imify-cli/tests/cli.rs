use assert_cmd::Command;

#[test]
fn version_flag_prints_the_package_version() {
    let output = Command::cargo_bin("imify")
        .unwrap()
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("imify {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_argument_exits_with_usage_error() {
    Command::cargo_bin("imify")
        .unwrap()
        .arg("--no-such-flag")
        .assert()
        .code(2);
}
