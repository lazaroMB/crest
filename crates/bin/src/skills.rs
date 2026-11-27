//! The skills and custom commands, read from disk: the adapter of
//! [`SkillsPort`].
//!
//! Where they are, in order of precedence (the first of a name wins, as in
//! Claude Code the user's own over the project's):
//!
//! 1. the user's: `~/.claude/skills/<name>/SKILL.md`, then crest's config
//!    directory (`~/.config/crest/skills/`), then the commands
//!    (`~/.claude/commands/*.md`, `~/.config/crest/commands/*.md`);
//! 2. the workspace's: `.claude/skills/`, `.crest/skills/`, then
//!    `.claude/commands/`, `.crest/commands/`;
//! 3. the `.claude/skills` of subdirectories, once a tool worked there
//!    ([`SkillsPort::discover`]).
//!
//! A command in a subfolder (`commands/frontend/test.md`) is still called
//! by its file name (`/test`). A name the built-in commands use is not
//! taken; neither is one that is not 1-64 letters, digits, `-` or `_`.
//! Every problem is a warning, and the rest still loads.
//!
//! The list is read again when a file or folder changes (by modification
//! time and size), and [`SkillsPort::revision`] moves with it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use application::ports::skills::{
    Skill, SkillContext, SkillKind, SkillMeta, SkillScope, SkillsPort,
};
use application::usecases::commands::Command;
use serde::Deserialize;

use crate::frontmatter;
use crate::instructions::{modified, slashed};

/// The file of a skill, in its folder.
pub const SKILL_FILE: &str = "SKILL.md";
/// Bytes read of a skill's file.
const MAX_SKILL_BYTES: u64 = 256 * 1024;
/// Characters of a description taken from the body.
const MAX_DESCRIPTION_CHARS: usize = 1_024;

/// The skills of the user and of a workspace.
#[derive(Debug)]
pub struct FileSkills {
    /// The workspace root, canonical; `None` for a chat without a directory.
    root: Option<PathBuf>,
    home: Option<PathBuf>,
    config: Option<PathBuf>,
    /// The `.claude/skills` of subdirectories found so far.
    nested: Mutex<BTreeSet<PathBuf>>,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    loaded: Option<Loaded>,
    revision: u64,
}

#[derive(Debug, Clone)]
struct Loaded {
    stamps: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
    /// The nested folders it was read with.
    nested: BTreeSet<PathBuf>,
    list: Arc<[SkillMeta]>,
    warnings: Vec<String>,
}

/// Where skills of one kind are found.
struct Source {
    dir: PathBuf,
    scope: SkillScope,
    kind: SkillKind,
}

/// A skill's frontmatter, with Claude Code's keys.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Front {
    name: Option<String>,
    description: Option<String>,
    #[serde(alias = "when_to_use")]
    when_to_use: Option<String>,
    argument_hint: Option<String>,
    #[serde(default, deserialize_with = "frontmatter::flag")]
    disable_model_invocation: Option<bool>,
    #[serde(default, deserialize_with = "frontmatter::flag")]
    user_invocable: Option<bool>,
    #[serde(default, deserialize_with = "frontmatter::list")]
    allowed_tools: Vec<String>,
    model: Option<String>,
    context: Option<String>,
    agent: Option<String>,
    hooks: Option<serde_norway::Value>,
}

impl FileSkills {
    /// The skills of the user (from `home` and crest's `config` directory)
    /// and of the workspace at `root`, if any.
    #[must_use]
    pub fn new(root: Option<&Path>, home: Option<PathBuf>, config: Option<PathBuf>) -> Self {
        let root = root.map(|r| r.canonicalize().unwrap_or_else(|_| r.to_path_buf()));
        Self {
            root,
            home,
            config,
            nested: Mutex::default(),
            state: Mutex::default(),
        }
    }

    /// The skills of the user's home and config directories and of `root`.
    #[must_use]
    pub fn for_user(root: Option<&Path>) -> Self {
        Self::new(
            root,
            platform::dirs::home_dir(),
            platform::dirs::config_dir(),
        )
    }

    /// The folders to look in, by precedence.
    fn sources(&self, nested: &BTreeSet<PathBuf>) -> Vec<Source> {
        let mut out = Vec::new();
        let mut level = |bases: Vec<PathBuf>, scope: SkillScope| {
            for (sub, kind) in [
                ("skills", SkillKind::Skill),
                ("commands", SkillKind::Command),
            ] {
                for base in &bases {
                    out.push(Source {
                        dir: base.join(sub),
                        scope,
                        kind,
                    });
                }
            }
        };
        let mut user = Vec::new();
        if let Some(home) = &self.home {
            user.push(home.join(".claude"));
        }
        if let Some(config) = &self.config {
            user.push(config.clone());
        }
        level(user, SkillScope::User);
        if let Some(root) = &self.root {
            level(
                vec![root.join(".claude"), root.join(".crest")],
                SkillScope::Project,
            );
        }
        out.extend(nested.iter().map(|dir| Source {
            dir: dir.clone(),
            scope: SkillScope::Nested,
            kind: SkillKind::Skill,
        }));
        out
    }

    /// The list, read again when something changed.
    fn current(&self) -> (Loaded, u64) {
        let nested = self
            .nested
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(loaded) = &state.loaded
            && loaded.nested == nested
            && loaded
                .stamps
                .iter()
                .all(|(path, stamp)| modified(path) == *stamp)
        {
            return (loaded.clone(), state.revision);
        }
        let loaded = self.read_all(nested);
        let changed = state
            .loaded
            .as_ref()
            .is_none_or(|old| old.list != loaded.list);
        if changed {
            state.revision += 1;
        }
        state.loaded = Some(loaded.clone());
        (loaded, state.revision)
    }

    fn read_all(&self, nested: BTreeSet<PathBuf>) -> Loaded {
        let mut stamps = Vec::new();
        let mut warnings = Vec::new();
        let mut by_name: BTreeMap<String, SkillMeta> = BTreeMap::new();
        for source in self.sources(&nested) {
            let files = match source.kind {
                SkillKind::Skill => skill_files(&source.dir, &mut stamps),
                SkillKind::Command => {
                    let mut out = Vec::new();
                    command_files(&source.dir, &mut stamps, &mut BTreeSet::new(), &mut out);
                    out
                }
            };
            for file in files {
                let Some(meta) = self.meta(&file, &source, &mut warnings) else {
                    continue;
                };
                match by_name.get(&meta.name) {
                    Some(winner) => warnings.push(format!(
                        "{}: the skill `{}` of {} is used instead",
                        meta.path, meta.name, winner.path
                    )),
                    None => {
                        by_name.insert(meta.name.clone(), meta);
                    }
                }
            }
        }
        Loaded {
            stamps,
            nested,
            list: by_name.into_values().collect(),
            warnings,
        }
    }

    /// What the file at `path` declares, or `None` with a warning.
    fn meta(&self, path: &Path, source: &Source, warnings: &mut Vec<String>) -> Option<SkillMeta> {
        let shown = self.display(path);
        if source.scope != SkillScope::User && !self.inside_root(path) {
            warnings.push(format!("{shown}: points outside the workspace; not read"));
            return None;
        }
        let text = match read(path) {
            Ok(text) => text,
            Err(e) => {
                warnings.push(format!("{shown}: {e}"));
                return None;
            }
        };
        let (front, body) = frontmatter::parse::<Front>(&text);
        let front = match front {
            Ok(front) => front,
            Err(e) => {
                warnings.push(format!(
                    "{shown}: the frontmatter is not valid YAML ({e}); not loaded"
                ));
                return None;
            }
        };
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let fallback = match source.kind {
            SkillKind::Skill => dir.file_name(),
            SkillKind::Command => path.file_stem(),
        }
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
        let name = front
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&fallback)
            .to_owned();
        if !valid_name(&name) {
            warnings.push(format!(
                "{shown}: `{name}` is not a valid name (1-64 letters, digits, - or _); not loaded"
            ));
            return None;
        }
        if !matches!(Command::parse(&name), Command::Other { .. }) {
            warnings.push(format!(
                "{shown}: /{name} is a built-in command; rename the skill to use it"
            ));
            return None;
        }
        if front.hooks.is_some() {
            warnings.push(format!(
                "{shown}: `hooks` in a skill are not supported; ignored"
            ));
        }
        let description = front
            .description
            .map(|d| d.trim().to_owned())
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| first_paragraph(body));
        let context = match front.context.as_deref().map(str::trim) {
            Some("fork") => SkillContext::Fork,
            None | Some("" | "inline") => SkillContext::Inline,
            Some(other) => {
                warnings.push(format!(
                    "{shown}: context `{other}` is not known (fork); it runs inline"
                ));
                SkillContext::Inline
            }
        };
        let model = front
            .model
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty() && m != "inherit");
        Some(SkillMeta {
            name,
            description,
            when_to_use: front.when_to_use.filter(|w| !w.trim().is_empty()),
            argument_hint: front.argument_hint.filter(|h| !h.trim().is_empty()),
            model_invocable: !front.disable_model_invocation.unwrap_or(false),
            user_invocable: front.user_invocable.unwrap_or(true),
            allowed_tools: front.allowed_tools,
            model,
            context,
            agent: front.agent.filter(|a| !a.trim().is_empty()),
            scope: source.scope,
            kind: source.kind,
            dir: dir.canonicalize().unwrap_or(dir),
            path: shown,
        })
    }

    /// The path shown for `path`: relative to the root, `~/…` under the
    /// home, else as it is.
    fn display(&self, path: &Path) -> String {
        if let Some(rel) = self.root.as_deref().and_then(|r| path.strip_prefix(r).ok()) {
            return slashed(rel);
        }
        if let Some(rel) = self.home.as_deref().and_then(|h| path.strip_prefix(h).ok()) {
            return format!("~/{}", slashed(rel));
        }
        path.display().to_string()
    }

    /// Whether `path` (followed through symlinks) is inside the workspace.
    fn inside_root(&self, path: &Path) -> bool {
        let Some(root) = self.root.as_deref() else {
            return false;
        };
        path.canonicalize()
            .is_ok_and(|canonical| canonical.starts_with(root))
    }

    /// The path of the skill `meta` describes, from its shown path.
    fn file_of(&self, meta: &SkillMeta) -> PathBuf {
        match meta.kind {
            SkillKind::Skill => meta.dir.join(SKILL_FILE),
            SkillKind::Command => {
                let name = Path::new(&meta.path)
                    .file_name()
                    .map(ToOwned::to_owned)
                    .unwrap_or_default();
                meta.dir.join(name)
            }
        }
    }
}

impl SkillsPort for FileSkills {
    fn list(&self) -> Arc<[SkillMeta]> {
        self.current().0.list
    }

    fn load(&self, name: &str) -> Option<Skill> {
        let meta = self.list().iter().find(|meta| meta.name == name).cloned()?;
        let text = read(&self.file_of(&meta)).ok()?;
        let (_, body) = frontmatter::parse::<Front>(&text);
        Some(Skill {
            body: body.trim().to_owned(),
            meta,
        })
    }

    fn discover(&self, path: &str) {
        let Some(root) = &self.root else {
            return;
        };
        let rel = Path::new(path.trim());
        let rel = if rel.is_absolute() {
            match rel.strip_prefix(root) {
                Ok(rel) => rel,
                Err(_) => return,
            }
        } else {
            rel
        };
        if rel.components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        }) {
            return;
        }
        let mut at = root.clone();
        let mut found = Vec::new();
        let parts: Vec<_> = rel.components().collect();
        // The path's own folders, not the root's (already a source).
        for (index, part) in parts.iter().enumerate() {
            at.push(part);
            if index + 1 == parts.len() && !at.is_dir() {
                break;
            }
            let skills = at.join(".claude").join("skills");
            if skills.is_dir() {
                found.push(skills);
            }
        }
        if found.is_empty() {
            return;
        }
        self.nested
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(found);
    }

    fn revision(&self) -> u64 {
        self.current().1
    }

    fn warnings(&self) -> Vec<String> {
        self.current().0.warnings
    }
}

/// The `SKILL.md` of each folder in `dir`, sorted; the folders are
/// stamped, and each file.
fn skill_files(dir: &Path, stamps: &mut Vec<(PathBuf, Option<(SystemTime, u64)>)>) -> Vec<PathBuf> {
    stamps.push((dir.to_path_buf(), modified(dir)));
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut folders: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();
    let mut out = Vec::new();
    for folder in folders {
        stamps.push((folder.clone(), modified(&folder)));
        let file = folder.join(SKILL_FILE);
        stamps.push((file.clone(), modified(&file)));
        if file.is_file() {
            out.push(file);
        }
    }
    out
}

/// The `.md` files under `dir`, sorted, subfolders included (each folder
/// once, following symlinks).
fn command_files(
    dir: &Path,
    stamps: &mut Vec<(PathBuf, Option<(SystemTime, u64)>)>,
    visited: &mut BTreeSet<PathBuf>,
    out: &mut Vec<PathBuf>,
) {
    stamps.push((dir.to_path_buf(), modified(dir)));
    let Ok(canonical) = dir.canonicalize() else {
        return;
    };
    if !visited.insert(canonical) {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            command_files(&path, stamps, visited, out);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            stamps.push((path.clone(), modified(&path)));
            out.push(path);
        }
    }
}

/// The text of `path`, refused above [`MAX_SKILL_BYTES`].
fn read(path: &Path) -> Result<String, String> {
    let size = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_SKILL_BYTES {
        return Err(format!(
            "longer than {} KB; not loaded",
            MAX_SKILL_BYTES / 1024
        ));
    }
    fs::read_to_string(path).map_err(|e| e.to_string())
}

/// Whether `name` can be typed as `/name` and named in a tool call.
fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The first paragraph of `body` that is not a heading, on one line.
fn first_paragraph(body: &str) -> String {
    let mut lines = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !lines.is_empty() {
                break;
            }
            continue;
        }
        if lines.is_empty() && line.starts_with('#') {
            continue;
        }
        lines.push(line);
    }
    let text = lines.join(" ");
    match text.char_indices().nth(MAX_DESCRIPTION_CHARS) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        config: PathBuf,
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let base = tmp.path().canonicalize().unwrap();
            let (home, config, root) = (base.join("home"), base.join("config"), base.join("ws"));
            for dir in [&home, &config, &root] {
                fs::create_dir_all(dir).unwrap();
            }
            Self {
                _tmp: tmp,
                home,
                config,
                root,
            }
        }

        fn write(&self, path: &Path, text: &str) {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn ws(&self, rel: &str, text: &str) {
            self.write(&self.root.join(rel), text);
        }

        fn port(&self) -> FileSkills {
            FileSkills::new(
                Some(&self.root),
                Some(self.home.clone()),
                Some(self.config.clone()),
            )
        }
    }

    fn names(port: &FileSkills) -> Vec<String> {
        port.list().iter().map(|s| s.name.clone()).collect()
    }

    #[test]
    fn reads_skills_and_commands_with_their_frontmatter() {
        let fx = Fixture::new();
        fx.ws(
            ".claude/skills/review/SKILL.md",
            "---\nname: review\ndescription: Reviews the diff\nargument-hint: \"[file]\"\n\
             allowed-tools: Bash(git diff:*), Read\nmodel: haiku\ncontext: fork\nagent: Explore\n\
             disable-model-invocation: true\n---\nReview $ARGUMENTS.\n",
        );
        fx.ws(
            ".claude/commands/frontend/greet.md",
            "# Greet\n\nSays hello\nto them.\n\nBody",
        );
        fx.ws(".crest/skills/plain/SKILL.md", "Just text.");
        let port = fx.port();
        assert_eq!(names(&port), ["greet", "plain", "review"]);
        let review = port.load("review").unwrap();
        assert_eq!(review.body, "Review $ARGUMENTS.");
        let meta = review.meta;
        assert_eq!(meta.description, "Reviews the diff");
        assert_eq!(meta.argument_hint.as_deref(), Some("[file]"));
        assert_eq!(meta.allowed_tools, ["Bash(git diff:*)", "Read"]);
        assert_eq!(meta.model.as_deref(), Some("haiku"));
        assert_eq!(meta.context, SkillContext::Fork);
        assert_eq!(meta.agent.as_deref(), Some("Explore"));
        assert!(!meta.model_invocable);
        assert!(meta.user_invocable);
        assert_eq!(meta.scope, SkillScope::Project);
        assert_eq!(meta.path, ".claude/skills/review/SKILL.md");
        assert_eq!(meta.dir, fx.root.join(".claude/skills/review"));
        let greet = port.load("greet").unwrap();
        assert_eq!(greet.meta.kind, SkillKind::Command);
        assert_eq!(greet.meta.description, "Says hello to them.");
        assert_eq!(greet.meta.path, ".claude/commands/frontend/greet.md");
        assert!(port.load("nope").is_none());
    }

    #[test]
    fn the_users_skill_wins_and_a_skill_beats_a_command() {
        let fx = Fixture::new();
        fx.write(
            &fx.home.join(".claude/skills/deploy/SKILL.md"),
            "---\ndescription: mine\n---\nx",
        );
        fx.ws(
            ".claude/skills/deploy/SKILL.md",
            "---\ndescription: repo\n---\ny",
        );
        fx.ws(
            ".claude/commands/lint.md",
            "---\ndescription: command\n---\nz",
        );
        fx.ws(
            ".claude/skills/lint/SKILL.md",
            "---\ndescription: skill\n---\nw",
        );
        fx.write(&fx.config.join("skills/notes/SKILL.md"), "notes");
        let port = fx.port();
        let list = port.list();
        let deploy = list.iter().find(|s| s.name == "deploy").unwrap();
        assert_eq!(deploy.description, "mine");
        assert_eq!(deploy.scope, SkillScope::User);
        assert_eq!(deploy.path, "~/.claude/skills/deploy/SKILL.md");
        let lint = list.iter().find(|s| s.name == "lint").unwrap();
        assert_eq!(lint.description, "skill");
        assert!(list.iter().any(|s| s.name == "notes"));
        let warnings = port.warnings();
        assert!(
            warnings
                .iter()
                .any(|w| w.starts_with(".claude/skills/deploy/SKILL.md"))
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.starts_with(".claude/commands/lint.md"))
        );
    }

    #[test]
    fn builtin_and_invalid_names_are_refused() {
        let fx = Fixture::new();
        fx.ws(".claude/commands/help.md", "x");
        fx.ws(".claude/skills/bad name/SKILL.md", "x");
        fx.ws(".claude/skills/broken/SKILL.md", "---\nname: [oops\n---\nx");
        fx.ws(
            ".claude/skills/hooked/SKILL.md",
            "---\nhooks:\n  PreToolUse: []\n---\nx",
        );
        let port = fx.port();
        assert_eq!(names(&port), ["hooked"]);
        let warnings = port.warnings().join("\n");
        assert!(
            warnings.contains("/help is a built-in command"),
            "{warnings}"
        );
        assert!(warnings.contains("not a valid name"), "{warnings}");
        assert!(warnings.contains("not valid YAML"), "{warnings}");
        assert!(warnings.contains("`hooks`"), "{warnings}");
    }

    #[test]
    fn changes_are_read_again_and_move_the_revision() {
        let fx = Fixture::new();
        let port = fx.port();
        assert!(port.list().is_empty());
        let first = port.revision();
        assert_eq!(port.revision(), first, "nothing changed");
        fx.ws(
            ".claude/skills/new/SKILL.md",
            "---\ndescription: one\n---\nx",
        );
        assert_eq!(names(&port), ["new"]);
        let second = port.revision();
        assert!(second > first);
        fx.ws(
            ".claude/skills/new/SKILL.md",
            "---\ndescription: two, longer\n---\nx",
        );
        assert_eq!(port.list()[0].description, "two, longer");
        assert!(port.revision() > second);
    }

    #[test]
    fn subdirectories_add_their_skills_once_a_tool_works_there() {
        let fx = Fixture::new();
        fx.ws("packages/web/.claude/skills/storybook/SKILL.md", "Stories");
        fx.ws("packages/web/src/app.ts", "");
        let port = fx.port();
        assert!(port.list().is_empty());
        let before = port.revision();
        port.discover("packages/web/src/app.ts");
        let list = port.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].scope, SkillScope::Nested);
        assert_eq!(
            list[0].path,
            "packages/web/.claude/skills/storybook/SKILL.md"
        );
        assert!(port.revision() > before);
        port.discover("../elsewhere/x");
        port.discover("/etc/passwd");
        assert_eq!(port.list().len(), 1);
    }

    #[test]
    fn a_chat_without_a_directory_reads_only_the_users() {
        let fx = Fixture::new();
        fx.ws(".claude/skills/repo/SKILL.md", "x");
        fx.write(&fx.home.join(".claude/commands/mine.md"), "y");
        let port = FileSkills::new(None, Some(fx.home.clone()), Some(fx.config.clone()));
        assert_eq!(names(&port), ["mine"]);
    }
}
