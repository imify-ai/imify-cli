use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Each schema in `docs/reference/schemas/` and the snapshots whose lines it validates.
const CASES: &[(&str, &[&str])] = &[
    (
        "error.json",
        &[
            "cli__usage_error_json.snap",
            "cli__invalid_imify_output_json.snap",
            "cli__whoami_auth_required_json.snap",
            "cli__whoami_auth_rejected_json.snap",
            "cli__datasets_validate_invalid_envelope_json.snap",
            "cli__datasets_upload_rejected_json.snap",
        ],
    ),
    (
        "datasets-validate.json",
        &[
            "cli__datasets_validate_valid_json.snap",
            "cli__datasets_validate_invalid_json.snap",
        ],
    ),
    (
        "event.json",
        &["imify__output__renderer__tests__event_stream_json.snap"],
    ),
    (
        "datasets-ls.json",
        &[
            "cli__datasets_ls_json.snap",
            "cli__datasets_ls_empty_json.snap",
        ],
    ),
    (
        "datasets-upload.json",
        &[
            "cli__datasets_upload_json.snap",
            "cli__datasets_upload_dry_run_json.snap",
        ],
    ),
    ("whoami.json", &["cli__whoami_json.snap"]),
];

fn schemas_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference/schemas")
}

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(name)
}

/// The snapshot content after the insta header with LF line endings, or `None` when the header
/// is missing.
fn snapshot_body(snapshot: &str) -> Option<String> {
    let snapshot = snapshot.replace("\r\n", "\n");
    let (_, body) = snapshot.strip_prefix("---\n")?.split_once("\n---\n")?;
    Some(body.to_owned())
}

#[test]
fn snapshot_body_reads_a_snapshot_with_crlf_line_endings() {
    let snapshot = "---\r\nsource: tests/cli.rs\r\n---\r\n{\"type\":\"result\"}\r\n";

    assert_eq!(
        snapshot_body(snapshot).as_deref(),
        Some("{\"type\":\"result\"}\n")
    );
}

#[test]
fn every_schema_file_has_a_case() {
    let files: BTreeSet<String> = fs::read_dir(schemas_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    let cases: BTreeSet<String> = CASES
        .iter()
        .map(|(schema, _)| (*schema).to_owned())
        .collect();

    assert_eq!(files, cases);
}

#[test]
fn every_snapshot_line_validates_against_its_schema() {
    for (schema_file, snapshots) in CASES {
        let schema: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(schemas_dir().join(schema_file)).unwrap())
                .unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();

        for snapshot in *snapshots {
            let content = fs::read_to_string(snapshot_path(snapshot)).unwrap();
            let body =
                snapshot_body(&content).unwrap_or_else(|| panic!("{snapshot} has no insta header"));
            for line in body.lines().filter(|line| !line.is_empty()) {
                let instance: serde_json::Value = serde_json::from_str(line).unwrap();
                let errors: Vec<String> = validator
                    .iter_errors(&instance)
                    .map(|error| error.to_string())
                    .collect();
                assert!(
                    errors.is_empty(),
                    "{snapshot} does not match {schema_file}: {errors:?}"
                );
            }
        }
    }
}
