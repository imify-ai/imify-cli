//! Expansion of the `data_path` and `video_path` templates of `meta/info.json`.

use std::path::PathBuf;

/// A path template with placeholders such as `{video_key}` and zero-padded integer
/// placeholders such as `{episode_index:06d}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Template(Vec<Part>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    Placeholder { name: String, width: usize },
}

/// The value of a placeholder.
#[derive(Debug, Clone, Copy)]
pub(super) enum Arg<'a> {
    /// Written in decimal, zero-padded to the placeholder's width.
    Int(u64),
    /// Written as it is.
    Str(&'a str),
}

impl Template {
    /// Parses `text`, accepting only placeholders named in `names`.
    ///
    /// Returns the reason when a placeholder is unknown, has a format spec other than
    /// a zero-padded width such as `06d`, or a brace is unbalanced.
    pub(super) fn parse(text: &str, names: &[&str]) -> Result<Self, String> {
        let mut parts = Vec::new();
        let mut rest = text;
        while let Some(open) = rest.find(['{', '}']) {
            if rest[open..].starts_with('}') {
                return Err(format!("unbalanced brace in {text}"));
            }
            let close = rest[open..]
                .find('}')
                .map(|offset| open + offset)
                .ok_or_else(|| format!("unbalanced brace in {text}"))?;
            if open > 0 {
                parts.push(Part::Literal(rest[..open].to_owned()));
            }
            parts.push(placeholder(&rest[open + 1..close], names)?);
            rest = &rest[close + 1..];
        }
        if !rest.is_empty() {
            parts.push(Part::Literal(rest.to_owned()));
        }
        Ok(Self(parts))
    }

    /// The path with every placeholder replaced by its value in `args`. A placeholder
    /// without a value expands to nothing.
    pub(super) fn expand(&self, args: &[(&str, Arg<'_>)]) -> PathBuf {
        let path: String = self
            .0
            .iter()
            .map(|part| match part {
                Part::Literal(text) => text.clone(),
                Part::Placeholder { name, width } => {
                    match args
                        .iter()
                        .find(|(arg, _)| arg == name)
                        .map(|(_, value)| value)
                    {
                        Some(Arg::Int(value)) => format!("{value:0width$}"),
                        Some(Arg::Str(value)) => (*value).to_owned(),
                        None => String::new(),
                    }
                }
            })
            .collect();
        PathBuf::from(path)
    }
}

fn placeholder(field: &str, names: &[&str]) -> Result<Part, String> {
    let (name, spec) = field.split_once(':').unwrap_or((field, ""));
    if !names.contains(&name) {
        return Err(format!("unknown placeholder {{{field}}}"));
    }
    let width = if spec.is_empty() {
        0
    } else {
        spec.strip_prefix('0')
            .and_then(|spec| spec.strip_suffix('d'))
            .and_then(|digits| digits.parse().ok())
            .ok_or_else(|| format!("unsupported format spec in {{{field}}}"))?
    };
    Ok(Part::Placeholder {
        name: name.to_owned(),
        width,
    })
}
