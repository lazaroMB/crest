//! The sidebar's pure model: its three widths and what a search leaves in
//! the list. No GPUI here, so the rules are plain unit tests.
//!
//! Today each directory is one project (named after it, with one
//! directory), the catalog arrives with the projects by recent use and
//! each project's chats newest first; the model keeps that order and
//! filters it. Pinned projects, standalone chats and the archive have no
//! data yet: they join [`visible_projects`] (its input and output) when
//! the store has them.

use super::{Catalog, ChatEntry, ProjectChats, project_name, same_workspace};

/// Width of the sidebar in each state, in pixels.
pub const FULL_WIDTH: f32 = 240.0;
/// The strip: a fixed column of icons.
pub const STRIP_WIDTH: f32 = 48.0;
/// Height of the band the Crest mark takes while the sidebar is hidden.
pub const MARK_BAND: f32 = 28.0;

/// A state of a control on the rail (an icon button or a project initial).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailState {
    /// Nothing on it.
    Idle,
    /// The pointer is on it.
    Hover,
    /// It is the open project, or it is pressed.
    Active,
}

/// The theme tokens a rail control is drawn with in `state`: its ink
/// (icon or initial) and the fill under it. The ink is one token in every
/// state and only the fill changes, so no state can paint the ink in the
/// colour of its own fill; a test checks the contrast in every theme.
#[must_use]
pub const fn rail_tokens(state: RailState) -> (&'static str, &'static str) {
    match state {
        RailState::Idle => ("sidebar.fg", "sidebar.bg"),
        RailState::Hover | RailState::Active => ("sidebar.fg", "list.selected.bg"),
    }
}

/// How much of the window the sidebar takes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SidebarWidth {
    /// The list of projects and chats.
    Full,
    /// A narrow column: the mark, new chat, search and one initial per project.
    Strip,
    /// Only the Crest mark, top left. The window opens like this.
    #[default]
    Hidden,
}

impl SidebarWidth {
    /// `panel.chats`: full, strip, hidden, and full again.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Full => Self::Strip,
            Self::Strip => Self::Hidden,
            Self::Hidden => Self::Full,
        }
    }

    /// The column the sidebar takes beside the conversation; hidden takes
    /// none (its mark floats over the top left).
    #[must_use]
    pub const fn column(self) -> f32 {
        match self {
            Self::Full => FULL_WIDTH,
            Self::Strip => STRIP_WIDTH,
            Self::Hidden => 0.0,
        }
    }
}

/// The chat that is open, as the list must show it even before the store
/// lists it.
#[derive(Debug, Clone, Copy)]
pub struct CurrentChat<'a> {
    /// Its first directory; empty for a chat with none.
    pub root: &'a str,
    /// Its project's id, when known.
    pub project: Option<&'a str>,
    /// Its id, once it has one.
    pub id: Option<&'a str>,
}

impl CurrentChat<'_> {
    /// Whether `project` is the open chat's: by id when it has one, else by
    /// directory.
    pub fn is_in(&self, project: &ProjectChats) -> bool {
        match self.project {
            Some(id) => project.id == id,
            None => !self.root.is_empty() && same_workspace(&project.root, self.root),
        }
    }
}

/// `catalog` plus the open project and chat, when it does not list them
/// yet: a chat with a directory goes in its project (made on the spot), one
/// with none among the chats in no project.
#[must_use]
pub fn listed_with(mut catalog: Catalog, current: &CurrentChat<'_>) -> Catalog {
    let id = current.id.filter(|id| !id.is_empty());
    let loose = current.root.is_empty() && current.project.is_none();
    if let Some(id) = id
        && catalog
            .projects
            .iter()
            .flat_map(|project| &project.chats)
            .chain(&catalog.chats)
            .any(|chat| chat.id == id)
    {
        return catalog;
    }
    let fresh = id.map(|id| ChatEntry {
        id: id.to_owned(),
        title: "New chat".into(),
    });
    if loose {
        catalog.chats.splice(0..0, fresh);
        return catalog;
    }
    if !catalog
        .projects
        .iter()
        .any(|project| current.is_in(project))
    {
        let id = current.project.unwrap_or(current.root).to_owned();
        catalog.projects.insert(
            0,
            ProjectChats {
                id,
                name: project_name(current.root),
                root: current.root.to_owned(),
                dirs: 1,
                chats: Vec::new(),
            },
        );
    }
    if let Some(project) = catalog
        .projects
        .iter_mut()
        .find(|project| current.is_in(project))
    {
        project.chats.splice(0..0, fresh);
    }
    catalog
}

/// The chats in no project that a search leaves: those whose title matches.
#[must_use]
pub fn visible_chats(chats: &[ChatEntry], query: &str) -> Vec<ChatEntry> {
    let query = query.trim().to_lowercase();
    chats
        .iter()
        .filter(|chat| chat.title.to_lowercase().contains(&query))
        .cloned()
        .collect()
}

/// The projects a search leaves, in their order. A query matching a
/// project's name keeps all its chats; otherwise only the chats whose
/// title matches, and a project with none disappears. Case is ignored; an
/// empty query leaves everything.
#[must_use]
pub fn visible_projects(projects: &[ProjectChats], query: &str) -> Vec<ProjectChats> {
    let query = query.trim().to_lowercase();
    let matches = |text: &str| text.to_lowercase().contains(&query);
    projects
        .iter()
        .filter_map(|project| {
            if query.is_empty() || matches(&project.name) {
                return Some(project.clone());
            }
            let chats: Vec<_> = project
                .chats
                .iter()
                .filter(|chat| matches(&chat.title))
                .cloned()
                .collect();
            (!chats.is_empty()).then(|| ProjectChats {
                chats,
                ..project.clone()
            })
        })
        .collect()
}

/// The letter the strip shows for a project: the first of its name,
/// uppercased (`?` for an empty name).
#[must_use]
pub fn initial(name: &str) -> String {
    name.chars()
        .next()
        .map_or_else(|| "?".to_owned(), |c| c.to_uppercase().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chats::ChatEntry;

    fn chat(title: &str) -> ChatEntry {
        ChatEntry {
            id: title.into(),
            title: title.into(),
        }
    }

    fn project(name: &str, titles: &[&str]) -> ProjectChats {
        ProjectChats {
            id: format!("id-{name}"),
            name: name.into(),
            root: format!("/w/{name}"),
            dirs: 1,
            chats: titles
                .iter()
                .map(|title| ChatEntry {
                    id: (*title).into(),
                    title: (*title).into(),
                })
                .collect(),
        }
    }

    #[test]
    fn the_cycle_goes_full_strip_hidden_and_back() {
        let mut width = SidebarWidth::default();
        assert_eq!(width, SidebarWidth::Hidden, "the window opens hidden");
        let seen: Vec<_> = (0..4)
            .map(|_| {
                width = width.next();
                width
            })
            .collect();
        use SidebarWidth::{Full, Hidden, Strip};
        assert_eq!(seen, [Full, Strip, Hidden, Full]);
        assert_eq!(
            [Full.column(), Strip.column(), Hidden.column()],
            [240.0, 48.0, 0.0]
        );
    }

    #[test]
    fn a_search_filters_chats_by_title_and_keeps_the_order() {
        let all = [
            project("crest", &["Fix add", "Release v0.6", "Rename add"]),
            project("notes", &["Parser"]),
            project("site", &["Hero copy"]),
        ];
        let names = |query: &str| -> Vec<(String, Vec<String>)> {
            visible_projects(&all, query)
                .into_iter()
                .map(|p| (p.name, p.chats.into_iter().map(|c| c.title).collect()))
                .collect()
        };
        assert_eq!(names("").len(), 3, "no query keeps everything");
        assert_eq!(
            names("  ADD "),
            [("crest".into(), vec!["Fix add".into(), "Rename add".into()])],
            "case and edge spaces do not matter; others drop out"
        );
        assert_eq!(names("notes"), [("notes".into(), vec!["Parser".into()])]);
        assert!(names("zzz").is_empty());
        // The chats in no project filter by title alone.
        let loose = [chat("Fix add"), chat("Parser")];
        let kept = visible_chats(&loose, "  ADD ");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].title, "Fix add");
        assert_eq!(visible_chats(&loose, "").len(), 2);
    }

    #[test]
    fn a_project_shows_its_first_letter() {
        assert_eq!(initial("crest"), "C");
        assert_eq!(initial("élan"), "É");
        assert_eq!(initial(""), "?");
    }
}
