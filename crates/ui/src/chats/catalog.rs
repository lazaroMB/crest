//! What the sidebar lists, as the binary fills it from the store.

/// Where a branch came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Branch {
    /// The title of the chat it was branched from; empty once that chat
    /// was deleted.
    pub from: String,
    /// How many blocks it copied.
    pub after: usize,
}

impl Branch {
    /// The line the timeline shows at the top of a branch.
    #[must_use]
    pub fn banner(&self) -> String {
        let blocks = if self.after == 1 { "block" } else { "blocks" };
        let source = if self.from.is_empty() {
            "a chat that no longer exists".to_owned()
        } else {
            format!("\"{}\"", self.from)
        };
        format!(
            "Branched from {source} after {} {blocks}. The original chat is unchanged.",
            self.after
        )
    }
}

/// One saved chat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatEntry {
    /// Session id, as printed.
    pub id: String,
    /// What the row shows.
    pub title: String,
    /// Whether it is listed first in its section.
    pub pinned: bool,
    /// Where it was branched from, if it is a branch.
    pub branch: Option<Branch>,
}

impl ChatEntry {
    /// A chat that is not pinned and not a branch.
    #[must_use]
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            ..Self::default()
        }
    }
}

/// A project and its chats, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectChats {
    /// Project id, as printed (a directory without a project uses its root).
    pub id: String,
    /// Name shown on the row.
    pub name: String,
    /// Its first directory, as stored: where its chats open.
    pub root: String,
    /// All its directories, in order (the first is `root`).
    pub roots: Vec<String>,
    /// Whether it is listed first.
    pub pinned: bool,
    /// Its chats, newest first.
    pub chats: Vec<ChatEntry>,
}

impl ProjectChats {
    /// How many directories it has.
    #[must_use]
    pub fn dirs(&self) -> usize {
        self.roots.len().max(1)
    }
}

/// An archived project: hidden with its chats until it is restored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchivedProject {
    /// Project id, as printed.
    pub id: String,
    /// Its name.
    pub name: String,
    /// How many chats it keeps.
    pub chats: usize,
}

/// What the "Archived" section lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Archive {
    /// Archived projects.
    pub projects: Vec<ArchivedProject>,
    /// Archived chats of a project that is not archived, or of none.
    pub chats: Vec<ChatEntry>,
}

impl Archive {
    /// How many rows the section has, for its "Archived (N)" label.
    #[must_use]
    pub fn count(&self) -> usize {
        self.projects.len() + self.chats.len()
    }
}

/// What the sidebar lists: the projects in their order, and the chats that
/// are in none, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    /// Projects, in the order the sidebar shows them.
    pub projects: Vec<ProjectChats>,
    /// Chats with no project.
    pub chats: Vec<ChatEntry>,
    /// What is archived.
    pub archived: Archive,
}

impl Catalog {
    /// The chat `id`, in a project or in none.
    #[must_use]
    pub fn chat(&self, id: &str) -> Option<&ChatEntry> {
        let in_projects = self.projects.iter().flat_map(|project| &project.chats);
        in_projects.chain(&self.chats).find(|chat| chat.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_says_where_it_came_from_and_how_much_it_copied() {
        let cases = [
            (
                "Fix",
                3,
                "Branched from \"Fix\" after 3 blocks. The original chat is unchanged.",
            ),
            (
                "Fix",
                1,
                "Branched from \"Fix\" after 1 block. The original chat is unchanged.",
            ),
            (
                "",
                0,
                "Branched from a chat that no longer exists after 0 blocks. The original chat is unchanged.",
            ),
        ];
        for (from, after, want) in cases {
            let branch = Branch {
                from: from.into(),
                after,
            };
            assert_eq!(branch.banner(), want);
        }
    }
}
