//! The YAML frontmatter of rules (`.claude/rules/*.md`) and skills
//! (`SKILL.md`, `.claude/commands/*.md`), as Claude Code writes it:
//!
//! ```text
//! ---
//! description: Review the staged changes
//! allowed-tools: Bash(git diff:*), Read
//! ---
//! The body...
//! ```
//!
//! The block starts on the first line with `---` and ends at the next line
//! that is `---`. Lists may be YAML lists or one string, split at commas
//! and spaces outside parentheses and braces (`Bash(git status:*)` and
//! `src/{a,b}/*.ts` stay whole).

use serde::Deserialize;
use serde::de::{DeserializeOwned, Deserializer};

/// The frontmatter of `text` (without its `---` lines) and the body after
/// it; `None` and the whole text when there is none.
#[must_use]
pub fn split(text: &str) -> (Option<&str>, &str) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(first) = text.lines().next() else {
        return (None, text);
    };
    if first.trim_end() != "---" {
        return (None, text);
    }
    let start = first.len() + line_break(&text[first.len()..]);
    let mut at = start;
    for line in text[start..].split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if bare.trim_end() == "---" {
            let yaml = &text[start..at];
            let body = &text[at + line.len()..];
            return (Some(yaml), body);
        }
        at += line.len();
    }
    // An opening line without a closing one is text.
    (None, text)
}

/// The length of the line break at the start of `rest` (0, 1 or 2).
fn line_break(rest: &str) -> usize {
    if rest.starts_with("\r\n") {
        2
    } else {
        usize::from(rest.starts_with('\n'))
    }
}

/// The frontmatter of `text` read as `T`, and the body. A text without
/// frontmatter gives `T::default()`; invalid YAML gives the error (and
/// the default) so the caller can warn.
pub fn parse<T: DeserializeOwned + Default>(text: &str) -> (Result<T, String>, &str) {
    let (yaml, body) = split(text);
    let Some(yaml) = yaml else {
        return (Ok(T::default()), body);
    };
    if yaml.trim().is_empty() {
        return (Ok(T::default()), body);
    }
    let parsed = serde_norway::from_str::<Option<T>>(yaml)
        .map(Option::unwrap_or_default)
        .map_err(|e| e.to_string());
    (parsed, body)
}

/// A list field that may also be one string (`paths: src/**/*.rs, lib/**`)
/// or absent.
///
/// # Errors
/// When the value is neither a string nor a list of strings.
pub fn list<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Option::<Raw>::deserialize(de)? {
        None => Vec::new(),
        Some(Raw::One(text)) => split_list(&text),
        Some(Raw::Many(items)) => items
            .iter()
            .map(|item| item.trim().to_owned())
            .filter(|item| !item.is_empty())
            .collect(),
    })
}

/// A boolean field that may also be written as a string (`"true"`).
///
/// # Errors
/// When the value is not a boolean.
pub fn flag<'de, D: Deserializer<'de>>(de: D) -> Result<Option<bool>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Bool(bool),
        Text(String),
    }
    match Option::<Raw>::deserialize(de)? {
        None => Ok(None),
        Some(Raw::Bool(value)) => Ok(Some(value)),
        Some(Raw::Text(text)) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" => Ok(Some(true)),
            "false" | "no" | "off" => Ok(Some(false)),
            other => Err(serde::de::Error::custom(format!(
                "expected true or false, found {other:?}"
            ))),
        },
    }
}

/// `text` split at commas and whitespace outside `()`, `[]` and `{}`.
#[must_use]
pub fn split_list(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut item = String::new();
    for c in text.chars() {
        match c {
            '(' | '{' | '[' => {
                depth += 1;
                item.push(c);
            }
            ')' | '}' | ']' => {
                depth = depth.saturating_sub(1);
                item.push(c);
            }
            ',' if depth == 0 => push_item(&mut out, &mut item),
            c if c.is_whitespace() && depth == 0 => push_item(&mut out, &mut item),
            c => item.push(c),
        }
    }
    push_item(&mut out, &mut item);
    out
}

fn push_item(out: &mut Vec<String>, item: &mut String) {
    let trimmed = item.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_owned());
    }
    item.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, Deserialize, PartialEq)]
    struct Front {
        #[serde(default, deserialize_with = "list")]
        paths: Vec<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default, deserialize_with = "flag")]
        on: Option<bool>,
    }

    #[test]
    fn splits_the_block_from_the_body() {
        let (yaml, body) = split("---\npaths: a\n---\nBody\n");
        assert_eq!(yaml, Some("paths: a\n"));
        assert_eq!(body, "Body\n");
        let (yaml, body) = split("---\r\nx: 1\r\n---\r\nBody");
        assert_eq!(yaml, Some("x: 1\r\n"));
        assert_eq!(body, "Body");
        assert_eq!(split("No front\n---\n"), (None, "No front\n---\n"));
        assert_eq!(split("---\nnever closed"), (None, "---\nnever closed"));
        assert_eq!(split("\u{feff}---\n---\nB"), (Some(""), "B"));
    }

    #[test]
    fn reads_lists_as_lists_or_strings() {
        let (front, body) = parse::<Front>("---\npaths:\n  - src/**\n  - lib/*.rs\n---\nB");
        assert_eq!(front.unwrap().paths, ["src/**", "lib/*.rs"]);
        assert_eq!(body, "B");
        let (front, _) = parse::<Front>("---\npaths: \"src/{a,b}/*.ts, docs/**\"\n---\n");
        assert_eq!(front.unwrap().paths, ["src/{a,b}/*.ts", "docs/**"]);
        let (front, _) = parse::<Front>("---\ndescription: hi\non: \"yes\"\n---\n");
        let front = front.unwrap();
        assert_eq!(front.paths, Vec::<String>::new());
        assert_eq!(front.on, Some(true));
    }

    #[test]
    fn invalid_yaml_is_an_error_and_no_block_is_the_default() {
        let (front, body) = parse::<Front>("---\npaths: [unclosed\n---\nBody");
        assert!(front.is_err());
        assert_eq!(body, "Body");
        let (front, body) = parse::<Front>("Just text");
        assert_eq!(front.unwrap(), Front::default());
        assert_eq!(body, "Just text");
        let (front, _) = parse::<Front>("---\n---\n");
        assert_eq!(front.unwrap(), Front::default());
    }

    #[test]
    fn split_list_keeps_parentheses_whole() {
        assert_eq!(
            split_list("Bash(git status:*), Read  Edit,Bash(npm run *)"),
            ["Bash(git status:*)", "Read", "Edit", "Bash(npm run *)"]
        );
    }
}
