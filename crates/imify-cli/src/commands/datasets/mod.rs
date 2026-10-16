//! Dataset commands.

mod journal;
pub mod upload;

use std::cmp::Ordering;
use std::io::{self, Write};
use std::path::Path;

use imify::datasets::Dataset;
use imify::validate::{Finding, Report};

use super::{client, write_result};
use crate::cli::{LsArgs, SortField, ValidateArgs};
use crate::output::renderer::{Render, Renderer};
use crate::output::sanitize::printable;
use crate::output::table::{Column, Table};
use crate::output::{CliError, Environment, ErrorCode};

/// `imify datasets ls`: prints the datasets that match the filters, in the requested order.
///
/// # Errors
///
/// Returns the error of building the client, of the API request, or of writing the result.
pub async fn ls(
    env: &Environment,
    args: &LsArgs,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    let api = client(env).await?;
    let datasets = api
        .client
        .datasets()
        .list()
        .await
        .map_err(|error| api.error(error))?;
    write_result(renderer, &DatasetList(select(datasets, args)))
}

/// Keeps the datasets that match every filter in `args` and orders them.
fn select(datasets: Vec<Dataset>, args: &LsArgs) -> Vec<Dataset> {
    let mut selected: Vec<Dataset> = datasets
        .into_iter()
        .filter(|dataset| passes_filters(dataset, args))
        .collect();
    selected.sort_by(|a, b| compare(a, b, args.sort));
    if args.reverse {
        selected.reverse();
    }
    selected
}

fn passes_filters(dataset: &Dataset, args: &LsArgs) -> bool {
    let task = args
        .task
        .as_ref()
        .is_none_or(|text| dataset.task.to_lowercase().contains(&text.to_lowercase()));
    let format = args
        .format
        .as_ref()
        .is_none_or(|format| dataset.format == *format);
    let embodiment = args
        .embodiment
        .as_ref()
        .is_none_or(|embodiment| dataset.embodiment == *embodiment);
    task && format && embodiment
}

/// Orders by `field`, then by id. Without a field: by task, then embodiment, then id.
fn compare(a: &Dataset, b: &Dataset, field: Option<SortField>) -> Ordering {
    let primary = match field {
        None => a
            .task
            .cmp(&b.task)
            .then_with(|| a.embodiment.cmp(&b.embodiment)),
        Some(SortField::Id) => Ordering::Equal,
        Some(SortField::Task) => a.task.cmp(&b.task),
        Some(SortField::Format) => a.format.cmp(&b.format),
        Some(SortField::Embodiment) => a.embodiment.cmp(&b.embodiment),
        Some(SortField::Episodes) => a.episodes.cmp(&b.episodes),
        Some(SortField::Accepted) => a.accepted.cmp(&b.accepted),
    };
    primary.then_with(|| a.id.cmp(&b.id))
}

/// The result of `imify datasets ls`.
struct DatasetList(Vec<Dataset>);

impl Render for DatasetList {
    fn kind(&self) -> &'static str {
        "datasets"
    }

    fn json(&self) -> serde_json::Value {
        self.0.iter().map(dataset_json).collect()
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        if self.0.is_empty() {
            return writeln!(w, "No datasets.");
        }
        let mut table = Table::new(vec![
            Column::left("ID"),
            Column::left("TASK"),
            Column::left("FORMAT"),
            Column::left("EMBODIMENT"),
            Column::right("EPISODES"),
            Column::right("ACCEPTED"),
        ]);
        for dataset in &self.0 {
            table.row(vec![
                dataset.id.clone(),
                dataset.task.clone(),
                dataset.format.clone(),
                dataset.embodiment.clone(),
                dataset.episodes.to_string(),
                dataset.accepted.to_string(),
            ]);
        }
        table.write(w)
    }
}

fn dataset_json(dataset: &Dataset) -> serde_json::Value {
    serde_json::json!({
        "id": dataset.id,
        "task": dataset.task,
        "format": dataset.format,
        "embodiment": dataset.embodiment,
        "embodiment_fingerprint": dataset.embodiment_fingerprint,
        "cameras": dataset.cameras,
        "action_dim": dataset.action_dim,
        "created_at": dataset.created_at,
        "episodes": dataset.episodes,
        "accepted": dataset.accepted,
    })
}

/// Separates the columns of the human report.
const COLUMN_GAP: &str = "  ";

/// `imify datasets validate <path>`: checks a local dataset and prints the report.
///
/// # Errors
///
/// Returns `invalid_argument` when `<path>` is not a readable directory, `dataset_invalid`
/// after printing a report with at least one error, and `internal` when stdout cannot be
/// written.
pub fn validate(args: &ValidateArgs, renderer: &mut dyn Renderer) -> Result<(), CliError> {
    let report = imify::validate::validate_dataset(&args.path).map_err(dataset_root_error)?;
    write_result(renderer, &ValidationReport(&report))?;
    match invalid_report_error(&report, &args.path) {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// The `dataset_invalid` error for a report with at least one error, or `None`.
fn invalid_report_error(report: &Report, path: &Path) -> Option<CliError> {
    let errors = report.errors().count();
    if errors == 0 {
        return None;
    }
    let error = CliError::new(
        ErrorCode::DatasetInvalid,
        format!("The dataset has {}.", count(errors, "error")),
    )
    .with_hint(format!(
        "Fix the errors listed in the report and run `imify datasets validate {}` again.",
        path.display()
    ));
    Some(error)
}

/// Maps an error of reading the dataset root: [`imify::Error::Io`] becomes
/// `invalid_argument` with a hint naming the dataset root.
fn dataset_root_error(error: imify::Error) -> CliError {
    match error {
        imify::Error::Io { path, source } => unreadable_root(&path, &source),
        other => other.into(),
    }
}

fn unreadable_root(path: &Path, source: &io::Error) -> CliError {
    CliError::new(
        ErrorCode::InvalidArgument,
        format!("Cannot read the directory {}: {source}.", path.display()),
    )
    .with_hint("Pass the dataset root: the directory that contains `meta/info.json`.")
}

/// `n` followed by `noun`, with an `s` unless `n` is 1.
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// The result of `imify datasets validate`.
struct ValidationReport<'a>(&'a Report);

/// A finding as a JSON object: `code`, `severity`, `path` and `message`.
fn finding_json(finding: &Finding) -> serde_json::Value {
    serde_json::json!({
        "code": finding.code.as_str(),
        "severity": finding.severity().as_str(),
        "path": finding.path.as_deref().map(|path| path.display().to_string()),
        "message": finding.message,
    })
}

/// Writes one line per finding: severity, code, path and message in aligned columns.
fn write_findings(w: &mut dyn Write, findings: &[Finding]) -> io::Result<()> {
    let rows: Vec<[String; 3]> = findings
        .iter()
        .map(|finding| {
            [
                finding.severity().as_str().to_owned(),
                finding.code.as_str().to_owned(),
                finding.path.as_deref().map_or_else(
                    || "-".to_owned(),
                    |path| printable(&path.display().to_string()).into_owned(),
                ),
            ]
        })
        .collect();
    let widths: [usize; 3] = std::array::from_fn(|column| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .max()
            .unwrap_or(0)
    });
    for (row, finding) in rows.iter().zip(findings) {
        for (cell, width) in row.iter().zip(widths) {
            // Padded by hand: a format width above `u16::MAX` panics.
            let padding = width.saturating_sub(cell.chars().count());
            write!(w, "{cell}{}{COLUMN_GAP}", " ".repeat(padding))?;
        }
        writeln!(w, "{}", printable(&finding.message))?;
    }
    Ok(())
}

impl Render for ValidationReport<'_> {
    fn kind(&self) -> &'static str {
        "validation_report"
    }

    fn json(&self) -> serde_json::Value {
        let findings: Vec<serde_json::Value> = self.0.findings.iter().map(finding_json).collect();
        serde_json::json!({
            "codebase_version": self.0.codebase_version,
            "valid": self.0.is_valid(),
            "findings": findings,
        })
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        write_findings(w, &self.0.findings)?;
        writeln!(
            w,
            "{}, {}",
            count(self.0.errors().count(), "error"),
            count(self.0.warnings().count(), "warning")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dataset as the SDK decodes it from an API response.
    fn dataset(id: &str, task: &str, embodiment: &str, episodes: u64) -> Dataset {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "task_description": task,
            "dataset_format": "lerobot_v3",
            "embodiment_label": embodiment,
            "embodiment_fingerprint": format!("fp-{embodiment}"),
            "cameras": ["top"],
            "action_dim": 6,
            "created_at": "2026-10-01T09:30:00Z",
            "episode_count": episodes,
            "accepted_count": episodes,
        }))
        .unwrap()
    }

    fn with_format(mut dataset: Dataset, format: &str) -> Dataset {
        format.clone_into(&mut dataset.format);
        dataset
    }

    fn ids(datasets: &[Dataset]) -> Vec<&str> {
        datasets.iter().map(|dataset| dataset.id.as_str()).collect()
    }

    fn sorted_by(field: SortField) -> LsArgs {
        LsArgs {
            sort: Some(field),
            ..LsArgs::default()
        }
    }

    #[test]
    fn default_order_is_task_then_embodiment_then_id() {
        let datasets = vec![
            dataset("d", "Stack", "koch", 1),
            dataset("c", "Pick", "so100", 1),
            dataset("b", "Pick", "koch", 1),
            dataset("a", "Pick", "so100", 1),
        ];

        let selected = select(datasets, &LsArgs::default());

        assert_eq!(ids(&selected), ["b", "a", "c", "d"]);
    }

    #[test]
    fn sort_by_episodes_orders_numerically() {
        let datasets = vec![
            dataset("a", "Pick", "so100", 10),
            dataset("b", "Pick", "so100", 9),
        ];

        let selected = select(datasets, &sorted_by(SortField::Episodes));

        assert_eq!(ids(&selected), ["b", "a"]);
    }

    #[test]
    fn sort_orders_ties_by_id() {
        let datasets = vec![
            dataset("b", "Stack", "so100", 5),
            dataset("a", "Pick", "so100", 5),
        ];

        let selected = select(datasets, &sorted_by(SortField::Episodes));

        assert_eq!(ids(&selected), ["a", "b"]);
    }

    #[test]
    fn reverse_reverses_the_order_including_ties() {
        let datasets = vec![
            dataset("a", "Pick", "so100", 5),
            dataset("c", "Pick", "so100", 9),
            dataset("b", "Pick", "so100", 5),
        ];
        let args = LsArgs {
            reverse: true,
            ..sorted_by(SortField::Episodes)
        };

        let selected = select(datasets, &args);

        assert_eq!(ids(&selected), ["c", "b", "a"]);
    }

    #[test]
    fn task_filter_matches_a_substring_regardless_of_case() {
        let datasets = vec![
            dataset("a", "Pick up the red CUBE", "so100", 1),
            dataset("b", "Stack two blocks", "so100", 1),
        ];
        let args = LsArgs {
            task: Some("cube".into()),
            ..LsArgs::default()
        };

        let selected = select(datasets, &args);

        assert_eq!(ids(&selected), ["a"]);
    }

    #[test]
    fn format_filter_matches_the_whole_value_only() {
        let datasets = vec![
            with_format(dataset("a", "Pick", "so100", 1), "lerobot_v3"),
            with_format(dataset("b", "Pick", "so100", 1), "lerobot_v3_1"),
        ];
        let args = LsArgs {
            format: Some("lerobot_v3".into()),
            ..LsArgs::default()
        };

        let selected = select(datasets, &args);

        assert_eq!(ids(&selected), ["a"]);
    }

    #[test]
    fn embodiment_filter_matches_the_label_not_the_fingerprint() {
        let datasets = vec![
            dataset("a", "Pick", "so100", 1),
            dataset("b", "Pick", "koch", 1),
        ];
        let by_label = LsArgs {
            embodiment: Some("so100".into()),
            ..LsArgs::default()
        };
        let by_fingerprint = LsArgs {
            embodiment: Some("fp-so100".into()),
            ..LsArgs::default()
        };

        assert_eq!(ids(&select(datasets.clone(), &by_label)), ["a"]);
        assert_eq!(ids(&select(datasets, &by_fingerprint)), Vec::<&str>::new());
    }

    #[test]
    fn two_filters_keep_only_datasets_that_match_both() {
        let datasets = vec![
            dataset("a", "Pick", "so100", 1),
            dataset("b", "Pick", "koch", 1),
            dataset("c", "Stack", "so100", 1),
        ];
        let args = LsArgs {
            task: Some("pick".into()),
            embodiment: Some("so100".into()),
            ..LsArgs::default()
        };

        let selected = select(datasets, &args);

        assert_eq!(ids(&selected), ["a"]);
    }

    /// The findings of a v2.1 dataset with a video feature named `name`.
    fn findings_with_video_feature(name: &str) -> Report {
        let dataset = imify_test_support::DatasetBuilder::new(imify_test_support::Format::V2_1)
            .episodes(1)
            .build()
            .unwrap();
        let info_path = dataset.root().join("meta/info.json");
        let mut info: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&info_path).unwrap()).unwrap();
        info["features"][name] = serde_json::json!({"dtype": "video"});
        std::fs::write(&info_path, serde_json::to_vec(&info).unwrap()).unwrap();
        imify::validate::validate_dataset(dataset.root()).unwrap()
    }

    #[test]
    fn findings_with_a_path_longer_than_any_format_width_are_written_in_full() {
        let name = "x".repeat(70_000);
        let report = findings_with_video_feature(&name);
        let mut output = Vec::new();

        write_findings(&mut output, &report.findings).unwrap();

        let text = String::from_utf8(output).unwrap();
        assert!(
            text.contains(&format!("videos/chunk-000/{name}/")),
            "{text:.200}"
        );
    }

    #[test]
    fn findings_columns_are_padded_to_the_longest_cell() {
        let report = findings_with_video_feature("wrist");
        let mut output = Vec::new();

        write_findings(&mut output, &report.findings).unwrap();

        let text = String::from_utf8(output).unwrap();
        let message_columns: Vec<usize> = text
            .lines()
            .map(|line| line.find("The ").unwrap())
            .collect();
        assert!(
            message_columns.windows(2).all(|pair| pair[0] == pair[1]),
            "{text}"
        );
    }
}
