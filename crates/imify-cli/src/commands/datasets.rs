//! Dataset commands.

use std::cmp::Ordering;
use std::io::{self, Write};

use imify::datasets::Dataset;

use super::{client, write_result};
use crate::cli::{LsArgs, SortField};
use crate::output::renderer::{Render, Renderer};
use crate::output::table::{Column, Table};
use crate::output::{CliError, Environment};

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
    let datasets = client(env)?.datasets().list().await?;
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
}
