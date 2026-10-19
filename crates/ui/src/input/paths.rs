//! `@` path completion: the last word of the prompt, when it starts with
//! `@`, names a workspace path (`/open @src/ma`, or a reference in a
//! prompt). The text up to its last `/` is the directory listed; the rest
//! filters the entries.
//!
//! Suggestions are always rooted at the workspace root: an absolute path
//! or one that climbs out with `..` gets no listing, only a message (and
//! `/open` refuses it).

use domain::workspace::WorkspacePath;

/// An `@` word at the end of the prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathToken<'a> {
    /// Byte offset of the `@` in the prompt.
    pub start: usize,
    /// The directory part as typed, with its trailing `/` (`src/`), or
    /// `""` for the root.
    pub dir_text: &'a str,
    /// What follows the last `/`: filters the entries of the directory.
    pub prefix: &'a str,
}

/// The directory a [`PathToken`] lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenDir {
    /// A directory inside the workspace.
    Inside(WorkspacePath),
    /// Absolute, or climbing out of the root with `..`.
    Outside,
}

/// One entry of a listed directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathEntry {
    /// File or directory name.
    pub name: String,
    /// Whether it is a directory.
    pub dir: bool,
}

/// The `@` word the prompt ends with, if any. A trailing space ends the
/// word, so a completed file (`@src/main.rs `) shows nothing.
pub fn path_token(text: &str) -> Option<PathToken<'_>> {
    let start = text
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(ix, c)| ix + c.len_utf8());
    let rest = text[start..].strip_prefix('@')?;
    let split = rest.rfind('/').map_or(0, |ix| ix + 1);
    Some(PathToken {
        start,
        dir_text: &rest[..split],
        prefix: &rest[split..],
    })
}

/// The directory `token` lists.
pub fn token_dir(token: &PathToken<'_>) -> TokenDir {
    let dir = token.dir_text.trim_end_matches('/');
    if dir.is_empty() && token.dir_text.is_empty() {
        return TokenDir::Inside(WorkspacePath::root());
    }
    if token.dir_text.starts_with('/') || dir.split('/').any(|part| part == "..") {
        return TokenDir::Outside;
    }
    WorkspacePath::parse(dir).map_or(TokenDir::Outside, TokenDir::Inside)
}

/// The entries matching `prefix`, best first: names starting with it
/// (ignoring case), then names containing it; directories before files
/// within each group, then by name ignoring case. Hidden entries only show once the prefix starts
/// with a `.`.
pub fn path_matches<'a>(entries: &'a [PathEntry], prefix: &str) -> Vec<&'a PathEntry> {
    let typed = prefix.to_lowercase();
    let mut ranked: Vec<(u8, &PathEntry)> = entries
        .iter()
        .filter(|entry| typed.starts_with('.') || !entry.name.starts_with('.'))
        .filter_map(|entry| {
            let name = entry.name.to_lowercase();
            if name.starts_with(&typed) {
                Some((0, entry))
            } else if name.contains(&typed) {
                Some((1, entry))
            } else {
                None
            }
        })
        .collect();
    ranked.sort_by(|(a_rank, a), (b_rank, b)| {
        a_rank
            .cmp(b_rank)
            .then(b.dir.cmp(&a.dir))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    ranked.into_iter().map(|(_, entry)| entry).collect()
}

/// The prompt with `token` replaced by `entry`: a directory ends in `/`
/// (so its entries are listed next), a file in a space.
pub fn complete(text: &str, token: &PathToken<'_>, entry: &PathEntry) -> String {
    let end = if entry.dir { "/" } else { " " };
    format!(
        "{}@{}{}{end}",
        &text[..token.start],
        token.dir_text,
        entry.name
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, dir: bool) -> PathEntry {
        PathEntry {
            name: name.into(),
            dir,
        }
    }

    #[test]
    fn the_last_at_word_is_the_token() {
        assert_eq!(path_token("hello"), None);
        assert_eq!(path_token("/open @src/main.rs "), None);
        assert_eq!(path_token("mail a@b"), None);
        assert_eq!(
            path_token("@"),
            Some(PathToken {
                start: 0,
                dir_text: "",
                prefix: ""
            })
        );
        assert_eq!(
            path_token("/open @src/comp"),
            Some(PathToken {
                start: 6,
                dir_text: "src/",
                prefix: "comp"
            })
        );
        assert_eq!(
            path_token("see\t@src/ui/"),
            Some(PathToken {
                start: 4,
                dir_text: "src/ui/",
                prefix: ""
            })
        );
        assert_eq!(path_token("ñ @a").map(|t| t.start), Some(3));
    }

    #[test]
    fn the_listed_directory_stays_inside_the_root() {
        let dir = |text: &str| token_dir(&path_token(text).unwrap());
        assert_eq!(dir("@ma"), TokenDir::Inside(WorkspacePath::root()));
        assert_eq!(
            dir("@src/components/x"),
            TokenDir::Inside(WorkspacePath::parse("src/components").unwrap())
        );
        assert_eq!(
            dir("@./src/"),
            TokenDir::Inside(WorkspacePath::parse("src").unwrap())
        );
        assert_eq!(dir("@../"), TokenDir::Outside);
        assert_eq!(dir("@src/../../"), TokenDir::Outside);
        assert_eq!(dir("@src/../"), TokenDir::Outside);
        assert_eq!(dir("@/etc/"), TokenDir::Outside);
        assert_eq!(dir("@/"), TokenDir::Outside);
    }

    #[test]
    fn matches_rank_prefix_then_substring_and_directories_first() {
        let entries = [
            entry("main.rs", false),
            entry("domain", true),
            entry("Makefile", false),
            entry("macros", true),
            entry(".mailmap", false),
        ];
        let names = |prefix: &str| -> Vec<&str> {
            path_matches(&entries, prefix)
                .into_iter()
                .map(|e| e.name.as_str())
                .collect()
        };
        assert_eq!(names("ma"), ["macros", "main.rs", "Makefile", "domain"]);
        assert_eq!(names(""), ["domain", "macros", "main.rs", "Makefile"]);
        assert_eq!(names(".m"), [".mailmap"]);
        assert!(names("zzz").is_empty());
    }

    #[test]
    fn completing_keeps_the_text_before_the_token() {
        let text = "/open @src/co";
        let token = path_token(text).unwrap();
        assert_eq!(
            complete(text, &token, &entry("components", true)),
            "/open @src/components/"
        );
        assert_eq!(
            complete(text, &token, &entry("config.rs", false)),
            "/open @src/config.rs "
        );
    }
}
