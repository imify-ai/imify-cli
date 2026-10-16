//! `meta/info.json`: parsing and the version, fps and field checks.

use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use super::template::{self, Template};
use super::{Finding, FindingCode, MAX_JSON_BYTES};
use crate::local_file;

/// Path of the info file relative to the dataset root.
pub(super) const INFO_PATH: &str = "meta/info.json";

/// Placeholders of `data_path` in v2.1, and of `video_path` together with `video_key`.
const V2_1_NAMES: [&str; 2] = ["episode_chunk", "episode_index"];
/// Placeholders of `data_path` in v3.x, and of `video_path` together with `video_key`.
const V3_NAMES: [&str; 2] = ["chunk_index", "file_index"];

/// The supported `codebase_version` families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Version {
    V2_1,
    V3,
}

impl Version {
    fn parse(text: &str) -> Option<Self> {
        if text == "v2.1" {
            return Some(Self::V2_1);
        }
        let minor = text.strip_prefix("v3.")?;
        (!minor.is_empty() && minor.bytes().all(|byte| byte.is_ascii_digit())).then_some(Self::V3)
    }

    fn placeholders(self) -> [&'static str; 2] {
        match self {
            Self::V2_1 => V2_1_NAMES,
            Self::V3 => V3_NAMES,
        }
    }
}

/// The fields of `meta/info.json` the later checks use.
#[derive(Debug, Clone)]
pub(super) struct Info {
    pub(super) version: Version,
    /// `None` when `fps` is not greater than 0.
    pub(super) fps: Option<f64>,
    pub(super) chunks_size: u64,
    pub(super) total_episodes: Option<u64>,
    pub(super) data_path: Template,
    /// `None` when the dataset has no video feature.
    pub(super) video_path: Option<Template>,
    /// Names of the features with `dtype` `video`, sorted.
    pub(super) video_keys: Vec<String>,
}

/// What `meta/info.json` declares.
#[derive(Debug, Default)]
pub(super) struct Declared {
    pub(super) codebase_version: Option<String>,
    /// `None` when the file is missing, malformed or declares an unsupported version.
    pub(super) info: Option<Info>,
}

/// Reads and checks `meta/info.json`.
pub(super) fn read(root: &Path, findings: &mut Vec<Finding>) -> Declared {
    let fields = match load(root) {
        Ok(fields) => fields,
        Err(finding) => {
            findings.push(finding);
            return Declared::default();
        }
    };
    let codebase_version = fields
        .get("codebase_version")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut parser = Parser {
        fields: &fields,
        findings,
        malformed: false,
    };
    let info = parser.info(codebase_version.as_deref());
    Declared {
        codebase_version,
        info,
    }
}

fn load(root: &Path) -> Result<Map<String, Value>, Finding> {
    let malformed = |message: String| Finding::new(FindingCode::InfoMalformed, INFO_PATH, message);
    let bytes =
        local_file::read_capped(&root.join(INFO_PATH), MAX_JSON_BYTES).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Finding::new(
                    FindingCode::InfoMissing,
                    INFO_PATH,
                    "meta/info.json does not exist.".to_owned(),
                )
            } else {
                malformed(format!("meta/info.json cannot be read: {error}."))
            }
        })?;
    match serde_json::from_slice(&bytes) {
        Ok(Value::Object(fields)) => Ok(fields),
        Ok(_) => Err(malformed("meta/info.json is not a JSON object.".to_owned())),
        Err(error) => Err(malformed(format!("meta/info.json is not JSON: {error}."))),
    }
}

/// Checks the fields one by one and reports every problem.
struct Parser<'a> {
    fields: &'a Map<String, Value>,
    findings: &'a mut Vec<Finding>,
    malformed: bool,
}

impl Parser<'_> {
    fn info(&mut self, codebase_version: Option<&str>) -> Option<Info> {
        let version = self.version(codebase_version);
        let fps = self.fps();
        let chunks_size = self.chunks_size();
        let video_keys = self.video_keys();
        let total_episodes = self.fields.get("total_episodes").and_then(Value::as_u64);
        let version = version?;
        let names = version.placeholders();
        let data_path = self.template("data_path", &names);
        let video_path = if video_keys.is_empty() {
            None
        } else {
            let [first, second] = names;
            self.template("video_path", &[first, second, "video_key"])
        };
        if self.malformed {
            return None;
        }
        Some(Info {
            version,
            fps,
            chunks_size: chunks_size?,
            total_episodes,
            data_path: data_path?,
            video_path,
            video_keys,
        })
    }

    fn report(&mut self, code: FindingCode, message: String) {
        if code == FindingCode::InfoMalformed {
            self.malformed = true;
        }
        self.findings.push(Finding::new(code, INFO_PATH, message));
    }

    fn version(&mut self, codebase_version: Option<&str>) -> Option<Version> {
        let Some(text) = codebase_version else {
            self.report(
                FindingCode::InfoMalformed,
                "codebase_version is missing or not a string.".to_owned(),
            );
            return None;
        };
        let version = Version::parse(text);
        if version.is_none() {
            self.report(
                FindingCode::CodebaseVersionUnsupported,
                format!("codebase_version {text} is not supported; use v2.1 or v3.x."),
            );
        }
        version
    }

    fn fps(&mut self) -> Option<f64> {
        let declared = self.fields.get("fps");
        let Some(fps) = declared.and_then(Value::as_f64) else {
            self.report(
                FindingCode::InfoMalformed,
                "fps is missing or not a number.".to_owned(),
            );
            return None;
        };
        if fps > 0.0 {
            return Some(fps);
        }
        let shown = declared.map(Value::to_string).unwrap_or_default();
        self.report(
            FindingCode::FpsInvalid,
            format!("fps is {shown}; it must be greater than 0."),
        );
        None
    }

    fn chunks_size(&mut self) -> Option<u64> {
        let size = self
            .fields
            .get("chunks_size")
            .and_then(Value::as_u64)
            .filter(|size| *size > 0);
        if size.is_none() {
            self.report(
                FindingCode::InfoMalformed,
                "chunks_size is missing or not a positive integer.".to_owned(),
            );
        }
        size
    }

    fn video_keys(&mut self) -> Vec<String> {
        let Some(features) = self.fields.get("features").and_then(Value::as_object) else {
            self.report(
                FindingCode::InfoMalformed,
                "features is missing or not an object.".to_owned(),
            );
            return Vec::new();
        };
        let keys: Vec<String> = features
            .iter()
            .filter(|(_, feature)| feature.get("dtype").and_then(Value::as_str) == Some("video"))
            .map(|(name, _)| name.clone())
            .collect();
        for key in keys.iter().filter(|key| !template::is_path_segment(key)) {
            self.report(
                FindingCode::InfoMalformed,
                format!(
                    "the video feature {key:?} cannot be a directory name: it is empty, contains / or starts with a dot."
                ),
            );
        }
        keys
    }

    fn template(&mut self, field: &str, names: &[&str]) -> Option<Template> {
        let Some(text) = self.fields.get(field).and_then(Value::as_str) else {
            self.report(
                FindingCode::InfoMalformed,
                format!("{field} is missing or not a string."),
            );
            return None;
        };
        Template::parse(text, names)
            .map_err(|reason| {
                self.report(FindingCode::InfoMalformed, format!("{field}: {reason}."));
            })
            .ok()
    }
}
