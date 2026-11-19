//! Recordings as the API reports them, and the settings of a new one.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

use reqwest::Url;
use serde::Deserialize;

use super::plan::UploadPlan;
use crate::resource::{self, last_segment};
use crate::{Client, Error, FieldError};

/// The collection of recordings below `/api/v1/`.
const RECORDINGS: &str = "recordings";

/// Most characters of [`NewRecording::display_name`].
const MAX_DISPLAY_NAME: usize = 128;
/// Most characters of [`NewRecording::task`].
const MAX_TASK: usize = 512;
/// Most characters of the path of a file.
const MAX_PATH: usize = 512;
/// Most characters of [`NewRecording::robot`], [`NewRecording::operator`] and
/// [`NewRecording::site`].
const MAX_ID: usize = 128;
/// Most entries of [`NewRecording::labels`].
const MAX_LABELS: usize = 64;
/// Most characters of a label key.
const MAX_LABEL_KEY: usize = 63;
/// Most characters of a label value.
const MAX_LABEL_VALUE: usize = 256;

/// The id of a recording: the `{r}` of its resource name `recordings/{r}`.
///
/// # Examples
///
/// ```
/// use imify::ingest::RecordingId;
///
/// let id = RecordingId::new("3f2c9a");
/// assert_eq!(id.as_str(), "3f2c9a");
/// assert_eq!(id.to_string(), "3f2c9a");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RecordingId(String);

impl RecordingId {
    /// Wraps a recording id, such as one stored from an earlier [`Recording`]. The id of
    /// an ingest session of the old API is the id of the same recording.
    ///
    /// # Examples
    ///
    /// ```
    /// let id = imify::ingest::RecordingId::new("3f2c9a");
    /// ```
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the id as sent by the API.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(imify::ingest::RecordingId::new("3f2c9a").as_str(), "3f2c9a");
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The resource name `recordings/{r}`.
    pub(crate) fn name(&self) -> String {
        format!("{RECORDINGS}/{}", self.0)
    }
}

impl fmt::Display for RecordingId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The `requestId` of a create: a create repeated with the same value returns the first
/// recording instead of a second one, also after the process restarted.
///
/// # Examples
///
/// ```
/// use imify::ingest::RequestId;
///
/// let id = RequestId::random()?;
/// assert_eq!(id.as_str().len(), 36);
/// let stored = RequestId::new(id.as_str());
/// assert_eq!(stored, id);
/// # Ok::<(), imify::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(String);

impl RequestId {
    /// A new random UUID (version 4).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] when the operating system has no random source.
    ///
    /// # Examples
    ///
    /// ```
    /// let id = imify::ingest::RequestId::random()?;
    /// # Ok::<(), imify::Error>(())
    /// ```
    pub fn random() -> Result<Self, Error> {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).map_err(|error| Error::Config {
            message: format!("the operating system has no random source: {error}"),
        })?;
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex = bytes
            .iter()
            .fold(String::with_capacity(32), |mut hex, byte| {
                let _ = write!(hex, "{byte:02x}");
                hex
            });
        Ok(Self(format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )))
    }

    /// Wraps a request id stored from an earlier create.
    ///
    /// # Examples
    ///
    /// ```
    /// let id = imify::ingest::RequestId::new("0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13");
    /// ```
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the request id.
    ///
    /// # Examples
    ///
    /// ```
    /// let id = imify::ingest::RequestId::new("0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13");
    /// assert_eq!(id.as_str(), "0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13");
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The settings of a new recording. The API files its episodes into the dataset of
/// `task` and of the embodiment and format of the upload.
///
/// `robot`, `operator`, `site`, `start_time` and `task` never change after the recording
/// exists. Without `operator`, the API sets the signed-in user.
///
/// # Examples
///
/// ```
/// use imify::ingest::NewRecording;
///
/// let recording = NewRecording::new("pick-place")
///     .task("Pick up the red cube")
///     .robot("so100-0042")
///     .label("shift", "night");
/// assert!(recording.check().is_ok());
/// assert_eq!(recording.display_name, "pick-place");
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct NewRecording {
    /// Name of the recording, at most 128 characters.
    pub display_name: String,
    /// What the robot does in the recorded episodes, at most 512 characters. Default:
    /// empty.
    pub task: String,
    /// Serial number of the robot: letters, digits, `.`, `_` and `-`, at most 128
    /// characters.
    pub robot: Option<String>,
    /// Id of the operator, never a name: letters, digits, `.`, `_` and `-`, at most 128
    /// characters. `None`: the API sets the signed-in user.
    pub operator: Option<String>,
    /// Id of the site: letters, digits, `.`, `_` and `-`, at most 128 characters.
    pub site: Option<String>,
    /// Start of the recording, an RFC 3339 date-time.
    pub start_time: Option<String>,
    /// Labels, at most 64. A key starts with a lowercase letter and has only lowercase
    /// letters, digits, `_` and `-`, at most 63 characters. A value has at most 256
    /// characters.
    pub labels: BTreeMap<String, String>,
}

impl NewRecording {
    /// Creates settings with `display_name` and no other value.
    ///
    /// # Examples
    ///
    /// ```
    /// let recording = imify::ingest::NewRecording::new("pick-place");
    /// assert_eq!(recording.task, "");
    /// ```
    pub fn new(display_name: impl Into<String>) -> Self {
        Self {
            display_name: display_name.into(),
            ..Self::default()
        }
    }

    /// Sets the task.
    ///
    /// # Examples
    ///
    /// ```
    /// let recording = imify::ingest::NewRecording::new("pick-place").task("Pick up the red cube");
    /// ```
    #[must_use]
    pub fn task(mut self, value: impl Into<String>) -> Self {
        self.task = value.into();
        self
    }

    /// Sets the serial number of the robot.
    ///
    /// # Examples
    ///
    /// ```
    /// let recording = imify::ingest::NewRecording::new("pick-place").robot("so100-0042");
    /// ```
    #[must_use]
    pub fn robot(mut self, value: impl Into<String>) -> Self {
        self.robot = Some(value.into());
        self
    }

    /// Sets the id of the operator.
    ///
    /// # Examples
    ///
    /// ```
    /// let recording = imify::ingest::NewRecording::new("pick-place").operator("op-17");
    /// ```
    #[must_use]
    pub fn operator(mut self, value: impl Into<String>) -> Self {
        self.operator = Some(value.into());
        self
    }

    /// Sets the id of the site.
    ///
    /// # Examples
    ///
    /// ```
    /// let recording = imify::ingest::NewRecording::new("pick-place").site("lab-munich");
    /// ```
    #[must_use]
    pub fn site(mut self, value: impl Into<String>) -> Self {
        self.site = Some(value.into());
        self
    }

    /// Sets the start of the recording, an RFC 3339 date-time.
    ///
    /// # Examples
    ///
    /// ```
    /// let recording = imify::ingest::NewRecording::new("pick-place")
    ///     .start_time("2026-11-18T09:30:00Z");
    /// ```
    #[must_use]
    pub fn start_time(mut self, value: impl Into<String>) -> Self {
        self.start_time = Some(value.into());
        self
    }

    /// Adds the label `key` with `value`, replacing an earlier value of `key`.
    ///
    /// # Examples
    ///
    /// ```
    /// let recording = imify::ingest::NewRecording::new("pick-place").label("shift", "night");
    /// assert_eq!(recording.labels["shift"], "night");
    /// ```
    #[must_use]
    pub fn label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    /// Checks every value against the limits of the API, so an upload fails before it
    /// sends a request.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] with one [`FieldError`] per value outside its limit.
    /// The message describes the first.
    ///
    /// # Examples
    ///
    /// ```
    /// let error = imify::ingest::NewRecording::new("pick-place")
    ///     .robot("so100 #42")
    ///     .check()
    ///     .unwrap_err();
    /// assert!(matches!(error, imify::Error::Validation { ref fields, .. } if fields[0].location == ["robot"]));
    /// ```
    pub fn check(&self) -> Result<(), Error> {
        let mut problems = Vec::new();
        check_length(
            &mut problems,
            &["displayName"],
            &self.display_name,
            MAX_DISPLAY_NAME,
        );
        check_length(&mut problems, &["task"], &self.task, MAX_TASK);
        for (field, value) in [
            ("robot", &self.robot),
            ("operator", &self.operator),
            ("site", &self.site),
        ] {
            if let Some(value) = value
                && !is_id(value)
            {
                problems.push(problem(
                    &[field],
                    format!(
                        "{field} {value:?} may hold only letters, digits, '.', '_' and '-', 1 to {MAX_ID} characters"
                    ),
                ));
            }
        }
        if let Some(start_time) = &self.start_time
            && !is_rfc3339(start_time)
        {
            problems.push(problem(
                &["startTime"],
                format!("startTime {start_time:?} is not an RFC 3339 date-time"),
            ));
        }
        if self.labels.len() > MAX_LABELS {
            problems.push(problem(
                &["labels"],
                format!("{} labels are more than {MAX_LABELS}", self.labels.len()),
            ));
        }
        for (key, value) in &self.labels {
            if !is_label_key(key) {
                problems.push(problem(
                    &["labels", key],
                    format!(
                        "label key {key:?} must start with a lowercase letter and hold only lowercase letters, digits, '_' and '-', at most {MAX_LABEL_KEY} characters"
                    ),
                ));
            }
            check_length(&mut problems, &["labels", key], value, MAX_LABEL_VALUE);
        }
        into_result(problems)
    }

    /// The body of `POST /api/v1/recordings`.
    pub(crate) fn to_json(&self, request_id: &RequestId) -> serde_json::Value {
        let mut body = serde_json::json!({
            "requestId": request_id.as_str(),
            "displayName": self.display_name,
            "task": self.task,
        });
        for (key, value) in [
            ("robot", &self.robot),
            ("operator", &self.operator),
            ("site", &self.site),
            ("startTime", &self.start_time),
        ] {
            if let Some(value) = value {
                body[key] = value.as_str().into();
            }
        }
        if !self.labels.is_empty() {
            body["labels"] = serde_json::json!(self.labels);
        }
        body
    }
}

/// Fails with [`Error::Validation`] when a file path of `plan` is longer than the API
/// takes.
pub(crate) fn check_paths(plan: &UploadPlan) -> Result<(), Error> {
    let mut problems = Vec::new();
    for file in plan.files() {
        check_length(&mut problems, &["file", "path"], file.path(), MAX_PATH);
    }
    into_result(problems)
}

fn check_length(problems: &mut Vec<FieldError>, location: &[&str], value: &str, max: usize) {
    let length = value.chars().count();
    if length > max {
        problems.push(problem(
            location,
            format!(
                "{} {value:?} has {length} characters, more than {max}",
                location.join(".")
            ),
        ));
    }
}

fn problem(location: &[&str], message: String) -> FieldError {
    FieldError {
        location: location
            .iter()
            .map(|segment| (*segment).to_owned())
            .collect(),
        message,
        kind: String::new(),
    }
}

fn into_result(problems: Vec<FieldError>) -> Result<(), Error> {
    match problems.first() {
        None => Ok(()),
        Some(first) => Err(Error::Validation {
            message: first.message.clone(),
            fields: problems,
            reason: None,
        }),
    }
}

/// `^[A-Za-z0-9._-]{1,128}$`
fn is_id(value: &str) -> bool {
    (1..=MAX_ID).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// `^[a-z][a-z0-9_-]{0,62}$`
fn is_label_key(key: &str) -> bool {
    key.len() <= MAX_LABEL_KEY
        && key
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && key.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

/// `YYYY-MM-DDTHH:MM:SS[.fraction](Z|+HH:MM|-HH:MM)`, a date-time of RFC 3339 section
/// 5.6. Checks the range of each field, not the number of days of the month.
fn is_rfc3339(value: &str) -> bool {
    let Some((date_time, zone)) = value.get(..19).zip(value.get(19..)) else {
        return false;
    };
    let fields_match = date_time
        .bytes()
        .zip(b"0000-00-00T00:00:00")
        .all(|(byte, pattern)| match pattern {
            b'0' => byte.is_ascii_digit(),
            b'T' => matches!(byte, b'T' | b't'),
            _ => byte == *pattern,
        });
    if !fields_match {
        return false;
    }
    let field = |range: std::ops::Range<usize>| date_time[range].parse::<u32>().unwrap_or(0);
    let in_range = (1..=12).contains(&field(5..7))
        && (1..=31).contains(&field(8..10))
        && field(11..13) <= 23
        && field(14..16) <= 59
        && field(17..19) <= 60;
    in_range && is_zone(zone)
}

/// `[.fraction](Z|+HH:MM|-HH:MM)`
fn is_zone(zone: &str) -> bool {
    let zone = match zone.strip_prefix('.') {
        Some(fraction) => {
            let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return false;
            }
            &fraction[digits..]
        }
        None => zone,
    };
    if matches!(zone, "Z" | "z") {
        return true;
    }
    let offset = zone.as_bytes();
    offset.len() == 6
        && matches!(offset[0], b'+' | b'-')
        && offset[3] == b':'
        && [1, 2, 4, 5]
            .iter()
            .all(|&index| offset[index].is_ascii_digit())
        && zone[1..3].parse::<u32>().is_ok_and(|hours| hours <= 23)
        && zone[4..6].parse::<u32>().is_ok_and(|minutes| minutes <= 59)
}

/// A recording as the API reports it. Unknown response fields are ignored.
///
/// # Examples
///
/// ```no_run
/// # async fn run(uploader: imify::ingest::Uploader) -> Result<(), imify::Error> {
/// use imify::ingest::{NewRecording, RecordingState, RequestId};
///
/// let recording = uploader
///     .create_recording(&NewRecording::new("pick-place"), &RequestId::random()?)
///     .await?;
/// assert_eq!(recording.state, RecordingState::Uploading);
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Recording {
    /// The recording id.
    pub id: RecordingId,
    /// Name of the recording.
    pub display_name: String,
    /// What the robot does in the recorded episodes.
    pub task: String,
    /// Serial number of the robot; empty when not set.
    pub robot: String,
    /// Id of the operator.
    pub operator: String,
    /// Id of the site; empty when not set.
    pub site: String,
    /// Start of the recording as RFC 3339 text; `None` when not set.
    pub start_time: Option<String>,
    /// Labels of the recording.
    pub labels: BTreeMap<String, String>,
    /// Processing state.
    pub state: RecordingState,
    /// Dataset format from `meta/info.json`, such as `v3`. `None` until the upload is
    /// complete.
    pub format: Option<String>,
    /// Robot type from `meta/info.json`. `None` until the upload is complete.
    pub robot_type: Option<String>,
    /// Frames per second from `meta/info.json`. `None` until the upload is complete.
    pub fps: Option<u32>,
    /// Number of episodes. 0 until the upload is complete.
    pub episode_count: u64,
    /// Id of the dataset the episodes went into. `None` until the upload is complete.
    pub dataset_id: Option<String>,
    /// Creation time as RFC 3339 text, as sent by the API.
    pub created_at: String,
}

/// Processing state of a recording.
///
/// # Examples
///
/// ```
/// use imify::ingest::RecordingState;
///
/// fn upload_done(state: &RecordingState) -> bool {
///     !matches!(state, RecordingState::Uploading)
/// }
///
/// assert!(upload_done(&RecordingState::Processing));
/// assert_eq!(RecordingState::Ready.as_str(), "ready");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecordingState {
    /// The recording accepts files.
    Uploading,
    /// The upload is complete; the platform processes its episodes.
    Processing,
    /// Every episode is processed.
    Ready,
    /// A state this SDK version does not know, in lowercase. The upload of such a
    /// recording is done.
    Other(String),
}

impl RecordingState {
    /// Returns the state's name: `uploading`, `processing`, `ready`, or the lowercase
    /// value sent by the API for [`RecordingState::Other`].
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(imify::ingest::RecordingState::Processing.as_str(), "processing");
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Uploading => "uploading",
            Self::Processing => "processing",
            Self::Ready => "ready",
            Self::Other(value) => value,
        }
    }

    fn from_wire(state: &str) -> Self {
        match state {
            "UPLOADING" => Self::Uploading,
            "PROCESSING" => Self::Processing,
            "READY" => Self::Ready,
            _ => Self::Other(state.to_lowercase()),
        }
    }
}

impl fmt::Display for RecordingState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A recording as `/api/v1/recordings` sends it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireRecording {
    name: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    task: String,
    #[serde(default)]
    robot: String,
    #[serde(default)]
    operator: String,
    #[serde(default)]
    site: String,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    labels: BTreeMap<String, String>,
    state: String,
    #[serde(default)]
    format: String,
    #[serde(default)]
    robot_type: String,
    #[serde(default)]
    fps: Option<u32>,
    #[serde(default)]
    episode_count: u64,
    #[serde(default)]
    dataset: Option<String>,
    #[serde(default)]
    create_time: String,
}

impl From<WireRecording> for Recording {
    fn from(wire: WireRecording) -> Self {
        let state = RecordingState::from_wire(&wire.state);
        let complete = state != RecordingState::Uploading;
        let non_empty = |value: String| (!value.is_empty()).then_some(value);
        Self {
            id: RecordingId(last_segment(&wire.name).to_owned()),
            display_name: wire.display_name,
            task: wire.task,
            robot: wire.robot,
            operator: wire.operator,
            site: wire.site,
            start_time: wire.start_time,
            labels: wire.labels,
            format: non_empty(wire.format),
            robot_type: non_empty(wire.robot_type),
            fps: wire.fps.filter(|_| complete),
            episode_count: wire.episode_count,
            dataset_id: wire
                .dataset
                .filter(|name| !name.is_empty())
                .map(|name| last_segment(&name).to_owned()),
            created_at: wire.create_time,
            state,
        }
    }
}

/// URL of the recording collection.
pub(crate) fn recordings_url(client: &Client) -> Result<Url, Error> {
    resource::url(client, [RECORDINGS])
}

/// URL of the recording `id`, or of `segment` below it, such as `files:batchCreate`.
pub(crate) fn recording_url(
    client: &Client,
    id: &RecordingId,
    segment: Option<&str>,
) -> Result<Url, Error> {
    resource::url(client, [RECORDINGS, id.as_str()].into_iter().chain(segment))
}

/// URL of the custom method `verb` of the recording `id`, such as
/// `recordings/{r}:complete`.
pub(crate) fn method_url(client: &Client, id: &RecordingId, verb: &str) -> Result<Url, Error> {
    resource::url(client, [RECORDINGS, &format!("{}:{verb}", id.as_str())])
}

#[cfg(test)]
mod tests;
