//! The changes the menus ask the binary to run in the store, and what the
//! window needs to know about the list to follow them.

use super::{Branch, ChatOp, ChatsPanel, ProjectOp};

/// A change to a chat or a project, as the binary runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Organize {
    /// A row of a chat's menu.
    Chat {
        /// Session id.
        id: String,
        /// What to do.
        op: ChatOp,
    },
    /// A row of a project's menu.
    Project {
        /// Project id.
        id: String,
        /// What to do.
        op: ProjectOp,
    },
}

/// What a change left to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Organized {
    /// Done; the list refreshes.
    Done,
    /// A branch was created: open it, in `root` (empty with no project).
    Branch {
        /// Session id of the branch.
        id: String,
        /// Where it opens.
        root: String,
    },
    /// A project was deleted; this many chats became standalone.
    Freed(usize),
}

/// What a change is called when it fails ("could not ...") and what to tell
/// the user when it is done; `None` for the changes the list already shows
/// (a pin, a name).
type Words = (&'static str, Option<&'static str>);

impl ChatOp {
    fn words(&self) -> Words {
        match self {
            Self::Pin(true) => ("pin the chat", None),
            Self::Pin(false) => ("unpin the chat", None),
            Self::Move(Some(_)) => ("move the chat", Some("chat moved")),
            Self::Move(None) => ("move the chat", Some("now a standalone chat")),
            Self::Promote(_) => ("create the project", Some("project created")),
            Self::Archive(true) => ("archive the chat", Some("chat archived")),
            Self::Archive(false) => ("restore the chat", Some("chat restored")),
            Self::Branch => ("branch the chat", None),
        }
    }
}

impl ProjectOp {
    fn words(&self) -> Words {
        match self {
            Self::Rename(_) => ("rename the project", None),
            Self::Pin(on) => (
                if *on {
                    "pin the project"
                } else {
                    "unpin the project"
                },
                None,
            ),
            Self::AddDirectory(_) => ("add the directory", None),
            Self::RemoveDirectory(_) => ("remove the directory", None),
            Self::Archive(true) => ("archive the project", Some("project archived")),
            Self::Archive(false) => ("restore the project", Some("project restored")),
            Self::Delete => ("delete the project", None),
        }
    }
}

impl Organize {
    fn words(&self) -> Words {
        match self {
            Self::Chat { op, .. } => op.words(),
            Self::Project { op, .. } => op.words(),
        }
    }

    /// What failed, for "could not ...".
    #[must_use]
    pub fn doing(&self) -> &'static str {
        self.words().0
    }

    /// What to tell the user once it is done, if anything.
    #[must_use]
    pub fn done(&self, outcome: &Organized) -> Option<String> {
        match outcome {
            Organized::Freed(0) => Some("project deleted".to_owned()),
            Organized::Freed(n) => {
                let verb = if *n == 1 { " is" } else { "s are" };
                Some(format!("project deleted; {n} chat{verb} standalone now"))
            }
            _ => self.words().1.map(str::to_owned),
        }
    }
}

impl ChatsPanel {
    /// Where the chat `id` was branched from, if it is a branch.
    #[must_use]
    pub fn branch_of(&self, id: &str) -> Option<Branch> {
        self.listed.chat(id)?.branch.clone()
    }

    /// The directory the chat `id` opens in, as listed (empty with no
    /// project), if it is listed.
    #[must_use]
    pub fn root_of(&self, id: &str) -> Option<String> {
        self.group_of(id).map(|(root, _)| root.to_owned())
    }

    /// Another chat of the group of `id`, as `(directory, id)`, to open when
    /// `id` goes away.
    #[must_use]
    pub fn fallback_for(&self, id: &str) -> Option<(String, String)> {
        let (root, chats) = self.group_of(id)?;
        super::next_chat_id(chats, id).map(|other| (root.to_owned(), other.to_owned()))
    }

    /// The first directory of the project `id`, if it is listed.
    #[must_use]
    pub fn project_root(&self, id: &str) -> Option<String> {
        self.listed
            .projects
            .iter()
            .find(|project| project.id == id)
            .map(|project| project.root.clone())
    }
}
