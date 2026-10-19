//! What the project picker does with the path the user types: the
//! directory it lists and the filter of its entries.
//!
//! `~/Projects/cre` lists `~/Projects` and keeps the entries matching
//! `cre`; a trailing `/` lists the directory itself with no filter.

use crate::ports::projects::DirEntry;

/// A typed path split at its last `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedPath<'a> {
    /// The directory part, with its trailing `/` (`~/Projects/`), or `""`
    /// when nothing was typed before the filter.
    pub dir: &'a str,
    /// What follows the last `/`: filters the entries of `dir`.
    pub filter: &'a str,
}

/// Splits `input` at its last `/`. A bare `~` is the home directory.
#[must_use]
pub fn split_typed_path(input: &str) -> TypedPath<'_> {
    if input == "~" {
        return TypedPath {
            dir: input,
            filter: "",
        };
    }
    let split = input.rfind('/').map_or(0, |ix| ix + 1);
    TypedPath {
        dir: &input[..split],
        filter: &input[split..],
    }
}

/// The entries matching `filter`, best first: names starting with it
/// (ignoring case), then names containing it; projects before other
/// directories within each group, then by name. Hidden entries only show
/// once the filter starts with a `.`.
#[must_use]
pub fn filter_dirs<'a>(entries: &'a [DirEntry], filter: &str) -> Vec<&'a DirEntry> {
    let typed = filter.to_lowercase();
    let mut ranked: Vec<(u8, String, &DirEntry)> = entries
        .iter()
        .filter(|entry| typed.starts_with('.') || !entry.hidden)
        .filter_map(|entry| {
            let name = entry.name.to_lowercase();
            let rank = if name.starts_with(&typed) {
                0
            } else if name.contains(&typed) {
                1
            } else {
                return None;
            };
            Some((rank, name, entry))
        })
        .collect();
    ranked.sort_by(|(a_rank, a_name, a), (b_rank, b_name, b)| {
        a_rank
            .cmp(b_rank)
            .then(b.is_project.cmp(&a.is_project))
            .then_with(|| a_name.cmp(b_name))
    });
    ranked.into_iter().map(|(_, _, entry)| entry).collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn dir(name: &str, is_project: bool) -> DirEntry {
        DirEntry {
            name: name.into(),
            path: PathBuf::from("/home/u").join(name),
            hidden: name.starts_with('.'),
            is_project,
        }
    }

    #[test]
    fn typed_paths_split_at_the_last_slash() {
        let split = |input| {
            let t = split_typed_path(input);
            (t.dir, t.filter)
        };
        assert_eq!(split("~/Proj/cre"), ("~/Proj/", "cre"));
        assert_eq!(split("~/Proj/"), ("~/Proj/", ""));
        assert_eq!(split("~"), ("~", ""));
        assert_eq!(split("/"), ("/", ""));
        assert_eq!(split("/usr"), ("/", "usr"));
        assert_eq!(split("src"), ("", "src"));
        assert_eq!(split(""), ("", ""));
    }

    #[test]
    fn projects_and_prefixes_come_first() {
        let entries = [
            dir("notes", false),
            dir("crest", true),
            dir("Crates", false),
            dir("old-crest", true),
            dir(".cache", false),
        ];
        let names = |filter| -> Vec<&str> {
            filter_dirs(&entries, filter)
                .into_iter()
                .map(|e| e.name.as_str())
                .collect()
        };
        assert_eq!(names(""), ["crest", "old-crest", "Crates", "notes"]);
        assert_eq!(names("cr"), ["crest", "Crates", "old-crest"]);
        assert_eq!(names(".c"), [".cache"]);
        assert!(names("zzz").is_empty());
    }
}
