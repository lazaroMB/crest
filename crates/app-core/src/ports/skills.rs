//! Skills port: the skills and custom commands a project (and the user)
//! keeps for coding agents, in the layout Claude Code uses.
//!
//! A skill is a folder with a `SKILL.md` (`.claude/skills/<name>/`): YAML
//! frontmatter (`name`, `description`, `allowed-tools`...) and a Markdown
//! body with the instructions, next to any files they mention. A custom
//! command (`.claude/commands/<name>.md`) is a skill without a folder of
//! its own. The model loads one with the `skill` tool, which lists them by
//! name and description; the user runs one with `/<name> [args]`.

use std::path::PathBuf;
use std::sync::Arc;

/// Where a skill comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillScope {
    /// The user's own (`~/.claude/skills`, crest's config directory), for
    /// every project.
    User,
    /// The workspace's (`.claude/skills`, `.crest/skills`).
    Project,
    /// A subdirectory's `.claude/skills`, found when the agent worked
    /// there.
    Nested,
}

impl SkillScope {
    /// The word shown for the scope.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Nested => "nested",
        }
    }
}

/// What kind of file defines a skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillKind {
    /// A folder with a `SKILL.md`.
    Skill,
    /// A `commands/<name>.md` file.
    Command,
}

/// Where a skill runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkillContext {
    /// Its instructions join the conversation.
    #[default]
    Inline,
    /// A subagent follows them and only its report comes back
    /// (`context: fork`).
    Fork,
}

/// A skill as its frontmatter declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMeta {
    /// The name it is called by (`/name`, `skill(name)`).
    pub name: String,
    /// What it does, for the model to decide when to use it.
    pub description: String,
    /// When to use it, appended to the description (`when_to_use`).
    pub when_to_use: Option<String>,
    /// What the arguments are (`argument-hint`), shown in the palette.
    pub argument_hint: Option<String>,
    /// Whether the model may load it (`disable-model-invocation: false`).
    pub model_invocable: bool,
    /// Whether the user may run it as a command (`user-invocable`).
    pub user_invocable: bool,
    /// Tools it may use without asking while it runs, in Claude Code's
    /// notation (`Bash(git diff:*)`, `Read`).
    pub allowed_tools: Vec<String>,
    /// The model its turn (or subagent) uses: an alias, role or id.
    pub model: Option<String>,
    /// Where it runs.
    pub context: SkillContext,
    /// The subagent of a forked skill (`Explore`, `Plan`,
    /// `general-purpose`).
    pub agent: Option<String>,
    /// Where it comes from.
    pub scope: SkillScope,
    /// What kind of file defines it.
    pub kind: SkillKind,
    /// Its folder (a command's: the folder of its file).
    pub dir: PathBuf,
    /// The file shown for it: relative to the workspace, `~/…` or absolute.
    pub path: String,
}

impl SkillMeta {
    /// A skill named `name` described by `description`, with the defaults
    /// of a `SKILL.md` that only declares those two (tests and fakes).
    #[must_use]
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            path: format!(".claude/skills/{name}/SKILL.md"),
            dir: PathBuf::from(format!(".claude/skills/{name}")),
            name,
            description: description.into(),
            when_to_use: None,
            argument_hint: None,
            model_invocable: true,
            user_invocable: true,
            allowed_tools: Vec::new(),
            model: None,
            context: SkillContext::Inline,
            agent: None,
            scope: SkillScope::Project,
            kind: SkillKind::Skill,
        }
    }
}

/// A skill with its instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// What its frontmatter declares.
    pub meta: SkillMeta,
    /// Its body: the instructions, without the frontmatter.
    pub body: String,
}

/// Finds the skills of the user and the workspace.
///
/// Calls are synchronous: the files are small, and the adapter reads them
/// again only when they change.
pub trait SkillsPort: Send + Sync {
    /// Every skill, by name. When two share a name, the one that wins (see
    /// the adapter) is the only one listed.
    fn list(&self) -> Arc<[SkillMeta]>;

    /// The skill called `name`, with its body read now.
    fn load(&self, name: &str) -> Option<Skill>;

    /// Looks for `.claude/skills` in the directories between the workspace
    /// root and `path` (a file or directory the agent worked on), and adds
    /// what it finds. Does nothing by default.
    fn discover(&self, path: &str) {
        let _ = path;
    }

    /// A number that changes whenever [`Self::list`] does.
    fn revision(&self) -> u64;

    /// Problems found reading the skills, as "path: problem".
    fn warnings(&self) -> Vec<String> {
        Vec::new()
    }
}
