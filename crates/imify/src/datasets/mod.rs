//! Dataset endpoints and models.

use std::collections::HashMap;

use serde::Deserialize;

use crate::resource::{self, last_segment};
use crate::{Client, Error};

/// A dataset: the episodes recorded for one task on one embodiment, with the label,
/// cameras and action dimensions of the embodiment. Unknown response fields are ignored.
///
/// `Deserialize` reads the field names of this struct, such as a copy a caller stored;
/// [`Datasets::list`] reads the API's names.
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct Dataset {
    /// The dataset ID: the `{d}` of the resource name `datasets/{d}`.
    pub id: String,
    /// Task the episodes were recorded for.
    pub task: String,
    /// Dataset format, such as `v3` or `v2.1`.
    pub format: String,
    /// Display label of the robot embodiment, such as `so100`. Empty when the API lists
    /// no embodiment of the dataset.
    pub embodiment: String,
    /// Fingerprint that identifies the embodiment: the `{e}` of `embodiments/{e}`.
    pub embodiment_fingerprint: String,
    /// Camera names of the embodiment.
    pub cameras: Vec<String>,
    /// Number of action dimensions of the embodiment.
    pub action_dim: u32,
    /// Creation time as sent by the API, an RFC 3339 date-time.
    pub created_at: String,
    /// Number of episodes in the dataset, in any state.
    pub episodes: u64,
    /// Number of episodes in state `ACCEPTED`.
    pub accepted: u64,
}

/// A dataset as `GET /api/v1/datasets` sends it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireDataset {
    name: String,
    #[serde(default)]
    task: String,
    #[serde(default)]
    format: String,
    #[serde(default)]
    embodiment: String,
    #[serde(default)]
    create_time: String,
    #[serde(default)]
    episode_counts: HashMap<String, u64>,
}

/// An embodiment as `GET /api/v1/embodiments` sends it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireEmbodiment {
    name: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    cameras: Vec<String>,
    #[serde(default)]
    action_dim: u32,
}

/// The episode state whose count is [`Dataset::accepted`].
const ACCEPTED: &str = "ACCEPTED";

impl WireDataset {
    /// The dataset with the label, cameras and action dimensions of its embodiment from
    /// `embodiments`, keyed by resource name.
    fn joined(self, embodiments: &HashMap<String, WireEmbodiment>) -> Dataset {
        let embodiment = embodiments.get(&self.embodiment);
        Dataset {
            id: last_segment(&self.name).to_owned(),
            task: self.task,
            format: self.format,
            embodiment: embodiment
                .map(|e| e.display_name.clone())
                .unwrap_or_default(),
            embodiment_fingerprint: last_segment(&self.embodiment).to_owned(),
            cameras: embodiment.map(|e| e.cameras.clone()).unwrap_or_default(),
            action_dim: embodiment.map_or(0, |e| e.action_dim),
            created_at: self.create_time,
            episodes: self.episode_counts.values().sum(),
            accepted: self.episode_counts.get(ACCEPTED).copied().unwrap_or(0),
        }
    }
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
    /// Returns every dataset visible to the authenticated user, in the order the API
    /// returns them. Reads every page of `GET /api/v1/datasets` and of
    /// `GET /api/v1/embodiments`, and takes the label, cameras and action dimensions of
    /// each dataset from its embodiment.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Auth`] when the token source produces no token or the API rejects
    /// the token. Returns [`Error::UnexpectedResponse`] when a page is not a list of
    /// datasets or embodiments. Returns the other [`Error`] variants as for every request.
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
        let datasets_url = resource::url(self.client, ["datasets"])?;
        let embodiments_url = resource::url(self.client, ["embodiments"])?;
        let (datasets, embodiments) = futures_util::future::try_join(
            resource::list_all::<WireDataset>(self.client, &datasets_url, "datasets"),
            resource::list_all::<WireEmbodiment>(self.client, &embodiments_url, "embodiments"),
        )
        .await?;
        let embodiments: HashMap<String, WireEmbodiment> = embodiments
            .into_iter()
            .map(|embodiment| (embodiment.name.clone(), embodiment))
            .collect();
        Ok(datasets
            .into_iter()
            .map(|dataset| dataset.joined(&embodiments))
            .collect())
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
