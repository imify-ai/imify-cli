//! Policy commands.

use std::io::{self, Write};

use imify::policies::{Checkpoint, Lineage, Policy, PolicyFilter, PolicyId, PolicyState};

use super::{client, write_result};
use crate::cli::{PoliciesListArgs, PoliciesShowArgs, StateArg};
use crate::output::renderer::{Render, Renderer};
use crate::output::sanitize::printable;
use crate::output::table::{Column, Table};
use crate::output::{CliError, Environment};
use crate::tui::format::bytes;

/// `imify policies list`: prints the policies that match the filters, in the API's order.
///
/// # Errors
///
/// Returns the error of building the client, of the API request, or of writing the result.
pub async fn list(
    env: &Environment,
    args: &PoliciesListArgs,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    let api = client(env).await?;
    let policies = match api.client.policies().list(&filter(args)).await {
        Ok(policies) => policies,
        Err(error) => return Err(api.error(error).await),
    };
    write_result(renderer, &PolicyList(policies))
}

/// The SDK filter for the flags of `imify policies list`.
fn filter(args: &PoliciesListArgs) -> PolicyFilter {
    let mut filter = PolicyFilter::new();
    if let Some(state) = args.state {
        filter = filter.state(policy_state(state));
    }
    if let Some(dataset) = &args.dataset {
        filter = filter.dataset_id(dataset.clone());
    }
    filter
}

fn policy_state(state: StateArg) -> PolicyState {
    match state {
        StateArg::Queued => PolicyState::Queued,
        StateArg::Training => PolicyState::Training,
        StateArg::Ready => PolicyState::Ready,
        StateArg::Failed => PolicyState::Failed,
    }
}

/// `imify policies show <policy>`: prints the policy, its lineage and its checkpoints.
///
/// # Errors
///
/// Returns `not_found` when no policy has the id, and otherwise the error of building the
/// client, of an API request, or of writing the result.
pub async fn show(
    env: &Environment,
    args: &PoliciesShowArgs,
    renderer: &mut dyn Renderer,
) -> Result<(), CliError> {
    let api = client(env).await?;
    let id = PolicyId::new(args.policy.as_str());
    let policies = api.client.policies();
    let details = async {
        let policy = policies.get(&id).await?;
        let (lineage, checkpoints) =
            tokio::try_join!(policies.lineage(&id), policies.checkpoints(&id))?;
        Ok::<_, imify::Error>(PolicyDetails {
            policy,
            lineage,
            checkpoints,
        })
    };
    match details.await {
        Ok(details) => write_result(renderer, &details),
        Err(error) => Err(api.error(error).await),
    }
}

/// The result of `imify policies list`.
struct PolicyList(Vec<Policy>);

impl Render for PolicyList {
    fn kind(&self) -> &'static str {
        "policies"
    }

    fn json(&self) -> serde_json::Value {
        self.0.iter().map(policy_json).collect()
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        if self.0.is_empty() {
            return writeln!(w, "No policies.");
        }
        let mut table = Table::new(vec![
            Column::left("ID"),
            Column::left("TYPE"),
            Column::left("STATE"),
            Column::right("PROGRESS"),
            Column::left("DATASET"),
            Column::left("CREATED"),
        ]);
        for policy in &self.0 {
            table.row(vec![
                policy.id.to_string(),
                policy.policy_type.clone(),
                policy.state.to_string(),
                progress(policy),
                policy.dataset_id.clone(),
                policy.created_at.clone(),
            ]);
        }
        table.write(w)
    }
}

/// Training progress as `step/total_steps`.
fn progress(policy: &Policy) -> String {
    format!("{}/{}", policy.step, policy.total_steps)
}

fn policy_json(policy: &Policy) -> serde_json::Value {
    serde_json::json!({
        "id": policy.id.as_str(),
        "policy_type": policy.policy_type,
        "state": policy.state.as_str(),
        "phase": policy.phase,
        "step": policy.step,
        "total_steps": policy.total_steps,
        "dataset_id": policy.dataset_id,
        "dataset_version_id": policy.dataset_version_id,
        "error": policy.error,
        "created_at": policy.created_at,
        "started_at": policy.started_at,
        "finished_at": policy.finished_at,
    })
}

/// The result of `imify policies show`.
struct PolicyDetails {
    policy: Policy,
    lineage: Lineage,
    checkpoints: Vec<Checkpoint>,
}

impl Render for PolicyDetails {
    fn kind(&self) -> &'static str {
        "policy"
    }

    fn json(&self) -> serde_json::Value {
        let mut json = policy_json(&self.policy);
        json["lineage"] = lineage_json(&self.lineage);
        json["checkpoints"] = self.checkpoints.iter().map(checkpoint_json).collect();
        json
    }

    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        let policy = &self.policy;
        let lineage = &self.lineage;
        let none = || "-".to_owned();
        let mut fields = vec![
            ("ID", policy.id.to_string()),
            ("Type", policy.policy_type.clone()),
            ("State", policy.state.to_string()),
            ("Phase", policy.phase.clone()),
            ("Progress", progress(policy)),
            ("Dataset", lineage.dataset_id.clone()),
            (
                "Version",
                format!(
                    "{} ({})",
                    lineage.dataset_version_name, lineage.dataset_version_id
                ),
            ),
            ("Task", lineage.task.clone()),
            ("Embodiment", lineage.embodiment.clone()),
            ("Episodes", lineage.episodes.to_string()),
            ("Created", policy.created_at.clone()),
            ("Started", policy.started_at.clone().unwrap_or_else(none)),
            ("Finished", policy.finished_at.clone().unwrap_or_else(none)),
        ];
        if let Some(error) = &policy.error {
            fields.push(("Error", error.clone()));
        }
        write_fields(w, &fields)?;
        writeln!(w)?;
        if self.checkpoints.is_empty() {
            return writeln!(w, "No checkpoints.");
        }
        let mut table = Table::new(vec![
            Column::left("CHECKPOINT"),
            Column::right("SIZE"),
            Column::left("LAST MODIFIED"),
        ]);
        for checkpoint in &self.checkpoints {
            table.row(vec![
                checkpoint.id.clone(),
                bytes(checkpoint.size),
                checkpoint.last_modified.clone(),
            ]);
        }
        table.write(w)
    }
}

/// Writes one `label  value` line per field, values aligned.
fn write_fields(w: &mut dyn Write, fields: &[(&str, String)]) -> io::Result<()> {
    let width = fields
        .iter()
        .map(|(label, _)| label.len())
        .max()
        .unwrap_or(0);
    for (label, value) in fields {
        writeln!(w, "{label:<width$}  {}", printable(value))?;
    }
    Ok(())
}

fn lineage_json(lineage: &Lineage) -> serde_json::Value {
    serde_json::json!({
        "dataset_id": lineage.dataset_id,
        "dataset_version_id": lineage.dataset_version_id,
        "dataset_version_name": lineage.dataset_version_name,
        "task": lineage.task,
        "embodiment": lineage.embodiment,
        "embodiment_fingerprint": lineage.embodiment_fingerprint,
        "episodes": lineage.episodes,
        "episode_ids": lineage.episode_ids,
    })
}

fn checkpoint_json(checkpoint: &Checkpoint) -> serde_json::Value {
    serde_json::json!({
        "id": checkpoint.id,
        "size": checkpoint.size,
        "last_modified": checkpoint.last_modified,
    })
}
