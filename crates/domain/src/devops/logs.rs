//! Structured logs on the filterable list (`show_logs`).
//!
//! The adapter reads JSON lines, logfmt or plain text into a
//! [`ListSpec`]: the message as the item text, the level, the timestamp
//! as written and the other keys as fields, with facets on the fields
//! that make good chips (a service, a logger). A [`LogHistogram`] counts
//! the lines per time bucket and level class when the timestamps parse.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::invalid;
use crate::block::BlockError;
use crate::widget::list::{LevelClass, ListSpec};

/// Lines a log block keeps; the rest of a larger file is counted only.
pub const MAX_LINES: usize = 50_000;
/// Most buckets of the histogram.
pub const MAX_BUCKETS: usize = 120;

const BLOCK: &str = "logs";

/// How the lines were written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    /// One JSON object per line (pino, bunyan, zap, slog, tracing...).
    Json,
    /// `key=value` pairs (logrus, Go `log/slog` text, Heroku).
    Logfmt,
    /// Free text; level and timestamp guessed from each line.
    Text,
}

impl LogFormat {
    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Json => "JSON lines",
            Self::Logfmt => "logfmt",
            Self::Text => "text",
        }
    }
}

/// Lines of one time bucket, by level class.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BucketCounts {
    /// Fatal and error lines.
    #[serde(default)]
    pub errors: u32,
    /// Warnings.
    #[serde(default)]
    pub warnings: u32,
    /// Everything else (info, debug, no level).
    #[serde(default)]
    pub other: u32,
}

impl BucketCounts {
    /// All lines of the bucket.
    #[must_use]
    pub fn total(&self) -> u32 {
        self.errors + self.warnings + self.other
    }
}

/// Lines over time: `buckets.len()` buckets of `width` seconds from
/// `start` (seconds since the epoch).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LogHistogram {
    /// Start of the first bucket.
    pub start: f64,
    /// Width of a bucket, in seconds.
    pub width: f64,
    /// Counts per bucket.
    pub buckets: Vec<BucketCounts>,
}

/// A log file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LogSpec {
    /// Workspace path of the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// How the lines were written (most of them).
    pub format: LogFormat,
    /// Records in the bytes read; more than the items when cut at
    /// [`MAX_LINES`].
    pub total_lines: usize,
    /// Only the head of the file was read.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// The records, with the title and facets.
    pub list: ListSpec,
    /// Lines per time bucket, when enough timestamps parsed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub histogram: Option<LogHistogram>,
}

impl LogSpec {
    /// Checks the list, the size and the histogram.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        self.list.validate()?;
        if self.list.items.len() > MAX_LINES {
            return Err(invalid(
                BLOCK,
                format!(
                    "{} lines; at most {MAX_LINES} are kept",
                    self.list.items.len()
                ),
            ));
        }
        if self.total_lines < self.list.items.len() {
            return Err(invalid(BLOCK, "total_lines is less than the items"));
        }
        if let Some(h) = &self.histogram {
            if !(h.start.is_finite() && h.width.is_finite() && h.width > 0.0) {
                return Err(invalid(
                    BLOCK,
                    "histogram needs a finite start and width > 0",
                ));
            }
            if h.buckets.is_empty() || h.buckets.len() > MAX_BUCKETS {
                return Err(invalid(
                    BLOCK,
                    format!("histogram needs 1..={MAX_BUCKETS} buckets"),
                ));
            }
        }
        Ok(())
    }

    /// Lines per level name (lowercased; `-` for none), most severe class
    /// first.
    #[must_use]
    pub fn level_counts(&self) -> Vec<(String, usize)> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for item in &self.list.items {
            let level = item
                .level
                .as_deref()
                .map_or_else(|| "-".to_owned(), str::to_ascii_lowercase);
            *counts.entry(level).or_default() += 1;
        }
        let mut out: Vec<_> = counts.into_iter().collect();
        out.sort_by(|a, b| {
            let class = |l: &str| {
                if l == "-" {
                    (LevelClass::Other, 1)
                } else {
                    (LevelClass::of(l), 0)
                }
            };
            class(&a.0).cmp(&class(&b.0)).then(a.0.cmp(&b.0))
        });
        out
    }

    /// Indices of the fatal and error items.
    #[must_use]
    pub fn errors(&self) -> Vec<usize> {
        self.list
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                item.level
                    .as_deref()
                    .is_some_and(|l| LevelClass::of(l) <= LevelClass::Error)
            })
            .map(|(ix, _)| ix)
            .collect()
    }

    /// Error and fatal messages grouped by shape (numbers, hex ids and
    /// quoted values replaced by `<…>`): `(pattern, count, first index)`,
    /// most frequent first.
    #[must_use]
    pub fn error_patterns(&self) -> Vec<(String, usize, usize)> {
        let mut groups: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for ix in self.errors() {
            let pattern = pattern(&self.list.items[ix].text);
            let entry = groups.entry(pattern).or_insert((0, ix));
            entry.0 += 1;
        }
        let mut out: Vec<_> = groups
            .into_iter()
            .map(|(p, (count, first))| (p, count, first))
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
        out
    }
}

/// The shape of a message: digits, hex ids, UUIDs and quoted strings
/// become `<n>`, `<id>` and `<s>`, so similar errors group together.
#[must_use]
pub fn pattern(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' {
            let mut closed = false;
            let mut inner = String::new();
            for d in chars.by_ref() {
                if d == c {
                    closed = true;
                    break;
                }
                inner.push(d);
            }
            if closed {
                out.push_str("<s>");
            } else {
                out.push(c);
                out.push_str(&inner);
            }
        } else if c.is_ascii_alphanumeric() {
            let mut word = String::from(c);
            while let Some(&d) = chars.peek() {
                if d.is_ascii_alphanumeric() || d == '-' || d == '_' || d == '.' {
                    word.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            out.push_str(&word_shape(&word));
        } else {
            out.push(c);
        }
    }
    out
}

fn word_shape(word: &str) -> String {
    let digits = word.chars().filter(char::is_ascii_digit).count();
    if digits == 0 {
        return word.to_owned();
    }
    let trimmed = word.trim_end_matches('.');
    let numeric = trimmed
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '_');
    if numeric {
        return format!("<n>{}", &word[trimmed.len()..]);
    }
    let hexish = trimmed.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if (hexish && trimmed.len() >= 6) || digits * 2 >= trimmed.len() {
        return format!("<id>{}", &word[trimmed.len()..]);
    }
    // `req-42`: keep the letters, mask the digit runs.
    let mut out = String::with_capacity(word.len());
    let mut in_digits = false;
    for c in word.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push_str("<n>");
            }
            in_digits = true;
        } else {
            out.push(c);
            in_digits = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spec() -> LogSpec {
        let list: ListSpec = serde_json::from_value(json!({
            "items": [
                { "level": "INFO", "text": "started" },
                { "level": "error", "text": "connect to 10.0.0.5:5432 failed after 3 tries" },
                { "level": "error", "text": "connect to 10.0.0.7:5432 failed after 12 tries" },
                { "level": "warn", "text": "slow" },
                { "text": "plain" },
                { "level": "fatal", "text": "user \"bob\" missing id 9f8e7d6c" },
            ]
        }))
        .unwrap();
        LogSpec {
            path: Some("app.log".into()),
            format: LogFormat::Json,
            total_lines: 6,
            truncated: false,
            list,
            histogram: None,
        }
    }

    #[test]
    fn counts_levels_by_severity() {
        assert_eq!(
            spec().level_counts(),
            [
                ("fatal".into(), 1),
                ("error".into(), 2),
                ("warn".into(), 1),
                ("info".into(), 1),
                ("-".into(), 1)
            ]
        );
        assert_eq!(spec().errors(), [1, 2, 5]);
    }

    #[test]
    fn errors_group_by_shape() {
        let patterns = spec().error_patterns();
        assert_eq!(
            patterns[0],
            ("connect to <n>:<n> failed after <n> tries".into(), 2, 1)
        );
        assert_eq!(patterns[1].0, "user <s> missing id <id>");
        assert_eq!(pattern("req-42 done"), "req-<n> done");
        assert_eq!(pattern("took 1.5s."), "took <id>.");
        assert_eq!(pattern("it's fine"), "it's fine");
    }

    #[test]
    fn validates_the_histogram() {
        let mut logs = spec();
        logs.validate().unwrap();
        logs.histogram = Some(LogHistogram {
            start: 0.0,
            width: 0.0,
            buckets: vec![BucketCounts::default()],
        });
        assert!(logs.validate().is_err());
        logs.histogram = Some(LogHistogram {
            start: 0.0,
            width: 1.0,
            buckets: vec![BucketCounts {
                errors: 1,
                warnings: 2,
                other: 3,
            }],
        });
        logs.validate().unwrap();
        assert_eq!(logs.histogram.unwrap().buckets[0].total(), 6);
        logs = spec();
        logs.total_lines = 2;
        assert!(logs.validate().is_err());
    }
}
