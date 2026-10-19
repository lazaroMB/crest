//! The privacy layer: secrets and personal data are masked before anyone
//! reads them.
//!
//! Every tool result goes through [`Redactor::redact_output`] before the
//! budget cuts it and before the model, the timeline, the store or the
//! `after_tool` hooks see it; the user's prompts go through
//! [`Redactor::redact`] before routing and the model call. A masked value
//! becomes `[REDACTED:<kind>]` (`password = [REDACTED:secret]`), and the
//! result ends with a note so the model knows the placeholders are not
//! the real content. The file tools refuse to write text that carries a
//! placeholder, so a masked file is never saved over the real one.
//!
//! Secrets (API keys, tokens, JWTs, private keys, credentials in URLs,
//! `password = …` assignments) are always detected; personal data (emails,
//! cards, IBANs, national ids, international phone numbers) can be
//! switched off per category in [`PrivacyOptions`].

mod options;
mod rules;
#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, LazyLock};

use domain::conversation::ToolResultContent;
use regex::Regex;

pub use self::options::{
    CustomPattern, PRIVACY_OPTION, PiiCategories, PrivacyConfigError, PrivacyOptions, tighten,
};
use self::rules::{Category, Rule};
use crate::tools::registry::ToolOutput;

/// Start of every placeholder.
pub const PLACEHOLDER_PREFIX: &str = "[REDACTED:";

/// Whether `text` carries a placeholder of the redactor.
#[must_use]
pub fn contains_placeholder(text: &str) -> bool {
    text.contains(PLACEHOLDER_PREFIX)
}

static BUILTIN: LazyLock<Vec<Rule>> = LazyLock::new(rules::builtin);

/// Text after masking, and what was masked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redacted<'a> {
    /// The text with placeholders; borrowed when nothing was masked.
    pub text: Cow<'a, str>,
    /// Masked values by kind.
    pub counts: BTreeMap<String, usize>,
}

impl Redacted<'_> {
    /// Number of masked values.
    #[must_use]
    pub fn total(&self) -> usize {
        self.counts.values().sum()
    }
}

/// "2 api_key, 1 email", for notes and notices.
#[must_use]
pub fn describe(counts: &BTreeMap<String, usize>) -> String {
    counts
        .iter()
        .map(|(kind, n)| format!("{n} {kind}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Masks secrets and personal data; cheap to clone.
#[derive(Debug, Clone)]
pub struct Redactor {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    enabled: bool,
    pii: PiiCategories,
    custom: Vec<Rule>,
    allow: Vec<String>,
    prompts: bool,
}

impl Default for Redactor {
    /// Every built-in rule on, prompts included.
    fn default() -> Self {
        Self::from_valid(&PrivacyOptions::default(), Vec::new())
    }
}

impl Redactor {
    /// A redactor that masks nothing.
    #[must_use]
    pub fn disabled() -> Self {
        Self::from_valid(
            &PrivacyOptions {
                enabled: false,
                ..PrivacyOptions::default()
            },
            Vec::new(),
        )
    }

    /// The redactor configured by `options`.
    ///
    /// # Errors
    ///
    /// [`PrivacyConfigError::BadPattern`] when a pattern does not compile.
    pub fn new(options: &PrivacyOptions) -> Result<Self, PrivacyConfigError> {
        let custom = options
            .patterns
            .iter()
            .map(|p| p.compile().map(|regex| Rule::custom(&p.name, regex)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::from_valid(options, custom))
    }

    fn from_valid(options: &PrivacyOptions, custom: Vec<Rule>) -> Self {
        Self {
            inner: Arc::new(Inner {
                enabled: options.enabled,
                pii: options.pii,
                custom,
                allow: options.allow.clone(),
                prompts: options.prompts,
            }),
        }
    }

    /// Whether anything is masked at all.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.inner.enabled
    }

    /// Whether the user's prompts are masked too.
    #[must_use]
    pub fn masks_prompts(&self) -> bool {
        self.inner.enabled && self.inner.prompts
    }

    fn active(&self, rule: &Rule) -> bool {
        let pii = self.inner.pii;
        match rule.category {
            Category::Secret | Category::Custom => true,
            Category::Email => pii.email,
            Category::Card => pii.card,
            Category::Iban => pii.iban,
            Category::NationalId => pii.national_id,
            Category::Phone => pii.phone,
        }
    }

    /// `text` with every detected value replaced by its placeholder.
    #[must_use]
    pub fn redact<'a>(&self, text: &'a str) -> Redacted<'a> {
        let mut counts = BTreeMap::new();
        if !self.inner.enabled || text.is_empty() {
            return Redacted {
                text: Cow::Borrowed(text),
                counts,
            };
        }
        // (start, end, rule order) of every candidate.
        let mut spans: Vec<(usize, usize, usize, &str)> = Vec::new();
        let rules = BUILTIN.iter().chain(&self.inner.custom);
        for (order, rule) in rules.enumerate().filter(|(_, r)| self.active(r)) {
            for caps in rule.regex.captures_iter(text) {
                let Some(m) = caps.get(rule.group) else {
                    continue;
                };
                let value = m.as_str();
                if value.is_empty()
                    || contains_placeholder(value)
                    || rule.check.is_some_and(|check| !check(value))
                    || self.inner.allow.iter().any(|a| a == value)
                {
                    continue;
                }
                spans.push((m.start(), m.end(), order, rule.kind.as_str()));
            }
        }
        if spans.is_empty() {
            return Redacted {
                text: Cow::Borrowed(text),
                counts,
            };
        }
        // Earliest first, then the longest, then the more specific rule.
        spans.sort_by_key(|&(start, end, order, _)| (start, std::cmp::Reverse(end), order));
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0;
        for (start, end, _, kind) in spans {
            if start < cursor {
                continue;
            }
            out.push_str(&text[cursor..start]);
            let _ = write!(out, "{PLACEHOLDER_PREFIX}{kind}]");
            *counts.entry(kind.to_owned()).or_insert(0) += 1;
            cursor = end;
        }
        out.push_str(&text[cursor..]);
        Redacted {
            text: Cow::Owned(out),
            counts,
        }
    }

    /// `output` with its text items masked and, when something was, a
    /// note for the model at the end.
    #[must_use]
    pub fn redact_output(&self, mut output: ToolOutput) -> ToolOutput {
        self.redact_output_counted(&mut output);
        output
    }

    /// As [`redact_output`](Self::redact_output), in place; returns how
    /// many values were masked.
    pub fn redact_output_counted(&self, output: &mut ToolOutput) -> usize {
        if !self.inner.enabled {
            return 0;
        }
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut last_text = None;
        for (i, item) in output.content.iter_mut().enumerate() {
            if let ToolResultContent::Text { text } = item {
                last_text = Some(i);
                let redacted = self.redact(text);
                if let Cow::Owned(clean) = redacted.text {
                    for (kind, n) in redacted.counts {
                        *counts.entry(kind).or_insert(0) += n;
                    }
                    *text = clean;
                }
            }
        }
        let total = counts.values().sum();
        if let (true, Some(i)) = (total > 0, last_text)
            && let Some(ToolResultContent::Text { text }) = output.content.get_mut(i)
        {
            let _ = write!(
                text,
                "\n[crest redacted {total} sensitive value{}: {}; the {PLACEHOLDER_PREFIX}…] \
                 placeholders are not the real content]",
                if total == 1 { "" } else { "s" },
                describe(&counts),
            );
        }
        total
    }
}

/// Compiles a user pattern, for the options parser.
fn compile(pattern: &str) -> Result<Regex, regex::Error> {
    regex::RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .build()
}
