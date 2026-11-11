//! The `AGENTS.md` and `CLAUDE.md` files, read from disk: the adapter of
//! [`InstructionsPort`].
//!
//! The base files, in order (general first, so the later ones win):
//! `~/.claude/CLAUDE.md`, `~/.config/crest/AGENTS.md`, then the workspace
//! root's `AGENTS.md`, `CLAUDE.md` and `CLAUDE.local.md`. The same three
//! names in a subdirectory apply to its subtree and are read when a tool
//! works there ([`InstructionsPort::nested`]).
//!
//! A file imports another with `@path` (relative to the file, absolute or
//! `~/…`), outside code, up to [`MAX_IMPORT_DEPTH`] levels; the imported
//! text follows the file's. A project file only imports files inside the
//! workspace, so a cloned repository cannot send `@~/.ssh/id_rsa` to the
//! model; the user's own files import anything. A file is read once: a
//! symlink to one already read, a copy of its text, or a `CLAUDE.md` that
//! only imports the `AGENTS.md` beside it adds nothing.

use std::collections::BTreeSet;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use application::ports::instructions::{InstructionFile, InstructionScope, InstructionsPort};

/// The names of an instructions file, in the order they are read.
pub const NAMES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", "CLAUDE.local.md"];
/// Bytes kept of one file; the rest is dropped with a warning.
pub const MAX_FILE_BYTES: usize = 40 * 1024;
/// Levels of `@path` imports followed.
pub const MAX_IMPORT_DEPTH: usize = 5;

/// The instructions files of the user and of a workspace.
#[derive(Debug)]
pub struct FileInstructions {
    /// The workspace root, canonical; `None` for a chat without a directory.
    root: Option<PathBuf>,
    home: Option<PathBuf>,
    /// crest's config directory (`~/.config/crest`).
    config: Option<PathBuf>,
    cache: Mutex<Option<Loaded>>,
}

/// The base files as last read.
#[derive(Debug, Clone)]
struct Loaded {
    /// Modification time and size of every file read or looked for.
    stamps: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
    files: Vec<InstructionFile>,
    /// Canonical paths of the files read, imports included.
    read: BTreeSet<PathBuf>,
    text: Arc<str>,
    warnings: Vec<String>,
}

impl FileInstructions {
    /// The files of the user (from `home` and crest's `config` directory)
    /// and of the workspace at `root`, if any.
    #[must_use]
    pub fn new(root: Option<&Path>, home: Option<PathBuf>, config: Option<PathBuf>) -> Self {
        let root = root.map(|r| r.canonicalize().unwrap_or_else(|_| r.to_path_buf()));
        Self {
            root,
            home,
            config,
            cache: Mutex::new(None),
        }
    }

    /// The files of the user's home and config directories and of `root`.
    #[must_use]
    pub fn for_user(root: Option<&Path>) -> Self {
        Self::new(
            root,
            platform::dirs::home_dir(),
            platform::dirs::config_dir(),
        )
    }

    /// The base files to look for, with their scope.
    fn candidates(&self) -> Vec<(PathBuf, InstructionScope)> {
        let mut out = Vec::new();
        if let Some(home) = &self.home {
            out.push((
                home.join(".claude").join("CLAUDE.md"),
                InstructionScope::User,
            ));
        }
        if let Some(config) = &self.config {
            out.push((config.join("AGENTS.md"), InstructionScope::User));
        }
        if let Some(root) = &self.root {
            out.extend(
                NAMES
                    .iter()
                    .map(|name| (root.join(name), InstructionScope::Project)),
            );
        }
        out
    }

    /// The base files, read again when one of them (or of their imports)
    /// changed since the last time.
    fn current(&self) -> Loaded {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(loaded) = cache.as_ref()
            && loaded
                .stamps
                .iter()
                .all(|(path, stamp)| modified(path) == *stamp)
        {
            return loaded.clone();
        }
        let loaded = self.load_base();
        *cache = Some(loaded.clone());
        loaded
    }

    fn load_base(&self) -> Loaded {
        let mut reader = Reader::new(self);
        let mut files = Vec::new();
        for (path, scope) in self.candidates() {
            reader.stamp(&path);
            if let Some(file) = reader.file(&path, scope) {
                files.push(file);
            }
        }
        let text = render(&files).into();
        Loaded {
            stamps: reader.stamps,
            files,
            read: reader.seen,
            text,
            warnings: reader.warnings,
        }
    }

    /// The path shown for `path`: relative to the root, `~/…` under the
    /// home, else as it is.
    fn display(&self, path: &Path) -> String {
        if let Some(rel) = self
            .root
            .as_deref()
            .and_then(|root| path.strip_prefix(root).ok())
        {
            return slashed(rel);
        }
        if let Some(rel) = self
            .home
            .as_deref()
            .and_then(|home| path.strip_prefix(home).ok())
        {
            return format!("~/{}", slashed(rel));
        }
        path.display().to_string()
    }

    /// Whether `path` (canonical) is inside the workspace.
    fn inside_root(&self, path: &Path) -> bool {
        self.root
            .as_deref()
            .is_some_and(|root| path.starts_with(root))
    }
}

impl InstructionsPort for FileInstructions {
    fn base(&self) -> Arc<str> {
        self.current().text
    }

    fn nested(&self, path: &str) -> Vec<InstructionFile> {
        let Some(root) = &self.root else {
            return Vec::new();
        };
        let Some(rel) = relative(path) else {
            return Vec::new();
        };
        let target = root.join(&rel);
        // A file's directory, or the directory itself.
        let dir = if target.is_dir() {
            rel
        } else {
            rel.parent().map(Path::to_path_buf).unwrap_or_default()
        };
        let mut reader = Reader::new(self);
        // The base files and their imports are already in the prompt.
        reader.seen = self.current().read;
        let mut out = Vec::new();
        let mut at = root.clone();
        for part in dir.components() {
            at.push(part);
            for name in NAMES {
                if let Some(file) = reader.file(&at.join(name), InstructionScope::Nested) {
                    out.push(file);
                }
            }
        }
        out
    }

    fn loaded(&self) -> Vec<InstructionFile> {
        self.current().files
    }

    fn warnings(&self) -> Vec<String> {
        self.current().warnings
    }
}

/// `path` relative to the workspace root, if it stays inside it.
fn relative(path: &str) -> Option<PathBuf> {
    let path = Path::new(path.trim());
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(name) => out.push(name),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(out)
}

/// `path` with `/` between its parts on every platform.
fn slashed(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// When `path` changed and its size: a write within one tick of a coarse
/// clock still changes the size, most of the time.
fn modified(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Reads files, following imports and dropping what was already read.
struct Reader<'a> {
    owner: &'a FileInstructions,
    /// Canonical paths read (or not to read).
    seen: BTreeSet<PathBuf>,
    /// Texts read, trimmed.
    texts: BTreeSet<String>,
    stamps: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
    warnings: Vec<String>,
}

impl<'a> Reader<'a> {
    fn new(owner: &'a FileInstructions) -> Self {
        Self {
            owner,
            seen: BTreeSet::new(),
            texts: BTreeSet::new(),
            stamps: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn stamp(&mut self, path: &Path) {
        self.stamps.push((path.to_path_buf(), modified(path)));
    }

    /// The file at `path` with its imports, or `None` when it is missing,
    /// already read or adds nothing.
    fn file(&mut self, path: &Path, scope: InstructionScope) -> Option<InstructionFile> {
        let canonical = path.canonicalize().ok()?;
        if !canonical.is_file() || !self.seen.insert(canonical.clone()) {
            return None;
        }
        if scope != InstructionScope::User && !self.owner.inside_root(&canonical) {
            self.warnings.push(format!(
                "{}: points outside the workspace; not read",
                path.display()
            ));
            return None;
        }
        let own = self.read(&canonical)?;
        let mut imports = Vec::new();
        let mut imported = String::new();
        self.imports(&canonical, &own, scope, 1, &mut imports, &mut imported);
        let body = strip_imports(&own);
        let body = body.trim();
        if body.is_empty() && imported.trim().is_empty() {
            return None;
        }
        if !self.texts.insert(own.trim().to_owned()) {
            return None;
        }
        let mut text = body.to_owned();
        if !imported.is_empty() {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(imported.trim_start());
        }
        Some(InstructionFile {
            path: self.owner.display(path),
            scope,
            text,
            imports,
        })
    }

    /// Appends to `out` the files `text` (of the file at `from`) imports,
    /// and theirs.
    fn imports(
        &mut self,
        from: &Path,
        text: &str,
        scope: InstructionScope,
        depth: usize,
        names: &mut Vec<String>,
        out: &mut String,
    ) {
        if depth > MAX_IMPORT_DEPTH {
            return;
        }
        let dir = from.parent().unwrap_or(Path::new(""));
        for target in import_targets(text) {
            let Some(path) = self.resolve(dir, &target) else {
                continue;
            };
            // `@scope/pkg` and other words that are not files are text.
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };
            if !canonical.is_file() {
                continue;
            }
            self.stamp(&path);
            if scope != InstructionScope::User && !self.owner.inside_root(&canonical) {
                self.warnings.push(format!(
                    "{}: the import @{target} is outside the workspace; not read",
                    from.display()
                ));
                continue;
            }
            if !self.seen.insert(canonical.clone()) {
                continue;
            }
            let Some(imported) = self.read(&canonical) else {
                continue;
            };
            let shown = self.owner.display(&canonical);
            names.push(shown.clone());
            if self.texts.insert(imported.trim().to_owned()) {
                let body = strip_imports(&imported);
                if !body.trim().is_empty() {
                    out.push_str(&format!("\n\n(imported from {shown})\n{}", body.trim()));
                }
            }
            self.imports(&canonical, &imported, scope, depth + 1, names, out);
        }
    }

    fn resolve(&self, dir: &Path, target: &str) -> Option<PathBuf> {
        if let Some(rest) = target.strip_prefix("~/") {
            return self.owner.home.as_ref().map(|home| home.join(rest));
        }
        let path = Path::new(target);
        Some(if path.is_absolute() {
            path.to_path_buf()
        } else {
            dir.join(path)
        })
    }

    /// The text of `path`, cut to [`MAX_FILE_BYTES`].
    fn read(&mut self, path: &Path) -> Option<String> {
        match std::fs::read_to_string(path) {
            Ok(mut text) => {
                if text.len() > MAX_FILE_BYTES {
                    let mut end = MAX_FILE_BYTES;
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text.truncate(end);
                    self.warnings.push(format!(
                        "{}: longer than {} KB; the rest is not read",
                        path.display(),
                        MAX_FILE_BYTES / 1024
                    ));
                }
                Some(text)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                self.warnings.push(format!("{}: {e}", path.display()));
                None
            }
        }
    }
}

/// The `@path` imports of `text`: words starting with `@` after a space or
/// at a line start, outside fenced blocks and code spans.
fn import_targets(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for_prose(text, |prose| {
        for word in prose.split_whitespace() {
            if let Some(target) = import_word(word) {
                out.push(target.to_owned());
            }
        }
    });
    out
}

/// The path of an `@path` word, without trailing punctuation.
fn import_word(word: &str) -> Option<&str> {
    let target = word
        .strip_prefix('@')?
        .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '"', '\'']);
    (!target.is_empty()).then_some(target)
}

/// `text` without its import words (they are replaced by what they import).
fn strip_imports(text: &str) -> String {
    let mut fence = false;
    let mut out = Vec::new();
    for line in text.lines() {
        if is_fence(line) {
            fence = !fence;
            out.push(line.to_owned());
            continue;
        }
        if fence {
            out.push(line.to_owned());
            continue;
        }
        let kept = line
            .split('`')
            .enumerate()
            .map(|(i, part)| {
                if i % 2 == 1 {
                    return part.to_owned();
                }
                part.split(' ')
                    .filter(|word| import_word(word).is_none())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join("`");
        if kept.trim().is_empty() && !line.trim().is_empty() {
            continue;
        }
        out.push(kept.trim_end().to_owned());
    }
    out.join("\n")
}

/// Calls `f` with each piece of `text` outside fenced blocks and code
/// spans.
fn for_prose(text: &str, mut f: impl FnMut(&str)) {
    let mut fence = false;
    for line in text.lines() {
        if is_fence(line) {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        for (i, part) in line.split('`').enumerate() {
            if i % 2 == 0 {
                f(part);
            }
        }
    }
}

fn is_fence(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("```") || line.starts_with("~~~")
}

/// The system prompt section for `files`, or an empty text.
#[must_use]
pub fn render(files: &[InstructionFile]) -> String {
    if files.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "# Project instructions\n\
         The user keeps these instructions for coding agents in the files below (the \
         user's own first, then the project's). Follow them: they take precedence over the \
         defaults above, and a more specific file wins over a more general one. Files in \
         subdirectories are shown when you work in them.",
    );
    for file in files {
        out.push_str("\n\n## ");
        out.push_str(&file.path);
        out.push('\n');
        out.push_str(file.text.trim());
    }
    out
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
            let home = base.join("home");
            let config = base.join("config");
            let root = base.join("ws");
            for dir in [&home, &config, &root] {
                std::fs::create_dir_all(dir).unwrap();
            }
            Self {
                _tmp: tmp,
                home,
                config,
                root,
            }
        }

        fn write(&self, path: &Path, text: &str) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn ws(&self, rel: &str, text: &str) {
            self.write(&self.root.join(rel), text);
        }

        fn port(&self) -> FileInstructions {
            FileInstructions::new(
                Some(&self.root),
                Some(self.home.clone()),
                Some(self.config.clone()),
            )
        }
    }

    fn paths(files: &[InstructionFile]) -> Vec<&str> {
        files.iter().map(|f| f.path.as_str()).collect()
    }

    #[test]
    fn the_base_files_load_from_general_to_specific() {
        let fx = Fixture::new();
        fx.write(&fx.home.join(".claude/CLAUDE.md"), "home rule");
        fx.write(&fx.config.join("AGENTS.md"), "crest rule");
        fx.ws("AGENTS.md", "agents rule");
        fx.ws("CLAUDE.md", "claude rule");
        fx.ws("CLAUDE.local.md", "local rule");
        let port = fx.port();
        let files = port.loaded();
        let config = fx.config.join("AGENTS.md").display().to_string();
        assert_eq!(
            paths(&files),
            [
                "~/.claude/CLAUDE.md",
                config.as_str(),
                "AGENTS.md",
                "CLAUDE.md",
                "CLAUDE.local.md"
            ]
        );
        assert_eq!(files[0].scope, InstructionScope::User);
        assert_eq!(files[2].scope, InstructionScope::Project);
        let base = port.base();
        assert!(base.starts_with("# Project instructions"), "{base}");
        let order: Vec<_> = [
            "home rule",
            "crest rule",
            "agents rule",
            "claude rule",
            "local rule",
        ]
        .iter()
        .map(|rule| base.find(rule).unwrap())
        .collect();
        assert!(order.is_sorted(), "{base}");
    }

    #[test]
    fn without_files_the_base_is_empty() {
        let fx = Fixture::new();
        assert!(fx.port().base().is_empty());
        assert!(fx.port().loaded().is_empty());
    }

    #[test]
    fn a_chat_without_a_directory_reads_only_the_users_files() {
        let fx = Fixture::new();
        fx.write(&fx.config.join("AGENTS.md"), "crest rule");
        fx.ws("AGENTS.md", "agents rule");
        let port = FileInstructions::new(None, Some(fx.home.clone()), Some(fx.config.clone()));
        assert_eq!(port.loaded().len(), 1);
        assert!(port.nested("pkg/a.rs").is_empty());
    }

    #[test]
    fn a_claude_md_that_only_imports_agents_md_adds_nothing() {
        let fx = Fixture::new();
        fx.ws("AGENTS.md", "Use tabs.");
        fx.ws("CLAUDE.md", "@AGENTS.md\n");
        let files = fx.port().loaded();
        assert_eq!(paths(&files), ["AGENTS.md"]);
    }

    #[test]
    fn a_copy_of_a_file_already_read_adds_nothing() {
        let fx = Fixture::new();
        fx.ws("AGENTS.md", "Use tabs.\n");
        fx.ws("CLAUDE.md", "Use tabs.");
        assert_eq!(paths(&fx.port().loaded()), ["AGENTS.md"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_file_already_read_adds_nothing() {
        let fx = Fixture::new();
        fx.ws("AGENTS.md", "Use tabs.");
        std::os::unix::fs::symlink("AGENTS.md", fx.root.join("CLAUDE.md")).unwrap();
        assert_eq!(paths(&fx.port().loaded()), ["AGENTS.md"]);
    }

    #[test]
    fn imports_are_inlined_relative_to_the_file_and_from_home() {
        let fx = Fixture::new();
        fx.ws("docs/style.md", "Style: short functions.");
        fx.write(&fx.home.join("rules.md"), "Home import.");
        fx.write(
            &fx.home.join(".claude/CLAUDE.md"),
            "See @~/rules.md for more.",
        );
        fx.ws("CLAUDE.md", "Read @docs/style.md.\nEmail me@example.com.");
        let files = fx.port().loaded();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].imports, ["~/rules.md"]);
        assert!(files[0].text.contains("Home import."), "{}", files[0].text);
        let claude = &files[1];
        assert_eq!(claude.imports, ["docs/style.md"]);
        assert!(
            claude.text.starts_with("Read\nEmail me@example.com."),
            "{}",
            claude.text
        );
        assert!(
            claude
                .text
                .ends_with("(imported from docs/style.md)\nStyle: short functions.")
        );
    }

    #[test]
    fn imports_in_code_are_text_and_missing_ones_are_ignored() {
        let fx = Fixture::new();
        fx.ws("other.md", "secret rule");
        fx.ws(
            "AGENTS.md",
            "Run `npm i @other.md`.\n```\n@other.md\n```\nUses @types/node.",
        );
        let files = fx.port().loaded();
        assert!(files[0].imports.is_empty());
        assert!(!files[0].text.contains("secret rule"));
        assert!(fx.port().warnings().is_empty());
    }

    #[test]
    fn import_cycles_and_depth_stop() {
        let fx = Fixture::new();
        fx.ws("AGENTS.md", "root @a.md");
        fx.ws("a.md", "A @b.md");
        fx.ws("b.md", "B @a.md @AGENTS.md @c.md");
        fx.ws("c.md", "C @d.md");
        fx.ws("d.md", "D @e.md");
        fx.ws("e.md", "E @f.md");
        fx.ws("f.md", "F");
        let files = fx.port().loaded();
        assert_eq!(files[0].imports, ["a.md", "b.md", "c.md", "d.md", "e.md"]);
        assert!(!files[0].text.contains('F'), "{}", files[0].text);
    }

    #[test]
    fn a_project_file_cannot_import_outside_the_workspace() {
        let fx = Fixture::new();
        fx.write(&fx.home.join("secret.md"), "secret");
        fx.ws("AGENTS.md", "Rules @~/secret.md and @../home/secret.md");
        let port = fx.port();
        let files = port.loaded();
        assert!(!files[0].text.contains("secret\n") && !files[0].text.ends_with("secret"));
        assert!(files[0].imports.is_empty());
        assert_eq!(port.warnings().len(), 2, "{:?}", port.warnings());
    }

    #[test]
    fn long_files_are_cut_with_a_warning() {
        let fx = Fixture::new();
        fx.ws("AGENTS.md", &"é".repeat(MAX_FILE_BYTES));
        let port = fx.port();
        assert!(port.loaded()[0].text.len() <= MAX_FILE_BYTES);
        assert_eq!(port.warnings().len(), 1);
    }

    #[test]
    fn a_changed_file_is_read_again() {
        let fx = Fixture::new();
        let port = fx.port();
        assert!(port.base().is_empty());
        fx.ws("AGENTS.md", "first");
        assert!(port.base().contains("first"));
        std::fs::remove_file(fx.root.join("AGENTS.md")).unwrap();
        assert!(port.base().is_empty());
    }

    #[test]
    fn nested_files_are_found_between_the_root_and_the_path() {
        let fx = Fixture::new();
        fx.ws("AGENTS.md", "root");
        fx.ws("pkg/AGENTS.md", "pkg rule");
        fx.ws("pkg/api/CLAUDE.md", "api rule");
        fx.ws("pkg/api/handlers/x.rs", "");
        fx.ws("other/AGENTS.md", "other");
        let port = fx.port();
        let files = port.nested("pkg/api/handlers/x.rs");
        assert_eq!(paths(&files), ["pkg/AGENTS.md", "pkg/api/CLAUDE.md"]);
        assert!(files.iter().all(|f| f.scope == InstructionScope::Nested));
        assert_eq!(
            paths(&port.nested("pkg/api")),
            ["pkg/AGENTS.md", "pkg/api/CLAUDE.md"]
        );
        assert_eq!(paths(&port.nested("./pkg/new.rs")), ["pkg/AGENTS.md"]);
        assert!(port.nested("x.rs").is_empty(), "the root's are in the base");
        assert!(port.nested("../ws/pkg/a.rs").is_empty());
        assert!(port.nested("/etc/passwd").is_empty());
    }

    #[test]
    fn a_nested_file_already_in_the_base_is_not_repeated() {
        let fx = Fixture::new();
        fx.ws("AGENTS.md", "@pkg/AGENTS.md");
        fx.ws("pkg/AGENTS.md", "pkg rule");
        let port = fx.port();
        assert!(port.base().contains("pkg rule"));
        assert!(port.nested("pkg/a.rs").is_empty());
    }
}
