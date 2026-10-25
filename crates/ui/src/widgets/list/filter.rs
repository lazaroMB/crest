//! Filtering a list: the text query (plain or `/regex/`), the facet chips
//! and the lines the list shows (items, and the fields of opened items).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use domain::widget::list::{LevelClass, ListItem, ListSpec, value_text};
use regex::{Regex, RegexBuilder};

/// Name of the facet over items' levels.
pub const LEVEL_FACET: &str = "level";
/// Chips a facet shows at most (the most frequent values).
pub const MAX_CHIPS: usize = 12;
/// Longest regex accepted, in bytes of compiled program.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// A parsed filter text.
#[derive(Debug, Clone, Default)]
pub enum Query {
    /// Empty: everything passes.
    #[default]
    All,
    /// A substring; case-insensitive unless it has an uppercase letter.
    Text {
        /// The text, lowercased when `insensitive`.
        needle: String,
        /// Whether case is ignored.
        insensitive: bool,
    },
    /// `/pattern/` or `/pattern/i`.
    Regex(Regex),
}

impl Query {
    /// Parses `input`: `/…/` (optionally with an `i` flag) is a regex,
    /// anything else a substring.
    ///
    /// # Errors
    ///
    /// The regex compiler's message when the pattern is invalid.
    pub fn parse(input: &str) -> Result<Self, String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(Self::All);
        }
        let regex = trimmed
            .strip_prefix('/')
            .and_then(|rest| {
                rest.strip_suffix("/i")
                    .map(|p| (p, true))
                    .or_else(|| rest.strip_suffix('/').map(|p| (p, false)))
            })
            .filter(|(pattern, _)| !pattern.is_empty());
        if let Some((pattern, insensitive)) = regex {
            return RegexBuilder::new(pattern)
                .case_insensitive(insensitive)
                .size_limit(REGEX_SIZE_LIMIT)
                .build()
                .map(Self::Regex)
                .map_err(|e| e.to_string());
        }
        let insensitive = !trimmed.chars().any(char::is_uppercase);
        Ok(Self::Text {
            needle: if insensitive {
                trimmed.to_lowercase()
            } else {
                trimmed.to_owned()
            },
            insensitive,
        })
    }

    fn matches_text(&self, text: &str) -> bool {
        match self {
            Self::All => true,
            Self::Text {
                needle,
                insensitive: true,
            } => text.to_lowercase().contains(needle.as_str()),
            Self::Text { needle, .. } => text.contains(needle.as_str()),
            Self::Regex(regex) => regex.is_match(text),
        }
    }

    /// Whether `item` matches in its text, timestamp, level or any field
    /// value.
    pub fn matches(&self, item: &ListItem) -> bool {
        if matches!(self, Self::All) {
            return true;
        }
        self.matches_text(&item.text)
            || item.ts.as_deref().is_some_and(|ts| self.matches_text(ts))
            || item.level.as_deref().is_some_and(|l| self.matches_text(l))
            || item
                .fields
                .values()
                .any(|value| self.matches_text(&value_text(value)))
    }
}

/// The chips chosen per facet; a facet with none chosen lets every item
/// through.
pub type Selection = BTreeMap<String, BTreeSet<String>>;

/// The value of facet `facet` of `item`.
pub fn facet_value(item: &ListItem, facet: &str) -> Option<String> {
    if facet == LEVEL_FACET {
        item.level.clone()
    } else {
        item.field_text(facet)
    }
}

/// Whether `item` has one of the chosen values of every facet with a
/// choice.
pub fn passes(item: &ListItem, selection: &Selection) -> bool {
    selection.iter().all(|(facet, chosen)| {
        chosen.is_empty() || facet_value(item, facet).is_some_and(|v| chosen.contains(&v))
    })
}

/// Indices of the items that match `query` and `selection`, in order.
pub fn filter(spec: &ListSpec, query: &Query, selection: &Selection) -> Vec<usize> {
    spec.items
        .iter()
        .enumerate()
        .filter(|(_, item)| passes(item, selection) && query.matches(item))
        .map(|(ix, _)| ix)
        .collect()
}

/// A facet and its chips.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facet {
    /// Field name, or [`LEVEL_FACET`].
    pub name: String,
    /// Values with their counts over all items.
    pub values: Vec<(String, usize)>,
    /// Distinct values left out past [`MAX_CHIPS`].
    pub more: usize,
}

/// The facets of `spec`: the level first (by severity) when any item has
/// one, then the declared facets (by count, then name).
pub fn facets(spec: &ListSpec) -> Vec<Facet> {
    let count = |facet: &str| -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        for value in spec
            .items
            .iter()
            .filter_map(|item| facet_value(item, facet))
        {
            *counts.entry(value).or_insert(0) += 1;
        }
        counts
    };
    let mut out = Vec::new();
    let levels = count(LEVEL_FACET);
    if !levels.is_empty() {
        let mut values: Vec<(String, usize)> = levels.into_iter().collect();
        values.sort_by(|a, b| {
            LevelClass::of(&a.0)
                .cmp(&LevelClass::of(&b.0))
                .then_with(|| a.0.cmp(&b.0))
        });
        out.push(capped(LEVEL_FACET, values));
    }
    for name in &spec.facets {
        if name == LEVEL_FACET {
            continue;
        }
        let mut values: Vec<(String, usize)> = count(name).into_iter().collect();
        values.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out.push(capped(name, values));
    }
    out
}

fn capped(name: &str, mut values: Vec<(String, usize)>) -> Facet {
    let more = values.len().saturating_sub(MAX_CHIPS);
    values.truncate(MAX_CHIPS);
    Facet {
        name: name.to_owned(),
        values,
        more,
    }
}

/// One line of the list: an item, or one field of an opened item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// Item index.
    Item(usize),
    /// Item index and field position.
    Field(usize, usize),
}

/// The lines for the `visible` items, opened ones followed by their
/// fields.
pub fn lines(spec: &ListSpec, visible: &[usize], open: &HashSet<usize>) -> Vec<Line> {
    let mut out = Vec::with_capacity(visible.len());
    for &ix in visible {
        out.push(Line::Item(ix));
        if open.contains(&ix)
            && let Some(item) = spec.items.get(ix)
        {
            out.extend((0..item.fields.len()).map(|field| Line::Field(ix, field)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spec() -> ListSpec {
        serde_json::from_value(json!({
            "facets": ["service"],
            "items": [
                { "level": "info", "ts": "12:00:00", "text": "started", "fields": { "service": "api" } },
                { "level": "error", "text": "Connection refused", "fields": { "service": "db", "port": 5432 } },
                { "level": "warn", "text": "slow query", "fields": { "service": "db" } },
                { "level": "info", "text": "GET /health 200", "fields": { "service": "api" } },
                { "text": "no level" }
            ]
        }))
        .unwrap()
    }

    fn run(text: &str, selection: &Selection) -> Vec<usize> {
        filter(&spec(), &Query::parse(text).unwrap(), selection)
    }

    #[test]
    fn substrings_use_smart_case_and_search_fields() {
        let none = Selection::new();
        assert_eq!(run("", &none), [0, 1, 2, 3, 4]);
        assert_eq!(run("connection", &none), [1]);
        assert_eq!(run("Connection", &none), [1]);
        assert_eq!(run("CONNECTION", &none), Vec::<usize>::new());
        assert_eq!(run("5432", &none), [1]);
        assert_eq!(run("12:00", &none), [0]);
        assert_eq!(run("ERROR", &none), Vec::<usize>::new());
        assert_eq!(run("error", &none), [1]);
    }

    #[test]
    fn slashes_make_a_regex() {
        let none = Selection::new();
        assert_eq!(run(r"/^GET \S+ 2\d\d$/", &none), [3]);
        assert_eq!(run("/SLOW/i", &none), [2]);
        assert_eq!(run("/SLOW/", &none), Vec::<usize>::new());
        assert!(Query::parse("/(unclosed/").is_err());
        // A lone slash is text.
        assert_eq!(run("/", &none), [3]);
    }

    #[test]
    fn chips_narrow_per_facet() {
        let mut selection = Selection::new();
        selection
            .entry("service".into())
            .or_default()
            .insert("db".into());
        assert_eq!(run("", &selection), [1, 2]);
        selection
            .entry(LEVEL_FACET.into())
            .or_default()
            .insert("warn".into());
        assert_eq!(run("", &selection), [2]);
        selection
            .entry(LEVEL_FACET.into())
            .or_default()
            .insert("error".into());
        assert_eq!(run("", &selection), [1, 2]);
        assert_eq!(run("refused", &selection), [1]);
    }

    #[test]
    fn facets_count_values() {
        let facets = facets(&spec());
        assert_eq!(facets[0].name, LEVEL_FACET);
        assert_eq!(
            facets[0].values,
            [
                ("error".to_owned(), 1),
                ("warn".to_owned(), 1),
                ("info".to_owned(), 2)
            ]
        );
        assert_eq!(facets[1].name, "service");
        assert_eq!(
            facets[1].values,
            [("api".to_owned(), 2), ("db".to_owned(), 2)]
        );
        let many: ListSpec = serde_json::from_value(json!({
            "facets": ["n"],
            "items": (0..20).map(|n| json!({ "text": "x", "fields": { "n": n } })).collect::<Vec<_>>()
        }))
        .unwrap();
        let facets = super::facets(&many);
        assert_eq!((facets[0].values.len(), facets[0].more), (MAX_CHIPS, 8));
    }

    #[test]
    fn opened_items_list_their_fields() {
        let spec = spec();
        let open = HashSet::from([1]);
        assert_eq!(
            lines(&spec, &[0, 1, 2], &open),
            [
                Line::Item(0),
                Line::Item(1),
                Line::Field(1, 0),
                Line::Field(1, 1),
                Line::Item(2)
            ]
        );
    }
}
