//! Policies: the models trained on a dataset version, their checkpoints and training logs.
//!
//! A policy is a training run of the API (`/api/v1/trainingRuns`).

use std::fmt;

use reqwest::header::HeaderMap;
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;

use crate::error::from_response;
use crate::resource::{self, last_segment};
use crate::retry::RetryMode;
use crate::{Client, Error};

/// The collection of training runs below `/api/v1/`.
pub(crate) const TRAINING_RUNS: &str = "trainingRuns";

/// The id of a policy.
///
/// # Examples
///
/// ```
/// use imify::policies::PolicyId;
///
/// let id = PolicyId::new("run-3c9e1a7f");
/// assert_eq!(id.as_str(), "run-3c9e1a7f");
/// assert_eq!(id.to_string(), "run-3c9e1a7f");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PolicyId(String);

impl PolicyId {
    /// Wraps a policy id, such as one stored from an earlier [`Policy`].
    ///
    /// # Examples
    ///
    /// ```
    /// let id = imify::policies::PolicyId::new("run-3c9e1a7f");
    /// ```
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the id as sent by the API.
    ///
    /// # Examples
    ///
    /// ```
    /// let id = imify::policies::PolicyId::new("run-3c9e1a7f");
    /// assert_eq!(id.as_str(), "run-3c9e1a7f");
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PolicyId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Where a policy is in its life cycle.
///
/// # Examples
///
/// ```
/// use imify::policies::PolicyState;
///
/// assert_eq!(PolicyState::Training.as_str(), "training");
/// assert_eq!(PolicyState::Other("cancelled".into()).to_string(), "cancelled");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PolicyState {
    /// Training has not started.
    Queued,
    /// Training is running.
    Training,
    /// Training succeeded.
    Ready,
    /// Training failed; [`Policy::error`] holds the reason.
    Failed,
    /// A state this SDK version does not know, as sent by the API.
    Other(String),
}

impl PolicyState {
    /// Returns the state's name: `queued`, `training`, `ready`, `failed`, or the value
    /// sent by the API for [`PolicyState::Other`].
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(imify::policies::PolicyState::Ready.as_str(), "ready");
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Queued => "queued",
            Self::Training => "training",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Other(value) => value,
        }
    }

    /// The state of the API's `state`. A value this SDK version does not know is kept
    /// in lowercase, so [`PolicyState::as_str`] stays lowercase for every state.
    fn from_wire(state: &str) -> Self {
        match state {
            "QUEUED" => Self::Queued,
            "RUNNING" => Self::Training,
            "SUCCEEDED" => Self::Ready,
            "FAILED" => Self::Failed,
            _ => Self::Other(state.to_lowercase()),
        }
    }
}

impl fmt::Display for PolicyState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A policy: the model trained on one dataset version. Unknown response fields are
/// ignored.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
/// use imify::policies::PolicyFilter;
///
/// for policy in client.policies().list(&PolicyFilter::new()).await? {
///     println!("{} {} {} {}/{}", policy.id, policy.policy_type, policy.state, policy.step, policy.total_steps);
/// }
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "WirePolicy")]
#[non_exhaustive]
pub struct Policy {
    /// The policy id.
    pub id: PolicyId,
    /// Policy architecture, such as `act`, `groot_n1.7` or `pi0.5`. Values unknown to this
    /// SDK version are kept as sent.
    pub policy_type: String,
    /// Life cycle state.
    pub state: PolicyState,
    /// Id of the dataset the policy is trained on: the `{d}` of `dataset_version`.
    pub dataset_id: String,
    /// Id of the dataset version the policy is trained on: the `{v}` of
    /// `dataset_version`.
    pub dataset_version_id: String,
    /// Resource name of the dataset version, `datasets/{d}/versions/{v}`.
    pub dataset_version: String,
    /// Sub-step of the current state as sent by the API, such as `materializing`.
    pub phase: String,
    /// Training steps completed.
    pub step: u64,
    /// Training steps planned.
    pub total_steps: u64,
    /// Reason for a failed training, as sent by the API.
    pub error: Option<String>,
    /// Creation time as sent by the API, an RFC 3339 date-time.
    pub created_at: String,
    /// Start of training as sent by the API, an RFC 3339 date-time.
    pub started_at: Option<String>,
    /// End of training as sent by the API, an RFC 3339 date-time.
    pub finished_at: Option<String>,
}

/// A training run as `/api/v1/trainingRuns` sends it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePolicy {
    name: String,
    #[serde(default)]
    policy: String,
    state: String,
    #[serde(default)]
    dataset_version: String,
    #[serde(default)]
    phase: String,
    #[serde(default)]
    step: u64,
    #[serde(default)]
    step_count: u64,
    #[serde(default)]
    error: Option<WireStatus>,
    #[serde(default)]
    create_time: String,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    end_time: Option<String>,
}

/// The `error` of a failed training run.
#[derive(Deserialize)]
struct WireStatus {
    #[serde(default)]
    message: String,
}

impl From<WirePolicy> for Policy {
    fn from(wire: WirePolicy) -> Self {
        let (dataset_id, dataset_version_id) = version_ids(&wire.dataset_version);
        Self {
            id: PolicyId(last_segment(&wire.name).to_owned()),
            policy_type: wire.policy,
            state: PolicyState::from_wire(&wire.state),
            dataset_id,
            dataset_version_id,
            dataset_version: wire.dataset_version,
            phase: wire.phase,
            step: wire.step,
            total_steps: wire.step_count,
            error: wire.error.map(|error| error.message),
            created_at: wire.create_time,
            started_at: wire.start_time,
            finished_at: wire.end_time,
        }
    }
}

/// The `{d}` and `{v}` of a version name `datasets/{d}/versions/{v}`. `{v}` is the rest
/// after `/versions/`, since an ID of a version made with the old API may hold `/`. Both
/// are empty for another name.
fn version_ids(name: &str) -> (String, String) {
    name.strip_prefix("datasets/")
        .and_then(|rest| rest.split_once("/versions/"))
        .map(|(dataset, version)| (dataset.to_owned(), version.to_owned()))
        .unwrap_or_default()
}

/// What a policy was trained on: the dataset version, its dataset and the exact
/// episodes.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
/// use imify::policies::PolicyId;
///
/// let lineage = client.policies().lineage(&PolicyId::new("run-3c9e1a7f")).await?;
/// println!("{} on {}: {} episodes", lineage.task, lineage.embodiment, lineage.episodes);
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Lineage {
    /// Id of the dataset.
    pub dataset_id: String,
    /// Id of the dataset version, chosen when the version was made, such as `v3`.
    pub dataset_version_id: String,
    /// Task of the dataset.
    pub task: String,
    /// Display label of the robot embodiment, such as `so100`.
    pub embodiment: String,
    /// Fingerprint that identifies the embodiment.
    pub embodiment_fingerprint: String,
    /// Number of episodes in the dataset version.
    pub episodes: u64,
    /// Ids of the episodes in the dataset version, sorted.
    pub episode_ids: Vec<String>,
}

/// A dataset version as `GET /api/v1/datasets/{d}/versions/{v}` sends it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireVersion {
    #[serde(default)]
    episode_count: u64,
}

/// A dataset as `GET /api/v1/datasets/{d}` sends it.
#[derive(Deserialize)]
struct WireDataset {
    #[serde(default)]
    task: String,
    #[serde(default)]
    embodiment: String,
}

/// An embodiment as `GET /api/v1/embodiments/{e}` sends it.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireEmbodiment {
    #[serde(default)]
    display_name: String,
}

/// An item of a List whose resource name is all this SDK reads.
#[derive(Deserialize)]
struct Named {
    name: String,
}

/// A policy's state at one training step. Unknown response fields are ignored.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
/// use imify::policies::PolicyId;
///
/// for checkpoint in client.policies().checkpoints(&PolicyId::new("run-3c9e1a7f")).await? {
///     println!("{} {} bytes", checkpoint.id, checkpoint.size);
/// }
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "WireCheckpoint")]
#[non_exhaustive]
pub struct Checkpoint {
    /// Checkpoint id, such as the training step `020000`.
    pub id: String,
    /// Total size of the checkpoint's files in bytes.
    pub size: u64,
    /// Time of the last change as sent by the API, an RFC 3339 date-time.
    pub last_modified: String,
}

/// A checkpoint as `GET /api/v1/trainingRuns/{r}/checkpoints` sends it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCheckpoint {
    name: String,
    #[serde(default)]
    size_bytes: u64,
    #[serde(default)]
    update_time: String,
}

impl From<WireCheckpoint> for Checkpoint {
    fn from(wire: WireCheckpoint) -> Self {
        Self {
            id: last_segment(&wire.name).to_owned(),
            size: wire.size_bytes,
            last_modified: wire.update_time,
        }
    }
}

/// Which policies [`Policies::list`] returns. The default matches every policy.
///
/// # Examples
///
/// ```
/// use imify::policies::{PolicyFilter, PolicyState};
///
/// let filter = PolicyFilter::new()
///     .state(PolicyState::Ready)
///     .dataset_id("0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13");
/// assert_eq!(filter.state, Some(PolicyState::Ready));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PolicyFilter {
    /// Only policies in this state. `None` matches every state.
    pub state: Option<PolicyState>,
    /// Only policies trained on this dataset. `None` matches every dataset.
    pub dataset_id: Option<String>,
}

impl PolicyFilter {
    /// Creates a filter that matches every policy.
    ///
    /// # Examples
    ///
    /// ```
    /// let filter = imify::policies::PolicyFilter::new();
    /// assert_eq!(filter.state, None);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Matches only policies in `state`.
    ///
    /// # Examples
    ///
    /// ```
    /// use imify::policies::{PolicyFilter, PolicyState};
    ///
    /// let filter = PolicyFilter::new().state(PolicyState::Failed);
    /// ```
    #[must_use]
    pub fn state(mut self, state: PolicyState) -> Self {
        self.state = Some(state);
        self
    }

    /// Matches only policies trained on the dataset `id`.
    ///
    /// # Examples
    ///
    /// ```
    /// let filter = imify::policies::PolicyFilter::new()
    ///     .dataset_id("0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13");
    /// ```
    #[must_use]
    pub fn dataset_id(mut self, id: impl Into<String>) -> Self {
        self.dataset_id = Some(id.into());
        self
    }

    fn matches(&self, policy: &Policy) -> bool {
        self.state
            .as_ref()
            .is_none_or(|state| *state == policy.state)
            && self
                .dataset_id
                .as_ref()
                .is_none_or(|id| *id == policy.dataset_id)
    }
}

/// Policy endpoints. Created by [`Client::policies`].
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
/// use imify::policies::{PolicyFilter, PolicyState};
///
/// let ready = client.policies().list(&PolicyFilter::new().state(PolicyState::Ready)).await?;
/// println!("{} policies ready", ready.len());
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Policies<'a> {
    client: &'a Client,
}

impl Policies<'_> {
    /// Returns the policies visible to the authenticated user that match `filter`, in the
    /// order the API returns them. Reads every page of `GET /api/v1/trainingRuns` and
    /// applies the filter to the response.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Auth`] when the token source produces no token or the API rejects
    /// the token. Returns [`Error::UnexpectedResponse`] when the body is not a list of
    /// policies. Returns the other [`Error`] variants as for every request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
    /// use imify::policies::PolicyFilter;
    ///
    /// let filter = PolicyFilter::new().dataset_id("0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13");
    /// for policy in client.policies().list(&filter).await? {
    ///     println!("{} {}", policy.id, policy.state);
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn list(&self, filter: &PolicyFilter) -> Result<Vec<Policy>, Error> {
        let url = resource::url(self.client, [TRAINING_RUNS])?;
        let policies: Vec<Policy> = resource::list_all(self.client, &url, "trainingRuns").await?;
        Ok(policies
            .into_iter()
            .filter(|policy| filter.matches(policy))
            .collect())
    }

    /// Returns the policy `id` (`GET /api/v1/trainingRuns/{id}`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] when no policy has the id. Returns [`Error::Auth`] and
    /// [`Error::UnexpectedResponse`] as [`Policies::list`] does, and the other [`Error`]
    /// variants as for every request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
    /// use imify::policies::PolicyId;
    ///
    /// let policy = client.policies().get(&PolicyId::new("run-3c9e1a7f")).await?;
    /// println!("{} {}", policy.policy_type, policy.state);
    /// # Ok(()) }
    /// ```
    pub async fn get(&self, id: &PolicyId) -> Result<Policy, Error> {
        resource::get(self.client, self.url(id, None)?).await
    }

    /// Returns what the policy `id` was trained on. Reads the policy, its dataset version
    /// (`GET /api/v1/datasets/{d}/versions/{v}`), its dataset and that dataset's embodiment,
    /// and every page of the version's episodes
    /// (`GET /api/v1/episodes?datasetVersion=datasets/{d}/versions/{v}`).
    ///
    /// # Errors
    ///
    /// As [`Policies::get`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
    /// use imify::policies::PolicyId;
    ///
    /// let lineage = client.policies().lineage(&PolicyId::new("run-3c9e1a7f")).await?;
    /// println!("dataset version {}", lineage.dataset_version_id);
    /// # Ok(()) }
    /// ```
    pub async fn lineage(&self, id: &PolicyId) -> Result<Lineage, Error> {
        let policy = self.get(id).await?;
        let (version, (dataset, embodiment), episode_ids) = futures_util::future::try_join3(
            self.version(&policy),
            self.dataset_with_embodiment(&policy.dataset_id),
            self.episode_ids(&policy.dataset_version),
        )
        .await?;
        Ok(Lineage {
            embodiment_fingerprint: last_segment(&dataset.embodiment).to_owned(),
            dataset_id: policy.dataset_id,
            dataset_version_id: policy.dataset_version_id,
            task: dataset.task,
            embodiment: embodiment.display_name,
            episodes: version.episode_count,
            episode_ids,
        })
    }

    /// The dataset version `policy` was trained on.
    async fn version(self, policy: &Policy) -> Result<WireVersion, Error> {
        let segments = [
            "datasets",
            policy.dataset_id.as_str(),
            "versions",
            policy.dataset_version_id.as_str(),
        ];
        resource::get(self.client, resource::url(self.client, segments)?).await
    }

    /// The dataset `id` and its embodiment; an empty embodiment when the dataset names
    /// none.
    async fn dataset_with_embodiment(
        self,
        id: &str,
    ) -> Result<(WireDataset, WireEmbodiment), Error> {
        let dataset: WireDataset =
            resource::get(self.client, resource::url(self.client, ["datasets", id])?).await?;
        let embodiment_id = last_segment(&dataset.embodiment);
        if embodiment_id.is_empty() {
            return Ok((dataset, WireEmbodiment::default()));
        }
        let url = resource::url(self.client, ["embodiments", embodiment_id])?;
        let embodiment = resource::get(self.client, url).await?;
        Ok((dataset, embodiment))
    }

    /// The sorted ids of the episodes of the dataset version named `version`, from every
    /// page of `GET /api/v1/episodes?datasetVersion=`.
    async fn episode_ids(self, version: &str) -> Result<Vec<String>, Error> {
        let mut url = resource::url(self.client, ["episodes"])?;
        url.query_pairs_mut().append_pair("datasetVersion", version);
        let episodes: Vec<Named> = resource::list_all(self.client, &url, "episodes").await?;
        let mut ids: Vec<String> = episodes
            .iter()
            .map(|episode| last_segment(&episode.name).to_owned())
            .collect();
        ids.sort();
        Ok(ids)
    }

    /// Returns the checkpoints of the policy `id`, oldest first. Reads every page of
    /// `GET /api/v1/trainingRuns/{id}/checkpoints`.
    ///
    /// # Errors
    ///
    /// As [`Policies::get`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
    /// use imify::policies::PolicyId;
    ///
    /// let checkpoints = client.policies().checkpoints(&PolicyId::new("run-3c9e1a7f")).await?;
    /// if let Some(latest) = checkpoints.last() {
    ///     println!("latest checkpoint {}", latest.id);
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn checkpoints(&self, id: &PolicyId) -> Result<Vec<Checkpoint>, Error> {
        let url = self.url(id, Some("checkpoints"))?;
        resource::list_all(self.client, &url, "checkpoints").await
    }

    /// Returns the training log of the policy `id` as text
    /// (`GET /api/v1/trainingRuns/{id}/log`). Bytes that are not valid UTF-8 are
    /// replaced with U+FFFD.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] when no policy has the id or the policy has no log yet.
    /// Returns [`Error::UnexpectedResponse`] when the log exceeds 32 MiB. Returns
    /// [`Error::Auth`] as [`Policies::list`] does, and the other [`Error`] variants as for
    /// every request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
    /// use imify::policies::PolicyId;
    ///
    /// let log = client.policies().log(&PolicyId::new("run-3c9e1a7f")).await?;
    /// print!("{log}");
    /// # Ok(()) }
    /// ```
    pub async fn log(&self, id: &PolicyId) -> Result<String, Error> {
        let url = self.url(id, Some("log"))?;
        self.client
            .execute_with(RetryMode::Idempotent, Method::GET, url, &decode_text)
            .await
    }

    /// URL of the policy `id`, or of `segment` below it, each segment percent-encoded.
    fn url(self, id: &PolicyId, segment: Option<&str>) -> Result<Url, Error> {
        resource::url(
            self.client,
            [TRAINING_RUNS, id.as_str()].into_iter().chain(segment),
        )
    }
}

/// Decodes a successful response as UTF-8 text, replacing invalid bytes with U+FFFD, and
/// maps any other status to an [`Error`].
fn decode_text(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> Result<String, Error> {
    if !status.is_success() {
        return Err(from_response(status.as_u16(), headers, body));
    }
    Ok(String::from_utf8_lossy(body).into_owned())
}

impl Client {
    /// The policy endpoints of this client.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
    /// let policies = client.policies().list(&imify::policies::PolicyFilter::new()).await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn policies(&self) -> Policies<'_> {
        Policies { client: self }
    }
}

#[cfg(test)]
mod tests;
