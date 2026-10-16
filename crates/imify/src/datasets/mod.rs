//! Dataset endpoints and models.

use reqwest::Method;

use crate::{Client, Error};

/// A dataset: the episodes recorded for one task on one embodiment. Unknown response
/// fields are ignored.
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
/// for dataset in client.datasets().list().await? {
///     println!("{} {} {}/{}", dataset.id, dataset.task, dataset.accepted, dataset.episodes);
/// }
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[non_exhaustive]
pub struct Dataset {
    /// Identifier assigned by the API, a UUID string.
    pub id: String,
    /// Task description the episodes were recorded for.
    #[serde(rename = "task_description")]
    pub task: String,
    /// Dataset format, such as `lerobot_v3`.
    #[serde(rename = "dataset_format")]
    pub format: String,
    /// Display label of the robot embodiment, such as `so100`.
    #[serde(rename = "embodiment_label")]
    pub embodiment: String,
    /// Fingerprint that identifies the embodiment.
    pub embodiment_fingerprint: String,
    /// Camera names recorded in the episodes.
    pub cameras: Vec<String>,
    /// Number of action dimensions.
    pub action_dim: u32,
    /// Creation time as sent by the API, an ISO 8601 date-time.
    pub created_at: String,
    /// Number of episodes in the dataset.
    #[serde(rename = "episode_count")]
    pub episodes: u64,
    /// Number of episodes that passed quality control.
    #[serde(rename = "accepted_count")]
    pub accepted: u64,
}

/// Dataset endpoints. Created by [`Client::datasets`].
///
/// # Examples
///
/// ```no_run
/// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
/// let datasets = client.datasets().list().await?;
/// println!("{} datasets", datasets.len());
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Datasets<'a> {
    client: &'a Client,
}

impl Datasets<'_> {
    /// Returns every dataset visible to the authenticated user (`GET /api/v1/datasets`),
    /// in the order the API returns them.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Auth`] when the token source produces no token or the API rejects
    /// the token. Returns [`Error::UnexpectedResponse`] when the body is not a list of
    /// datasets. Returns the other [`Error`] variants as for every request.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), imify::Error> {
    /// use imify::Client;
    /// use imify::auth::StaticToken;
    ///
    /// let client = Client::builder("https://api.example.com")
    ///     .token_source(StaticToken::new("secret"))
    ///     .build()?;
    /// for dataset in client.datasets().list().await? {
    ///     println!("{} {}", dataset.id, dataset.task);
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn list(&self) -> Result<Vec<Dataset>, Error> {
        self.client
            .execute(Method::GET, "/api/v1/datasets", None)
            .await
    }
}

impl Client {
    /// The dataset endpoints of this client.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(client: imify::Client) -> Result<(), imify::Error> {
    /// let datasets = client.datasets().list().await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn datasets(&self) -> Datasets<'_> {
        Datasets { client: self }
    }
}

#[cfg(test)]
mod tests;
