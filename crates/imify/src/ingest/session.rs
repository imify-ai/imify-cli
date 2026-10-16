//! Ingest sessions as the API reports them.

use std::fmt;

use reqwest::Url;
use serde::Deserialize;

use crate::{Client, Error};

const SESSIONS_PATH: &str = "api/v1/ingest/sessions";

/// The id of an ingest session.
///
/// # Examples
///
/// ```
/// use imify::ingest::SessionId;
///
/// let id = SessionId::new("3f2c9a");
/// assert_eq!(id.as_str(), "3f2c9a");
/// assert_eq!(id.to_string(), "3f2c9a");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    /// Wraps a session id, such as one stored from an earlier [`Session`].
    ///
    /// # Examples
    ///
    /// ```
    /// let id = imify::ingest::SessionId::new("3f2c9a");
    /// ```
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the id as sent by the API.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(imify::ingest::SessionId::new("3f2c9a").as_str(), "3f2c9a");
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The settings of a new ingest session.
///
/// # Examples
///
/// ```
/// use imify::ingest::NewSession;
///
/// let session = NewSession::new("pick-place").task_description("Pick up the red cube");
/// assert_eq!(session.name, "pick-place");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NewSession {
    /// Name of the session.
    pub name: String,
    /// What the robot does in the recorded episodes. Default: empty.
    pub task_description: String,
}

impl NewSession {
    /// Creates session settings with `name` and an empty task description.
    ///
    /// # Examples
    ///
    /// ```
    /// let session = imify::ingest::NewSession::new("pick-place");
    /// assert_eq!(session.task_description, "");
    /// ```
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            task_description: String::new(),
        }
    }

    /// Sets the task description.
    ///
    /// # Examples
    ///
    /// ```
    /// let session = imify::ingest::NewSession::new("pick-place")
    ///     .task_description("Pick up the red cube");
    /// ```
    #[must_use]
    pub fn task_description(mut self, value: impl Into<String>) -> Self {
        self.task_description = value.into();
        self
    }

    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "task_description": self.task_description,
        })
    }
}

/// An ingest session as the API reports it.
///
/// # Examples
///
/// ```no_run
/// # async fn run(uploader: imify::ingest::Uploader) -> Result<(), imify::Error> {
/// use imify::ingest::{NewSession, SessionStatus};
///
/// let session = uploader.create_session(&NewSession::new("pick-place")).await?;
/// assert_eq!(session.status, SessionStatus::Uploading);
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Session {
    /// The session id.
    pub id: SessionId,
    /// Name of the session.
    pub name: String,
    /// Dataset format as detected by the API. `None` until the API has read
    /// `meta/info.json`.
    pub dataset_format: Option<String>,
    /// Robot type from `meta/info.json`.
    pub robot_type: Option<String>,
    /// Frames per second from `meta/info.json`.
    pub fps: Option<f64>,
    /// Number of episodes from `meta/info.json`.
    pub total_episodes: Option<u64>,
    /// Processing state.
    pub status: SessionStatus,
    /// What the robot does in the recorded episodes.
    pub task_description: String,
    /// Creation time as RFC 3339 text, as sent by the API.
    pub created_at: String,
}

/// Processing state of an ingest session.
///
/// # Examples
///
/// ```
/// use imify::ingest::SessionStatus;
///
/// fn is_done(status: &SessionStatus) -> bool {
///     matches!(status, SessionStatus::Ready)
/// }
///
/// assert!(!is_done(&SessionStatus::Uploading));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionStatus {
    /// The session accepts files.
    Uploading,
    /// The session is complete and its dataset is catalogued.
    Ready,
    /// A status this SDK version does not know, as sent by the API.
    Other(String),
}

impl From<String> for SessionStatus {
    fn from(value: String) -> Self {
        match value.as_str() {
            "uploading" => Self::Uploading,
            "ready" => Self::Ready,
            _ => Self::Other(value),
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct WireSession {
    id: String,
    name: String,
    dataset_format: Option<String>,
    robot_type: Option<String>,
    fps: Option<f64>,
    total_episodes: Option<u64>,
    status: String,
    #[serde(default)]
    task_description: String,
    created_at: String,
}

impl From<WireSession> for Session {
    fn from(wire: WireSession) -> Self {
        Self {
            id: SessionId(wire.id),
            name: wire.name,
            dataset_format: wire.dataset_format,
            robot_type: wire.robot_type,
            fps: wire.fps,
            total_episodes: wire.total_episodes,
            status: SessionStatus::from(wire.status),
            task_description: wire.task_description,
            created_at: wire.created_at,
        }
    }
}

/// URL of the session collection.
pub(crate) fn sessions_url(client: &Client) -> Result<Url, Error> {
    client.url(SESSIONS_PATH)
}

/// URL of `segments` below the session `id`, each segment percent-encoded.
pub(crate) fn session_url<'a>(
    client: &Client,
    id: &SessionId,
    segments: impl IntoIterator<Item = &'a str>,
) -> Result<Url, Error> {
    let mut url = sessions_url(client)?;
    url.path_segments_mut()
        .map_err(|()| Error::Config {
            message: "base URL cannot have a path".to_owned(),
        })?
        .pop_if_empty()
        .push(id.as_str())
        .extend(segments);
    Ok(url)
}

#[cfg(test)]
mod tests;
