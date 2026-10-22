//! Workspace paths: a canonical root and lexically normalized paths inside it.
//!
//! This module is pure: it never touches the file system. The caller (the
//! fs adapter) canonicalizes the root before building a [`WorkspaceRoot`] and
//! is responsible for resolving symlinks after joining a [`WorkspacePath`].
//! A [`WorkspacePath`] always uses `/`; on Windows `\` is accepted as a
//! separator too, and drive (`C:`), UNC and verbatim (`\\?\`) prefixes mark
//! a path as absolute.

use std::borrow::Cow;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Errors produced while building workspace roots and paths.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// The path resolves outside of the workspace root. Reported to the model
    /// as `PathOutsideWorkspace`.
    #[error("path is outside the workspace: {path}")]
    OutsideWorkspace {
        /// The path exactly as it was received.
        path: String,
    },
    /// The workspace root is not an absolute path.
    #[error("workspace root must be an absolute path: {}", path.display())]
    NotAbsoluteRoot {
        /// The rejected root.
        path: PathBuf,
    },
    /// The path is malformed.
    #[error("invalid path: {reason}")]
    InvalidPath {
        /// Why the path was rejected.
        reason: InvalidPathReason,
    },
}

/// Why a path was rejected as [`PathError::InvalidPath`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidPathReason {
    /// The path is the empty string.
    #[error("path is empty")]
    Empty,
    /// The path contains a NUL byte.
    #[error("path contains a NUL byte")]
    ContainsNul,
    /// The root contains `..` components, so it is not canonical.
    #[error("workspace root is not normalized")]
    RootNotNormalized,
}

impl From<InvalidPathReason> for PathError {
    fn from(reason: InvalidPathReason) -> Self {
        Self::InvalidPath { reason }
    }
}

/// The absolute root directory of a workspace.
///
/// It is expected to be already canonicalized by the caller; this type only
/// checks that it is absolute, has no `..` components and no NUL bytes, and
/// drops redundant `.` components and trailing slashes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceRoot(PathBuf);

impl WorkspaceRoot {
    /// Builds a root from an absolute, canonical path.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, PathError> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(InvalidPathReason::Empty.into());
        }
        if path.as_os_str().as_encoded_bytes().contains(&0) {
            return Err(InvalidPathReason::ContainsNul.into());
        }
        if !path.is_absolute() {
            return Err(PathError::NotAbsoluteRoot { path });
        }
        // `components()` already drops `.` and trailing slashes; only `..`
        // remains to be rejected.
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(InvalidPathReason::RootNotNormalized.into());
        }
        Ok(Self(path.components().collect()))
    }

    /// The root as a path.
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Resolves user or model input to a path inside this workspace.
    ///
    /// Relative input is taken relative to the root; absolute input must
    /// lie under the root. Normalization is purely lexical: `.` and empty
    /// components are dropped and `..` removes the previous component, so
    /// `a/b/../c` becomes `a/c`. A `..` that would climb above the root, or
    /// an absolute path that does not start with the root (compared by
    /// whole components, so `/ws-other` is not inside `/ws`), yields
    /// [`PathError::OutsideWorkspace`]. `"."` (and the root itself) resolve
    /// to [`WorkspacePath::root`]; the empty string and NUL bytes are
    /// rejected as [`PathError::InvalidPath`].
    ///
    /// Symlinks are not followed: the fs adapter must canonicalize the joined
    /// path and check again that it stays under the root.
    pub fn resolve(&self, input: &str) -> Result<WorkspacePath, PathError> {
        check_input(input)?;
        let outside = || PathError::OutsideWorkspace {
            path: input.to_owned(),
        };
        if !is_absolute_input(input) {
            return WorkspacePath::parse(input);
        }
        let (prefix, path) = split_prefix(input);
        let root_prefix = self.0.components().find_map(|c| match c {
            Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().into_owned()),
            _ => None,
        });
        // A prefix (Windows only) must name the root's drive or share; a
        // rooted path without one (`\x`) is on the root's drive.
        if let Some(prefix) = prefix
            && !root_prefix.is_some_and(|r| prefix_key(&r) == prefix_key(prefix))
        {
            return Err(outside());
        }
        // `C:x` is relative to the current directory of drive `C:`.
        if !path.starts_with(is_separator) {
            return Err(outside());
        }
        let segments = normalize(path, true).ok_or_else(outside)?;
        let mut rest = segments.as_slice();
        for component in self.0.components() {
            let Component::Normal(name) = component else {
                continue;
            };
            match rest.split_first() {
                Some((first, tail)) if same_name(first, &name.to_string_lossy()) => rest = tail,
                _ => return Err(outside()),
            }
        }
        Ok(WorkspacePath::from_segments(rest))
    }

    /// The absolute path of `path` under this root. Purely lexical: the fs
    /// adapter must still canonicalize the result before using it.
    pub fn join(&self, path: &WorkspacePath) -> PathBuf {
        if path.is_root() {
            self.0.clone()
        } else {
            self.0.join(path.as_str())
        }
    }
}

/// A lexically normalized path relative to a [`WorkspaceRoot`].
///
/// It never contains `.`, `..`, empty components, a leading or trailing `/`
/// or NUL bytes. The root itself is represented as `"."`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkspacePath(String);

impl WorkspacePath {
    const ROOT: &'static str = ".";

    /// The workspace root itself.
    pub fn root() -> Self {
        Self(Self::ROOT.to_owned())
    }

    /// Parses an already relative path without a root at hand.
    ///
    /// Applies the same lexical normalization as [`WorkspaceRoot::resolve`].
    /// Absolute paths cannot be checked against a root here, so they are
    /// rejected as [`PathError::OutsideWorkspace`].
    pub fn parse(input: &str) -> Result<Self, PathError> {
        check_input(input)?;
        let outside = || PathError::OutsideWorkspace {
            path: input.to_owned(),
        };
        if is_absolute_input(input) {
            return Err(outside());
        }
        let segments = normalize(input, false).ok_or_else(outside)?;
        Ok(Self::from_segments(&segments))
    }

    /// The path as `/`-separated components, or `"."` for the root.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the workspace root itself.
    pub fn is_root(&self) -> bool {
        self.0 == Self::ROOT
    }

    /// The components of the path; empty for the root.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        let path = if self.is_root() { "" } else { self.0.as_str() };
        path.split('/').filter(|c| !c.is_empty())
    }

    /// The containing directory, or `None` for the root.
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        Some(match self.0.rsplit_once('/') {
            Some((parent, _)) => Self(parent.to_owned()),
            None => Self::root(),
        })
    }

    /// The last component, or `None` for the root.
    pub fn file_name(&self) -> Option<&str> {
        if self.is_root() {
            return None;
        }
        Some(
            self.0
                .rsplit_once('/')
                .map_or(self.0.as_str(), |(_, name)| name),
        )
    }

    fn from_segments(segments: &[&str]) -> Self {
        if segments.is_empty() {
            Self::root()
        } else {
            Self(segments.join("/"))
        }
    }
}

impl fmt::Display for WorkspacePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for WorkspacePath {
    type Err = PathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for WorkspacePath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WorkspacePath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Cow::<'de, str>::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for WorkspacePath {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "WorkspacePath".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "Path relative to the workspace root, using `/` as separator; `.` is the root."
        })
    }
}

/// Default patterns denied for reading, see [`SensitivePathRules::default`].
pub const DEFAULT_SENSITIVE_PATTERNS: [&str; 4] = [".env*", "*.pem", "id_rsa*", ".git/"];

/// Why a sensitive-path pattern was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SensitivePatternError {
    /// The pattern is empty (or only `/`).
    #[error("sensitive path pattern is empty")]
    Empty,
    /// The pattern has a `/` other than a single trailing one.
    #[error("sensitive path pattern must be a single name, optionally ending in `/`: {pattern}")]
    ContainsSlash {
        /// The rejected pattern.
        pattern: String,
    },
    /// The pattern contains a NUL byte.
    #[error("sensitive path pattern contains a NUL byte")]
    ContainsNul,
}

/// Name globs of paths that tools must not read.
///
/// Each pattern matches a single path component with `*` and `?`; a path is
/// denied when **any** of its components matches, so `.env*` denies both
/// `.env` and `config/.env.local`, and everything inside a matching
/// directory is denied too. A trailing `/` (as in `.git/`) marks a directory
/// pattern; since this check is lexical and cannot tell files from
/// directories, it matches the same components as the pattern without the
/// slash (a `.git` file of a git worktree is denied as well).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensitivePathRules {
    patterns: Vec<String>,
}

impl SensitivePathRules {
    /// Builds rules from user-provided patterns (e.g. from the Lua config).
    pub fn new<I, S>(patterns: I) -> Result<Self, SensitivePatternError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let patterns = patterns
            .into_iter()
            .map(|p| {
                let pattern = p.into();
                validate_pattern(&pattern)?;
                Ok(pattern)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { patterns })
    }

    /// The patterns as they were given.
    pub fn patterns(&self) -> impl Iterator<Item = &str> {
        self.patterns.iter().map(String::as_str)
    }

    /// Whether reading `path` is denied.
    pub fn is_denied(&self, path: &WorkspacePath) -> bool {
        self.matching_pattern(path).is_some()
    }

    /// The first pattern that denies `path`, to explain the refusal.
    ///
    /// On file systems that ignore case by default (Windows, macOS) the
    /// match ignores case too, so `.ENV` is as sensitive as `.env`.
    pub fn matching_pattern(&self, path: &WorkspacePath) -> Option<&str> {
        self.patterns().find(|pattern| {
            let glob = pattern.strip_suffix('/').unwrap_or(pattern);
            if CASE_INSENSITIVE_FS {
                let glob = glob.to_lowercase();
                path.components()
                    .any(|c| glob_match(&glob, &c.to_lowercase()))
            } else {
                path.components().any(|c| glob_match(glob, c))
            }
        })
    }
}

impl Default for SensitivePathRules {
    /// Denies `.env*`, `*.pem`, `id_rsa*` and `.git/`.
    fn default() -> Self {
        Self {
            patterns: DEFAULT_SENSITIVE_PATTERNS.map(str::to_owned).to_vec(),
        }
    }
}

fn validate_pattern(pattern: &str) -> Result<(), SensitivePatternError> {
    if pattern.contains('\0') {
        return Err(SensitivePatternError::ContainsNul);
    }
    let glob = pattern.strip_suffix('/').unwrap_or(pattern);
    if glob.is_empty() {
        return Err(SensitivePatternError::Empty);
    }
    if glob.contains('/') {
        return Err(SensitivePatternError::ContainsSlash {
            pattern: pattern.to_owned(),
        });
    }
    Ok(())
}

/// Matches a single path component against a glob with `*` (any run of
/// characters, including none) and `?` (exactly one character). Matching is
/// case-sensitive and `*` also matches a leading dot.
fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0, 0);
    // Position of the last `*` seen and the name index it is retried from.
    let mut backtrack: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                backtrack = Some((p, n));
                p += 1;
            }
            Some(c) if *c == '?' || Some(c) == name.get(n) => {
                p += 1;
                n += 1;
            }
            _ => match backtrack {
                Some((star, from)) => {
                    p = star + 1;
                    n = from + 1;
                    backtrack = Some((star, from + 1));
                }
                None => return false,
            },
        }
    }
    pattern
        .get(p..)
        .is_some_and(|rest| rest.iter().all(|c| *c == '*'))
}

/// Whether the platform's default file systems compare names ignoring case
/// (NTFS, APFS), so two spellings of a name open the same file.
const CASE_INSENSITIVE_FS: bool = cfg!(any(windows, target_os = "macos"));

/// Whether `c` separates path components: `/` everywhere, and `\` too on
/// Windows (on Unix it is an ordinary character of a name).
fn is_separator(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// Whether two component names denote the same entry on this platform.
fn same_name(a: &str, b: &str) -> bool {
    if CASE_INSENSITIVE_FS {
        a.to_lowercase() == b.to_lowercase()
    } else {
        a == b
    }
}

/// A prefix in a comparable form: lowercase, `\` separators, and a verbatim
/// drive (`\\?\C:`) reduced to the drive (`c:`).
fn prefix_key(prefix: &str) -> String {
    let key = prefix.replace('/', "\\").to_lowercase();
    match key.strip_prefix(r"\\?\") {
        Some(drive) if drive.len() == 2 && drive.ends_with(':') => drive.to_owned(),
        _ => key,
    }
}

/// Whether `input` is absolute, or at least not relative to the workspace:
/// a leading separator, and on Windows a drive (`C:`), UNC (`\\host`) or
/// verbatim (`\\?\`) prefix.
fn is_absolute_input(input: &str) -> bool {
    input.starts_with(is_separator) || split_prefix(input).0.is_some()
}

/// Splits a Windows prefix (drive letter, UNC share or verbatim/device
/// prefix) off `input`, as [`std::path::Prefix`] would. Always `None` on
/// other platforms, where `C:` is an ordinary name.
fn split_prefix(input: &str) -> (Option<&str>, &str) {
    if !cfg!(windows) {
        return (None, input);
    }
    let bytes = input.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return (Some(&input[..2]), &input[2..]);
    }
    let sep = |b: u8| b == b'/' || b == b'\\';
    if bytes.len() >= 2 && sep(bytes[0]) && sep(bytes[1]) {
        // `\\host\share`, `\\?\C:`, `\\.\device`: the prefix runs
        // through the second component after the leading pair.
        let mut end = 2;
        for _ in 0..2 {
            match input[end..].find(is_separator) {
                Some(i) => end += i + 1,
                None => return (Some(input), ""),
            }
        }
        return (Some(&input[..end - 1]), &input[end - 1..]);
    }
    (None, input)
}

fn check_input(input: &str) -> Result<(), PathError> {
    if input.is_empty() {
        return Err(InvalidPathReason::Empty.into());
    }
    if input.contains('\0') {
        return Err(InvalidPathReason::ContainsNul.into());
    }
    Ok(())
}

/// Lexically normalizes `input` into its components.
///
/// Returns `None` when a `..` climbs above the start. For absolute paths a
/// `..` at the file system root stays there, as the kernel does.
fn normalize(input: &str, absolute: bool) -> Option<Vec<&str>> {
    let mut stack = Vec::new();
    for segment in input.split(is_separator) {
        match segment {
            "" | "." => {}
            ".." => {
                if stack.pop().is_none() && !absolute {
                    return None;
                }
            }
            name => stack.push(name),
        }
    }
    Some(stack)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> WorkspaceRoot {
        WorkspaceRoot::new(if cfg!(windows) { r"C:\ws" } else { "/ws" }).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn root_accepts_absolute_path() {
        assert_eq!(root().as_path(), Path::new("/ws"));
    }

    #[cfg(unix)]
    #[test]
    fn root_strips_trailing_slash_and_dot_components() {
        let r = WorkspaceRoot::new("/ws/./proj/").unwrap();
        assert_eq!(r.as_path(), Path::new("/ws/proj"));
    }

    #[cfg(unix)]
    #[test]
    fn root_accepts_filesystem_root() {
        assert_eq!(WorkspaceRoot::new("/").unwrap().as_path(), Path::new("/"));
    }

    #[cfg(unix)]
    #[test]
    fn root_accepts_unicode() {
        let r = WorkspaceRoot::new("/home/josé/проект").unwrap();
        assert_eq!(r.as_path(), Path::new("/home/josé/проект"));
    }

    #[test]
    fn root_rejects_relative_path() {
        assert_eq!(
            WorkspaceRoot::new("ws"),
            Err(PathError::NotAbsoluteRoot { path: "ws".into() })
        );
        assert_eq!(
            WorkspaceRoot::new("./ws"),
            Err(PathError::NotAbsoluteRoot {
                path: "./ws".into()
            })
        );
    }

    #[test]
    fn root_rejects_empty_path() {
        assert_eq!(WorkspaceRoot::new(""), Err(InvalidPathReason::Empty.into()));
    }

    #[cfg(unix)]
    #[test]
    fn root_rejects_parent_components() {
        assert_eq!(
            WorkspaceRoot::new("/ws/../etc"),
            Err(InvalidPathReason::RootNotNormalized.into())
        );
    }

    #[test]
    fn root_rejects_nul() {
        assert_eq!(
            WorkspaceRoot::new("/ws\0x"),
            Err(InvalidPathReason::ContainsNul.into())
        );
    }

    fn resolve(input: &str) -> Result<String, PathError> {
        root().resolve(input).map(|p| p.as_str().to_owned())
    }

    fn outside(input: &str) -> Result<String, PathError> {
        Err(PathError::OutsideWorkspace {
            path: input.to_owned(),
        })
    }

    #[test]
    fn resolve_keeps_simple_relative_path() {
        assert_eq!(resolve("src/main.rs"), Ok("src/main.rs".into()));
    }

    #[test]
    fn resolve_drops_dots_empty_components_and_trailing_slash() {
        assert_eq!(resolve("./a//b/"), Ok("a/b".into()));
        assert_eq!(resolve("a/./b/."), Ok("a/b".into()));
    }

    #[test]
    fn resolve_allows_parent_that_stays_inside() {
        assert_eq!(resolve("a/b/../c"), Ok("a/c".into()));
        assert_eq!(resolve("a/.."), Ok(".".into()));
    }

    #[test]
    fn resolve_rejects_parent_that_escapes() {
        assert_eq!(resolve("../x"), outside("../x"));
        assert_eq!(resolve("a/../../x"), outside("a/../../x"));
        assert_eq!(resolve(".."), outside(".."));
    }

    #[test]
    fn resolve_rejects_escape_even_if_it_comes_back() {
        // `../ws/a` would lexically land inside again, but climbing above
        // the root is never allowed for relative input.
        assert_eq!(resolve("../ws/a"), outside("../ws/a"));
    }

    #[test]
    fn resolve_treats_dot_as_root() {
        assert_eq!(resolve("."), Ok(".".into()));
        assert_eq!(resolve("./"), Ok(".".into()));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_accepts_root_itself_as_absolute() {
        assert_eq!(resolve("/ws"), Ok(".".into()));
        assert_eq!(resolve("/ws/"), Ok(".".into()));
        assert!(root().resolve("/ws").unwrap() == WorkspacePath::root());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_accepts_absolute_path_inside() {
        assert_eq!(resolve("/ws/src/lib.rs"), Ok("src/lib.rs".into()));
        assert_eq!(resolve("//ws/./src//lib.rs"), Ok("src/lib.rs".into()));
        assert_eq!(resolve("/other/../ws/a"), Ok("a".into()));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_rejects_absolute_path_outside() {
        assert_eq!(resolve("/etc/passwd"), outside("/etc/passwd"));
        assert_eq!(resolve("/"), outside("/"));
        assert_eq!(resolve("/ws/../etc"), outside("/ws/../etc"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_rejects_misleading_prefix() {
        assert_eq!(resolve("/ws-other/a"), outside("/ws-other/a"));
        assert_eq!(resolve("/wsx"), outside("/wsx"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_with_nested_root() {
        let r = WorkspaceRoot::new("/home/me/proj").unwrap();
        assert_eq!(r.resolve("/home/me/proj/a").unwrap().as_str(), "a");
        assert!(r.resolve("/home/me").is_err());
        assert!(r.resolve("/home/me/project/a").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_with_filesystem_root_accepts_any_absolute_path() {
        let r = WorkspaceRoot::new("/").unwrap();
        assert_eq!(r.resolve("/etc/passwd").unwrap().as_str(), "etc/passwd");
        assert_eq!(r.resolve("/..").unwrap(), WorkspacePath::root());
        assert!(r.resolve("..").is_err());
    }

    #[test]
    fn resolve_keeps_unicode_untouched() {
        assert_eq!(
            resolve("docs/ñandú/日本.md"),
            Ok("docs/ñandú/日本.md".into())
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_compares_unicode_roots_exactly() {
        let r = WorkspaceRoot::new("/home/josé").unwrap();
        assert_eq!(r.resolve("/home/josé/ü").unwrap().as_str(), "ü");
        assert!(r.resolve("/home/jose/ü").is_err());
    }

    #[test]
    fn resolve_treats_dotted_names_as_normal() {
        assert_eq!(resolve("...").unwrap(), "...");
        assert_eq!(resolve("a/..b/.c"), Ok("a/..b/.c".into()));
    }

    #[test]
    fn resolve_rejects_empty_input() {
        assert_eq!(resolve(""), Err(InvalidPathReason::Empty.into()));
    }

    #[test]
    fn resolve_rejects_nul() {
        assert_eq!(resolve("a\0b"), Err(InvalidPathReason::ContainsNul.into()));
        assert_eq!(
            resolve("/ws/a\0"),
            Err(InvalidPathReason::ContainsNul.into())
        );
    }

    fn wp(input: &str) -> WorkspacePath {
        root().resolve(input).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn join_builds_absolute_path() {
        assert_eq!(root().join(&wp("a/b.rs")), PathBuf::from("/ws/a/b.rs"));
        assert_eq!(root().join(&wp("ü/日本")), PathBuf::from("/ws/ü/日本"));
    }

    #[cfg(unix)]
    #[test]
    fn join_of_root_is_the_root() {
        assert_eq!(root().join(&WorkspacePath::root()), PathBuf::from("/ws"));
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(wp("./a//b/").to_string(), "a/b");
        assert_eq!(WorkspacePath::root().to_string(), ".");
    }

    #[test]
    fn is_root_only_for_root() {
        assert!(wp(".").is_root());
        assert!(wp("a/..").is_root());
        assert!(!wp("a").is_root());
    }

    #[test]
    fn components_split_path() {
        assert_eq!(
            wp("a/b/c").components().collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(WorkspacePath::root().components().count(), 0);
    }

    #[test]
    fn parent_walks_up_to_root() {
        assert_eq!(wp("a/b/c").parent(), Some(wp("a/b")));
        assert_eq!(wp("a").parent(), Some(WorkspacePath::root()));
        assert_eq!(WorkspacePath::root().parent(), None);
    }

    #[test]
    fn file_name_is_last_component() {
        assert_eq!(wp("a/b/c.rs").file_name(), Some("c.rs"));
        assert_eq!(wp("c.rs").file_name(), Some("c.rs"));
        assert_eq!(wp("a/ñ").file_name(), Some("ñ"));
        assert_eq!(WorkspacePath::root().file_name(), None);
    }

    #[test]
    fn parse_normalizes_relative_paths() {
        assert_eq!(WorkspacePath::parse("./a//b/../c/"), Ok(wp("a/c")));
        assert_eq!(".".parse::<WorkspacePath>(), Ok(WorkspacePath::root()));
    }

    #[test]
    fn parse_rejects_escapes_and_absolute_paths() {
        assert_eq!(
            WorkspacePath::parse("a/../../x"),
            Err(PathError::OutsideWorkspace {
                path: "a/../../x".into()
            })
        );
        assert_eq!(
            WorkspacePath::parse("/ws/a"),
            Err(PathError::OutsideWorkspace {
                path: "/ws/a".into()
            })
        );
        assert_eq!(
            WorkspacePath::parse(""),
            Err(InvalidPathReason::Empty.into())
        );
    }

    #[test]
    fn serializes_as_plain_string() {
        assert_eq!(serde_json::to_string(&wp("a/ñ.rs")).unwrap(), r#""a/ñ.rs""#);
        assert_eq!(
            serde_json::to_string(&WorkspacePath::root()).unwrap(),
            r#"".""#
        );
    }

    #[test]
    fn deserialize_normalizes_and_validates() {
        let p: WorkspacePath = serde_json::from_str(r#""./a//b/""#).unwrap();
        assert_eq!(p, wp("a/b"));
        let p: WorkspacePath = serde_json::from_str(r#""a\u00f1""#).unwrap();
        assert_eq!(p.as_str(), "añ");
        for bad in [r#""../x""#, r#""/etc/passwd""#, r#""""#, r#""a\u0000b""#] {
            let err = serde_json::from_str::<WorkspacePath>(bad).unwrap_err();
            assert!(err.is_data(), "{bad}: {err}");
        }
    }

    #[test]
    fn deserialize_reports_reason() {
        let err = serde_json::from_str::<WorkspacePath>(r#""../x""#).unwrap_err();
        assert!(err.to_string().contains("outside the workspace"), "{err}");
    }

    #[test]
    fn json_schema_is_inline_string() {
        let schema = schemars::schema_for!(WorkspacePath);
        assert_eq!(schema.get("type"), Some(&serde_json::json!("string")));
    }

    #[test]
    fn glob_matches_literals() {
        assert!(glob_match("id_rsa", "id_rsa"));
        assert!(!glob_match("id_rsa", "id_rsa2"));
        assert!(!glob_match("id_rsa", "id_rs"));
        assert!(!glob_match("Env", "env"));
    }

    #[test]
    fn glob_star_matches_any_run() {
        assert!(glob_match(".env*", ".env"));
        assert!(glob_match(".env*", ".env.local"));
        assert!(!glob_match(".env*", "x.env"));
        assert!(glob_match("*.pem", "cert.pem"));
        assert!(glob_match("*.pem", ".pem"));
        assert!(!glob_match("*.pem", "cert.pem.bak"));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(glob_match("a*b*c", "abbbc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
        assert!(glob_match("*", ""));
        assert!(glob_match("**", "x"));
    }

    #[test]
    fn glob_question_matches_one_char() {
        assert!(glob_match("?.rs", "a.rs"));
        assert!(glob_match("?.rs", "ñ.rs"));
        assert!(!glob_match("?.rs", ".rs"));
        assert!(!glob_match("?.rs", "ab.rs"));
    }

    fn denied(path: &str) -> bool {
        SensitivePathRules::default().is_denied(&wp(path))
    }

    #[test]
    fn default_rules_deny_secrets() {
        for path in [
            ".env",
            ".env.local",
            "config/.env.production",
            "certs/server.pem",
            "key.pem",
            "id_rsa",
            "home/.ssh/id_rsa.pub",
            ".git",
            ".git/config",
            "vendor/lib/.git/HEAD",
        ] {
            assert!(denied(path), "{path} should be denied");
        }
    }

    #[test]
    fn default_rules_allow_ordinary_files() {
        for path in [
            "src/main.rs",
            "env.rs",
            "docs/envy/x.md",
            "x.env",
            "cert.pem.md",
            "my_id_rsa",
            ".github/workflows/ci.yml",
            ".gitignore",
            "ñandú/日本.md",
        ] {
            assert!(!denied(path), "{path} should be allowed");
        }
        assert!(!SensitivePathRules::default().is_denied(&WorkspacePath::root()));
    }

    #[test]
    fn matching_pattern_explains_denial() {
        let rules = SensitivePathRules::default();
        assert_eq!(rules.matching_pattern(&wp(".git/HEAD")), Some(".git/"));
        assert_eq!(rules.matching_pattern(&wp("a/b.pem")), Some("*.pem"));
        assert_eq!(rules.matching_pattern(&wp("a/b.rs")), None);
    }

    #[test]
    fn custom_rules_replace_defaults() {
        let rules = SensitivePathRules::new(["secret?.txt", "private/"]).unwrap();
        assert!(rules.is_denied(&wp("secret1.txt")));
        assert!(rules.is_denied(&wp("a/private/b.rs")));
        assert!(!rules.is_denied(&wp("secret12.txt")));
        assert!(!rules.is_denied(&wp(".env")));
        assert_eq!(
            rules.patterns().collect::<Vec<_>>(),
            ["secret?.txt", "private/"]
        );
    }

    #[test]
    fn empty_rules_deny_nothing() {
        let rules = SensitivePathRules::new(Vec::<String>::new()).unwrap();
        assert!(!rules.is_denied(&wp(".env")));
    }

    #[test]
    fn invalid_patterns_are_rejected() {
        assert_eq!(
            SensitivePathRules::new([""]),
            Err(SensitivePatternError::Empty)
        );
        assert_eq!(
            SensitivePathRules::new(["/"]),
            Err(SensitivePatternError::Empty)
        );
        assert_eq!(
            SensitivePathRules::new(["a/b"]),
            Err(SensitivePatternError::ContainsSlash {
                pattern: "a/b".into()
            })
        );
        assert_eq!(
            SensitivePathRules::new(["a\0"]),
            Err(SensitivePatternError::ContainsNul)
        );
    }

    #[test]
    fn errors_have_readable_messages() {
        assert_eq!(
            outside("/etc/passwd").unwrap_err().to_string(),
            "path is outside the workspace: /etc/passwd"
        );
        assert_eq!(
            PathError::from(InvalidPathReason::ContainsNul).to_string(),
            "invalid path: path contains a NUL byte"
        );
        assert_eq!(
            PathError::NotAbsoluteRoot { path: "ws".into() }.to_string(),
            "workspace root must be an absolute path: ws"
        );
    }

    #[test]
    fn rules_ignore_case_where_the_file_system_does() {
        assert_eq!(denied(".ENV"), CASE_INSENSITIVE_FS);
        assert_eq!(denied("sub/.Git/config"), CASE_INSENSITIVE_FS);
        assert_eq!(denied("KEY.PEM"), CASE_INSENSITIVE_FS);
    }

    #[cfg(unix)]
    #[test]
    fn backslashes_and_drive_letters_are_plain_names_on_unix() {
        assert_eq!(resolve(r"a\b"), Ok(r"a\b".into()));
        assert_eq!(resolve("C:"), Ok("C:".into()));
        assert!(!denied(r"sub\.env"), r"`sub\.env` is a single name on Unix");
    }

    #[cfg(windows)]
    mod windows {
        use super::*;

        #[test]
        fn backslash_separates_components() {
            assert_eq!(resolve(r"src\main.rs"), Ok("src/main.rs".into()));
            assert_eq!(resolve(r"a\.\b\..\c"), Ok("a/c".into()));
            assert_eq!(resolve(r"..\x"), outside(r"..\x"));
            assert_eq!(wp(r"a\b").components().collect::<Vec<_>>(), ["a", "b"]);
        }

        #[test]
        fn sensitive_rules_see_backslash_components() {
            assert!(denied(r"sub\.env"));
            assert!(denied(r".git\config"));
            assert!(denied(r"Sub\.ENV.local"));
        }

        #[test]
        fn absolute_paths_on_the_root_drive_resolve() {
            assert_eq!(resolve(r"C:\ws"), Ok(".".into()));
            assert_eq!(resolve(r"C:\ws\src\lib.rs"), Ok("src/lib.rs".into()));
            assert_eq!(resolve(r"c:/WS/src/lib.rs"), Ok("src/lib.rs".into()));
            assert_eq!(resolve(r"\\?\C:\ws\a"), Ok("a".into()));
            assert_eq!(resolve(r"\ws\a"), Ok("a".into()));
        }

        #[test]
        fn other_drives_shares_and_drive_relative_paths_are_outside() {
            for input in [
                r"D:\ws\a",
                r"C:\other\a",
                r"C:\ws\..\other",
                r"C:ws\a",
                r"\\host\share\ws\a",
                r"\\?\D:\ws",
                r"\\.\pipe\x",
            ] {
                assert_eq!(resolve(input), outside(input), "{input}");
            }
        }

        #[test]
        fn parse_rejects_prefixed_paths() {
            for input in [r"C:\x", "C:x", r"\\host\share", r"\x"] {
                assert!(WorkspacePath::parse(input).is_err(), "{input}");
            }
        }

        #[test]
        fn join_uses_the_native_root() {
            assert_eq!(root().join(&wp("a/b.rs")), PathBuf::from(r"C:\ws\a/b.rs"));
            assert_eq!(root().join(&WorkspacePath::root()), PathBuf::from(r"C:\ws"));
        }
    }
}
