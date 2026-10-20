//! Policies: the models trained on a dataset version, their checkpoints and training logs.

use std::fmt;

use reqwest::header::HeaderMap;
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::error::from_response;
use crate::retry::RetryMode;
use crate::{Client, Error};

const POLICIES_PATH: &str = "api/v1/training-runs";

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

    fn from_wire(status: String) -> Self {
        match status.as_str() {
            "queued" => Self::Queued,
            "running" => Self::Training,
            "succeeded" => Self::Ready,
            "failed" => Self::Failed,
            _ => Self::Other(status),
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
    /// Id of the dataset the policy is trained on.
    pub dataset_id: String,
    /// Id of the dataset version the policy is trained on.
    pub dataset_version_id: String,
    /// Sub-step of the current state as sent by the API, such as `materializing`.
    pub phase: String,
    /// Training steps completed.
    pub step: u64,
    /// Training steps planned.
    pub total_steps: u64,
    /// Reason for a failed training, as sent by the API.
    pub error: Option<String>,
    /// Creation time as sent by the API, an ISO 8601 date-time.
    pub created_at: String,
    /// Start of training as sent by the API, an ISO 8601 date-time.
    pub started_at: Option<String>,
    /// End of training as sent by the API, an ISO 8601 date-time.
    pub finished_at: Option<String>,
}

#[derive(Deserialize)]
struct WirePolicy {
    id: String,
    policy: String,
    status: String,
    dataset_id: String,
    version_id: String,
    phase: String,
    step: u64,
    total_steps: u64,
    error: Option<String>,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
}

impl From<WirePolicy> for Policy {
    fn from(wire: WirePolicy) -> Self {
        Self {
            id: PolicyId(wire.id),
            policy_type: wire.policy,
            state: PolicyState::from_wire(wire.status),
            dataset_id: wire.dataset_id,
            dataset_version_id: wire.version_id,
            phase: wire.phase,
            step: wire.step,
            total_steps: wire.total_steps,
            error: wire.error,
            created_at: wire.created_at,
            started_at: wire.started_at,
            finished_at: wire.finished_at,
        }
    }
}

/// What a policy was trained on: the dataset version, its dataset and the exact
/// episodes. Unknown response fields are ignored.
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "WireLineage")]
#[non_exhaustive]
pub struct Lineage {
    /// Id of the dataset.
    pub dataset_id: String,
    /// Id of the dataset version.
    pub dataset_version_id: String,
    /// Name of the dataset version.
    pub dataset_version_name: String,
    /// Task description of the dataset.
    pub task: String,
    /// Display label of the robot embodiment, such as `so100`.
    pub embodiment: String,
    /// Fingerprint that identifies the embodiment.
    pub embodiment_fingerprint: String,
    /// Number of episodes in the dataset version.
    pub episodes: u64,
    /// Ids of the episodes in the dataset version.
    pub episode_ids: Vec<String>,
}

#[derive(Deserialize)]
struct WireLineage {
    dataset_id: String,
    version_id: String,
    version_name: String,
    task_description: String,
    embodiment_label: String,
    embodiment_fingerprint: String,
    episode_count: u64,
    episode_ids: Vec<String>,
}

impl From<WireLineage> for Lineage {
    fn from(wire: WireLineage) -> Self {
        Self {
            dataset_id: wire.dataset_id,
            dataset_version_id: wire.version_id,
            dataset_version_name: wire.version_name,
            task: wire.task_description,
            embodiment: wire.embodiment_label,
            embodiment_fingerprint: wire.embodiment_fingerprint,
            episodes: wire.episode_count,
            episode_ids: wire.episode_ids,
        }
    }
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
#[non_exhaustive]
pub struct Checkpoint {
    /// Checkpoint id, such as the training step `020000`.
    pub id: String,
    /// Total size of the checkpoint's files in bytes.
    pub size: u64,
    /// Time of the last change as sent by the API, an ISO 8601 date-time.
    pub last_modified: String,
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
    /// Returns the policies visible to the authenticated user that match `filter`
    /// (`GET /api/v1/training-runs`), in the order the API returns them. The filter is
    /// applied to the response.
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
        let policies: Vec<Policy> = self
            .client
            .execute(Method::GET, POLICIES_PATH, None)
            .await?;
        Ok(policies
            .into_iter()
            .filter(|policy| filter.matches(policy))
            .collect())
    }

    /// Returns the policy `id` (`GET /api/v1/training-runs/{id}`).
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
        self.get_json(id, None).await
    }

    /// Returns what the policy `id` was trained on
    /// (`GET /api/v1/training-runs/{id}/lineage`).
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
    /// println!("dataset version {}", lineage.dataset_version_name);
    /// # Ok(()) }
    /// ```
    pub async fn lineage(&self, id: &PolicyId) -> Result<Lineage, Error> {
        self.get_json(id, Some("lineage")).await
    }

    /// Returns the checkpoints of the policy `id`, oldest first
    /// (`GET /api/v1/training-runs/{id}/checkpoints`).
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
        self.get_json(id, Some("checkpoints")).await
    }

    /// Returns the training log of the policy `id` as text
    /// (`GET /api/v1/training-runs/{id}/log`). Bytes that are not valid UTF-8 are
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

    /// Sends a `GET` to the policy `id`, or to `segment` below it, and decodes the JSON
    /// response as `T`.
    async fn get_json<T: DeserializeOwned>(
        &self,
        id: &PolicyId,
        segment: Option<&str>,
    ) -> Result<T, Error> {
        let url = self.url(id, segment)?;
        self.client
            .execute_at(RetryMode::Idempotent, Method::GET, url, None)
            .await
    }

    /// URL of the policy `id`, or of `segment` below it, each segment percent-encoded.
    fn url(self, id: &PolicyId, segment: Option<&str>) -> Result<Url, Error> {
        let mut url = self.client.url(POLICIES_PATH)?;
        url.path_segments_mut()
            .map_err(|()| Error::Config {
                message: "base URL cannot have a path".to_owned(),
            })?
            .pop_if_empty()
            .push(id.as_str())
            .extend(segment);
        Ok(url)
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
