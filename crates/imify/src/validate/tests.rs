use std::path::Path;

use imify_test_support::{Compression, DatasetBuilder, Defect, Format, SyntheticDataset};

use super::template::{Arg, Template};
use super::*;

const FORMATS: [Format; 2] = [Format::V2_1, Format::V3_0];

fn build(format: Format, defects: &[Defect]) -> SyntheticDataset {
    defects
        .iter()
        .fold(DatasetBuilder::new(format), |builder, defect| {
            builder.defect(defect.clone())
        })
        .build()
        .unwrap()
}

fn report_of(dataset: &SyntheticDataset) -> Report {
    validate_dataset(dataset.root()).unwrap()
}

fn validate(format: Format, defect: Defect) -> Report {
    report_of(&build(format, &[defect]))
}

fn codes(report: &Report) -> Vec<FindingCode> {
    report.findings.iter().map(|finding| finding.code).collect()
}

/// The path of each finding, or an empty string for a finding without a path.
fn paths(report: &Report) -> Vec<String> {
    report
        .findings
        .iter()
        .map(|finding| {
            finding
                .path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        })
        .collect()
}

#[test]
fn a_default_v2_1_dataset_has_no_findings() {
    let report = report_of(&build(Format::V2_1, &[]));

    assert_eq!(report.findings, []);
    assert_eq!(report.codebase_version.as_deref(), Some("v2.1"));
    assert!(report.is_valid());
}

#[test]
fn a_default_v3_0_dataset_has_no_findings() {
    let report = report_of(&build(Format::V3_0, &[]));

    assert_eq!(report.findings, []);
    assert_eq!(report.codebase_version.as_deref(), Some("v3.0"));
}

#[test]
fn datasets_without_cameras_have_no_findings() {
    for format in FORMATS {
        let dataset = DatasetBuilder::new(format).cameras(&[]).build().unwrap();

        assert_eq!(report_of(&dataset).findings, [], "{format:?}");
    }
}

#[test]
fn snappy_compressed_data_is_checked() {
    for format in FORMATS {
        let dataset = DatasetBuilder::new(format)
            .data_compression(Compression::Snappy)
            .defect(Defect::EpisodeLength)
            .build()
            .unwrap();

        assert_eq!(
            codes(&report_of(&dataset)),
            [FindingCode::EpisodeLengthMismatch],
            "{format:?}"
        );
    }
}

#[test]
fn a_root_that_does_not_exist_is_an_io_error() {
    let dataset = build(Format::V2_1, &[]);
    let missing = dataset.root().join("no-such-dataset");

    let error = validate_dataset(&missing).unwrap_err();

    assert!(matches!(error, Error::Io { ref path, .. } if *path == missing));
}

#[test]
fn a_root_that_is_a_file_is_an_io_error() {
    let dataset = build(Format::V2_1, &[]);

    let error = validate_dataset(&dataset.root().join("meta/info.json")).unwrap_err();

    assert!(matches!(error, Error::Io { .. }));
}

#[test]
fn v2_1_missing_info_is_the_only_finding() {
    let report = validate(Format::V2_1, Defect::MissingInfo);

    assert_eq!(codes(&report), [FindingCode::InfoMissing]);
    assert_eq!(paths(&report), ["meta/info.json"]);
    assert_eq!(report.codebase_version, None);
    assert!(!report.is_valid());
}

#[test]
fn v3_0_missing_info_is_the_only_finding() {
    let report = validate(Format::V3_0, Defect::MissingInfo);

    assert_eq!(codes(&report), [FindingCode::InfoMissing]);
}

#[test]
fn malformed_info_is_info_malformed() {
    for format in FORMATS {
        let report = validate(format, Defect::MalformedInfo);

        assert_eq!(codes(&report), [FindingCode::InfoMalformed], "{format:?}");
    }
}

#[test]
fn codebase_version_v2_0_is_unsupported() {
    for format in FORMATS {
        let report = validate(format, Defect::CodebaseVersion("v2.0".to_owned()));

        assert_eq!(
            codes(&report),
            [FindingCode::CodebaseVersionUnsupported],
            "{format:?}"
        );
        assert_eq!(report.codebase_version.as_deref(), Some("v2.0"));
    }
}

#[test]
fn codebase_version_v3_1_is_accepted() {
    let report = validate(Format::V3_0, Defect::CodebaseVersion("v3.1".to_owned()));

    assert_eq!(report.findings, []);
}

#[test]
fn fps_zero_and_negative_are_fps_invalid() {
    for format in FORMATS {
        for fps in [0, -1] {
            let report = validate(format, Defect::Fps(fps));

            assert_eq!(
                codes(&report),
                [FindingCode::FpsInvalid],
                "{format:?} {fps}"
            );
        }
    }
}

#[test]
fn v2_1_corrupt_data_file_is_parquet_unreadable() {
    let report = validate(Format::V2_1, Defect::CorruptDataFile);

    assert_eq!(codes(&report), [FindingCode::ParquetUnreadable]);
    assert_eq!(paths(&report), ["data/chunk-000/episode_000000.parquet"]);
}

#[test]
fn v3_0_corrupt_data_file_is_parquet_unreadable() {
    let report = validate(Format::V3_0, Defect::CorruptDataFile);

    assert_eq!(codes(&report), [FindingCode::ParquetUnreadable]);
    assert_eq!(paths(&report), ["data/chunk-000/file-000.parquet"]);
}

#[test]
fn v2_1_missing_data_file_is_data_file_missing() {
    let report = validate(Format::V2_1, Defect::MissingDataFile);

    assert_eq!(codes(&report), [FindingCode::DataFileMissing]);
    assert_eq!(paths(&report), ["data/chunk-000/episode_000000.parquet"]);
}

#[test]
fn v3_0_missing_data_file_is_data_file_missing() {
    let report = validate(Format::V3_0, Defect::MissingDataFile);

    assert_eq!(codes(&report), [FindingCode::DataFileMissing]);
    assert_eq!(paths(&report), ["data/chunk-000/file-000.parquet"]);
}

#[test]
fn v2_1_episode_length_is_episode_length_mismatch() {
    let report = validate(Format::V2_1, Defect::EpisodeLength);

    assert_eq!(codes(&report), [FindingCode::EpisodeLengthMismatch]);
    assert_eq!(
        report.findings[0].message,
        "Episode 0 has 10 rows; meta/episodes.jsonl declares 11."
    );
}

#[test]
fn v3_0_episode_length_is_episode_length_mismatch() {
    let report = validate(Format::V3_0, Defect::EpisodeLength);

    assert_eq!(codes(&report), [FindingCode::EpisodeLengthMismatch]);
    assert_eq!(paths(&report), ["data/chunk-000/file-000.parquet"]);
}

#[test]
fn v2_1_non_monotonic_timestamps_are_timestamps_not_monotonic() {
    let report = validate(Format::V2_1, Defect::NonMonotonicTimestamps);

    assert_eq!(codes(&report), [FindingCode::TimestampsNotMonotonic]);
}

#[test]
fn v3_0_non_monotonic_timestamps_are_timestamps_not_monotonic() {
    let report = validate(Format::V3_0, Defect::NonMonotonicTimestamps);

    assert_eq!(codes(&report), [FindingCode::TimestampsNotMonotonic]);
}

#[test]
fn v2_1_timestamp_gap_is_a_timestamp_spacing_warning() {
    let report = validate(Format::V2_1, Defect::TimestampGap);

    assert_eq!(codes(&report), [FindingCode::TimestampSpacing]);
    assert!(report.is_valid());
}

#[test]
fn v3_0_timestamp_gap_is_a_timestamp_spacing_warning() {
    let report = validate(Format::V3_0, Defect::TimestampGap);

    assert_eq!(codes(&report), [FindingCode::TimestampSpacing]);
    assert!(report.is_valid());
}

#[test]
fn a_stray_file_is_an_unexpected_entry_warning() {
    for format in FORMATS {
        let report = validate(format, Defect::StrayFile("notes.txt".to_owned()));

        assert_eq!(codes(&report), [FindingCode::UnexpectedEntry], "{format:?}");
        assert_eq!(paths(&report), ["notes.txt"]);
        assert!(report.is_valid());
    }
}

#[test]
fn a_hidden_entry_is_not_a_finding() {
    for format in FORMATS {
        let report = validate(format, Defect::StrayFile(".DS_Store".to_owned()));

        assert_eq!(report.findings, [], "{format:?}");
    }
}

#[test]
fn total_episodes_other_than_the_index_is_an_episode_count_mismatch_warning() {
    for format in FORMATS {
        let report = validate(format, Defect::TotalEpisodes(5));

        assert_eq!(
            codes(&report),
            [FindingCode::EpisodeCountMismatch],
            "{format:?}"
        );
        assert!(report.is_valid());
    }
}

#[test]
fn v2_1_missing_episodes_index_skips_the_data_and_video_checks() {
    let report = report_of(&build(
        Format::V2_1,
        &[
            Defect::MissingEpisodesIndex,
            Defect::MissingDataFile,
            Defect::MissingVideo,
        ],
    ));

    assert_eq!(codes(&report), [FindingCode::EpisodesIndexUnreadable]);
    assert_eq!(paths(&report), ["meta/episodes.jsonl"]);
    assert!(report.is_valid());
}

#[test]
fn v3_0_missing_episodes_index_skips_the_data_and_video_checks() {
    let report = report_of(&build(
        Format::V3_0,
        &[
            Defect::MissingEpisodesIndex,
            Defect::MissingDataFile,
            Defect::MissingVideo,
        ],
    ));

    assert_eq!(codes(&report), [FindingCode::EpisodesIndexUnreadable]);
    assert_eq!(paths(&report), ["meta/episodes"]);
}

#[test]
fn v2_1_missing_video_is_a_video_missing_warning() {
    let report = validate(Format::V2_1, Defect::MissingVideo);

    assert_eq!(codes(&report), [FindingCode::VideoMissing]);
    assert_eq!(
        paths(&report),
        ["videos/chunk-000/observation.images.front/episode_000000.mp4"]
    );
    assert!(report.is_valid());
}

#[test]
fn v3_0_missing_video_is_a_video_missing_warning() {
    let report = validate(Format::V3_0, Defect::MissingVideo);

    assert_eq!(codes(&report), [FindingCode::VideoMissing]);
    assert_eq!(
        paths(&report),
        ["videos/observation.images.front/chunk-000/file-000.mp4"]
    );
}

#[test]
fn gzip_compressed_data_is_content_not_checked_instead_of_unreadable() {
    for format in FORMATS {
        let dataset = DatasetBuilder::new(format)
            .data_compression(Compression::Gzip)
            .defect(Defect::EpisodeLength)
            .build()
            .unwrap();

        let report = report_of(&dataset);

        assert!(
            codes(&report)
                .iter()
                .all(|code| *code == FindingCode::ContentNotChecked),
            "{format:?}: {report:?}"
        );
        assert!(codes(&report).contains(&FindingCode::ContentNotChecked));
        assert!(report.is_valid());
    }
}

#[test]
fn findings_follow_the_check_order() {
    let report = report_of(&build(
        Format::V2_1,
        &[
            Defect::MissingVideo,
            Defect::EpisodeLength,
            Defect::TotalEpisodes(5),
            Defect::StrayFile("notes.txt".to_owned()),
        ],
    ));

    assert_eq!(
        codes(&report),
        [
            FindingCode::UnexpectedEntry,
            FindingCode::EpisodeCountMismatch,
            FindingCode::EpisodeLengthMismatch,
            FindingCode::VideoMissing,
        ]
    );
}

#[test]
fn errors_and_warnings_split_the_findings_by_severity() {
    let report = report_of(&build(
        Format::V3_0,
        &[
            Defect::EpisodeLength,
            Defect::StrayFile("notes.txt".to_owned()),
        ],
    ));

    let errors: Vec<FindingCode> = report.errors().map(|finding| finding.code).collect();
    let warnings: Vec<FindingCode> = report.warnings().map(|finding| finding.code).collect();

    assert_eq!(errors, [FindingCode::EpisodeLengthMismatch]);
    assert_eq!(warnings, [FindingCode::UnexpectedEntry]);
}

fn write_info(dataset: &SyntheticDataset, edit: impl FnOnce(&mut serde_json::Value)) {
    let path = dataset.root().join("meta/info.json");
    let mut info: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    edit(&mut info);
    std::fs::write(path, serde_json::to_vec(&info).unwrap()).unwrap();
}

#[test]
fn an_unknown_placeholder_in_data_path_is_info_malformed() {
    let dataset = build(Format::V2_1, &[]);
    write_info(&dataset, |info| {
        info["data_path"] = "data/{split}/episode_{episode_index:06d}.parquet".into();
    });

    let report = report_of(&dataset);

    assert_eq!(codes(&report), [FindingCode::InfoMalformed]);
}

#[test]
fn a_missing_required_info_field_is_info_malformed() {
    for field in ["features", "data_path", "chunks_size", "fps"] {
        let dataset = build(Format::V3_0, &[]);
        write_info(&dataset, |info| {
            info.as_object_mut().unwrap().remove(field);
        });

        let report = report_of(&dataset);

        assert_eq!(codes(&report), [FindingCode::InfoMalformed], "{field}");
    }
}

#[test]
fn template_pads_integers_to_the_declared_width() {
    let template = Template::parse(
        "data/chunk-{episode_chunk:03d}/episode_{episode_index:06d}.parquet",
        &["episode_chunk", "episode_index"],
    )
    .unwrap();

    let path = template.expand(&[
        ("episode_chunk", Arg::Int(1)),
        ("episode_index", Arg::Int(1042)),
    ]);

    assert_eq!(path, Path::new("data/chunk-001/episode_001042.parquet"));
}

#[test]
fn template_inserts_strings_as_they_are() {
    let template = Template::parse("videos/{video_key}/file.mp4", &["video_key"]).unwrap();

    let path = template.expand(&[("video_key", Arg::Str("observation.images.front"))]);

    assert_eq!(path, Path::new("videos/observation.images.front/file.mp4"));
}

#[test]
fn template_rejects_an_unknown_placeholder() {
    let error = Template::parse("data/{split}.parquet", &["episode_index"]).unwrap_err();

    assert_eq!(error, "unknown placeholder {split}");
}

#[test]
fn template_rejects_an_unsupported_format_spec_and_unbalanced_braces() {
    for text in ["{episode_index:x}", "{episode_index", "episode_index}"] {
        assert!(Template::parse(text, &["episode_index"]).is_err(), "{text}");
    }
}

/// The finding code table of `docs/guide/datasets-validate.md`: `(code, severity)` per row.
fn guide_table() -> Vec<(String, String)> {
    let guide = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/guide/datasets-validate.md"
    ))
    .unwrap();
    guide
        .lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            let code = cells.get(1)?.strip_prefix('`')?.strip_suffix('`')?;
            Some(((*code).to_owned(), (*cells.get(2)?).to_owned()))
        })
        .filter(|(code, _)| FindingCode::ALL.iter().any(|known| known.as_str() == code))
        .collect()
}

#[test]
fn finding_codes_and_severities_match_the_guide() {
    let codes: Vec<(String, String)> = FindingCode::ALL
        .iter()
        .map(|code| {
            (
                code.as_str().to_owned(),
                code.severity().as_str().to_owned(),
            )
        })
        .collect();

    assert_eq!(guide_table(), codes);
}

#[test]
fn finding_codes_match_the_report_schema() {
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/reference/schemas/datasets-validate.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let codes: Vec<&str> = FindingCode::ALL.iter().map(|code| code.as_str()).collect();

    assert_eq!(
        schema["$defs"]["finding"]["properties"]["code"]["enum"],
        serde_json::json!(codes)
    );
}
