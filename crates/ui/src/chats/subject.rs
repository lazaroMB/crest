//! What a menu is about, and how the list finds it.

use super::Catalog;

/// What a menu is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Subject {
    /// A chat.
    Chat {
        id: String,
        /// The directory it opens in; empty with no project.
        root: String,
        title: String,
        /// Its project, if any.
        project: Option<String>,
        pinned: bool,
    },
    /// A project.
    Project {
        id: String,
        name: String,
        roots: Vec<String>,
        pinned: bool,
        chats: usize,
    },
}

impl Subject {
    pub(super) fn id(&self) -> &str {
        match self {
            Self::Chat { id, .. } | Self::Project { id, .. } => id,
        }
    }

    /// The title of the chat or the name of the project.
    pub(super) fn title(&self) -> &str {
        match self {
            Self::Chat { title, .. } => title,
            Self::Project { name, .. } => name,
        }
    }

    /// Whether this menu is about the chat or project `id` (`project`
    /// tells which).
    pub(super) fn is(&self, project: bool, id: &str) -> bool {
        matches!(self, Self::Project { .. }) == project && self.id() == id
    }
}

impl Catalog {
    /// The chat `id` as listed, with the project it is under.
    pub(super) fn chat_subject(&self, id: &str) -> Option<Subject> {
        let group = self
            .projects
            .iter()
            .find(|project| project.chats.iter().any(|chat| chat.id == id));
        let chat = self.chat(id)?;
        Some(Subject::Chat {
            id: id.to_owned(),
            root: group
                .map(|project| project.root.clone())
                .unwrap_or_default(),
            title: chat.title.clone(),
            project: group.map(|project| project.id.clone()),
            pinned: chat.pinned,
        })
    }

    /// The project `id` as listed.
    pub(super) fn project_subject(&self, id: &str) -> Option<Subject> {
        let project = self.projects.iter().find(|project| project.id == id)?;
        Some(Subject::Project {
            id: id.to_owned(),
            name: project.name.clone(),
            roots: project.roots.clone(),
            pinned: project.pinned,
            chats: project.chats.len(),
        })
    }

    /// An archived chat or project `id`: only its delete question uses the
    /// subject.
    pub(super) fn archived_subject(&self, project: bool, id: &str) -> Option<Subject> {
        let archive = &self.archived;
        if project {
            let found = archive.projects.iter().find(|project| project.id == id)?;
            return Some(Subject::Project {
                id: id.to_owned(),
                name: found.name.clone(),
                roots: Vec::new(),
                pinned: false,
                chats: found.chats,
            });
        }
        let found = archive.chats.iter().find(|chat| chat.id == id)?;
        Some(Subject::Chat {
            id: id.to_owned(),
            root: String::new(),
            title: found.title.clone(),
            project: None,
            pinned: false,
        })
    }

    /// The projects a chat can move to: the ones listed, which never
    /// include an archived project (id, name).
    pub(super) fn move_targets(&self) -> Vec<(String, String)> {
        self.projects
            .iter()
            .map(|project| (project.id.clone(), project.name.clone()))
            .collect()
    }
}
