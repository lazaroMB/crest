//! The sidebar's pure model: its three widths and what a search leaves in
//! the list. No GPUI here, so the rules are plain unit tests.
//!
//! Today each directory is one project (named after it, with one
//! directory), the catalog arrives with the projects by recent use and
//! each project's chats newest first; the model keeps that order and
//! filters it. Pinned projects, standalone chats and the archive have no
//! data yet: they join [`visible_projects`] (its input and output) when
//! the store has them.

use super::ProjectChats;

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

    fn project(name: &str, titles: &[&str]) -> ProjectChats {
        ProjectChats {
            name: name.into(),
            root: format!("/w/{name}"),
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
    }

    #[test]
    fn a_project_shows_its_first_letter() {
        assert_eq!(initial("crest"), "C");
        assert_eq!(initial("élan"), "É");
        assert_eq!(initial(""), "?");
    }
}
