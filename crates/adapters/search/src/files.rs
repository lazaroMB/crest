//! `list_files` and `tree`, built on `rg --files`.
//!
//! The listing runs `rg --files --hidden --no-require-git --glob '!.git'`
//! in the workspace root, followed by the request glob and one exclusion
//! glob per sensitive path rule (later globs win, so a sensitive file
//! never sneaks in through a user glob). The paths are then validated,
//! filtered with [`SensitivePathRules::is_denied`] and sorted, so the order
//! is deterministic even though `rg` walks in parallel.
//!
//! When `rg` is not installed the listing falls back to the `ignore` crate
//! (the walker `rg` itself uses) with the same settings.

use std::ffi::OsString;
use std::path::Path;
use std::process::ExitStatus;

use application::ports::search::{FileList, ListFilesRequest, SearchError, TreeNode, TreeRequest};
use domain::workspace::{SensitivePathRules, WorkspacePath};

use crate::RipgrepSearch;
use crate::detect::Tool;
use crate::process::{self, ProcessError, ProcessSpec};

/// Glob excluding the git directory.
const GIT_DIR_EXCLUSION: &str = "!.git";

/// `rg` exits with 2 when an error occurred (possibly after printing some
/// results, e.g. when one directory is unreadable).
const RG_ERROR_EXIT: i32 = 2;

impl RipgrepSearch {
    /// The globs every walk uses: `.git` excluded, then `user_glob`, then
    /// the sensitive path exclusions (which take precedence).
    pub(crate) fn walk_globs(&self, user_glob: Option<&str>) -> Vec<String> {
        let mut globs = vec![GIT_DIR_EXCLUSION.to_owned()];
        globs.extend(user_glob.map(str::to_owned));
        globs.extend(sensitive_exclusions(&self.rules));
        globs
    }

    /// Arguments shared by every `rg` invocation: no user config (it could
    /// change the output format), hidden files included, ignore files
    /// honored outside git repositories too, `/` between components on
    /// every platform (paths parse as [`WorkspacePath`]), and the walk globs.
    pub(crate) fn rg_common_args(&self, user_glob: Option<&str>) -> Vec<OsString> {
        let mut args: Vec<OsString> = [
            "--no-config",
            "--hidden",
            "--no-require-git",
            "--path-separator",
            "/",
        ]
        .map(OsString::from)
        .to_vec();
        for glob in self.walk_globs(user_glob) {
            args.push("--glob".into());
            args.push(glob.into());
        }
        args
    }

    /// Whether `path` may appear in results.
    pub(crate) fn is_allowed(&self, path: &WorkspacePath) -> bool {
        !self.rules.is_denied(path)
    }

    /// Maps a failure to run `rg` to the port error.
    pub(crate) fn rg_error(err: ProcessError) -> SearchError {
        err.into_search_error(Tool::Ripgrep.install_hint())
    }

    /// Every listable workspace file matching `glob`, sorted by path.
    pub(crate) async fn all_files(
        &self,
        glob: Option<&str>,
    ) -> Result<Vec<WorkspacePath>, SearchError> {
        let spec = ProcessSpec::new(&self.rg, self.root.as_path())
            .arg("--files")
            .args(self.rg_common_args(glob));
        let out = match process::run(&spec, None, self.timeout).await {
            Ok(out) => out,
            Err(ProcessError::NotFound { .. }) => {
                tracing::warn!("rg not found; listing files with the built-in walker");
                return self.walk_files(glob).await;
            }
            Err(err) => return Err(Self::rg_error(err)),
        };
        if out.timed_out {
            return Err(SearchError::Timeout {
                after: self.timeout,
            });
        }
        check_rg_status(out.status, &out.stderr, !out.stdout.is_empty())?;
        let mut paths: Vec<WorkspacePath> = out
            .stdout
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .filter_map(parse_listed)
            .filter(|path| self.is_allowed(path))
            .collect();
        paths.sort_unstable();
        paths.dedup();
        Ok(paths)
    }

    /// The `rg`-less fallback of [`Self::all_files`].
    async fn walk_files(&self, glob: Option<&str>) -> Result<Vec<WorkspacePath>, SearchError> {
        let root = self.root.as_path().to_owned();
        let globs = self.walk_globs(glob);
        let walk = tokio::task::spawn_blocking(move || walk(&root, &globs));
        // On timeout the blocking walk cannot be interrupted; it finishes
        // in the background and its result is discarded.
        let mut paths = match tokio::time::timeout(self.timeout, walk).await {
            Ok(Ok(result)) => result?,
            Ok(Err(join)) => return Err(SearchError::Io(join.to_string())),
            Err(_) => {
                return Err(SearchError::Timeout {
                    after: self.timeout,
                });
            }
        };
        paths.retain(|path| self.is_allowed(path));
        paths.sort_unstable();
        paths.dedup();
        Ok(paths)
    }
}

/// Lists workspace files: what `FileSearchPort::list_files` returns.
pub async fn list_files(
    search: &RipgrepSearch,
    request: ListFilesRequest,
) -> Result<FileList, SearchError> {
    let files = search.all_files(request.glob.as_deref()).await?;
    Ok(FileList::from_matches(files, request.limit))
}

/// Builds the tree that `FileSearchPort::tree` returns. The whole
/// workspace is listed and cut to `request.path` afterwards, so `rg` never
/// gets a path argument (it would follow a symlink given explicitly).
pub async fn tree(search: &RipgrepSearch, request: TreeRequest) -> Result<TreeNode, SearchError> {
    let base = request.path.unwrap_or_else(WorkspacePath::root);
    let files = search.all_files(None).await?;
    Ok(TreeNode::from_files(&base, &files, request.depth))
}

/// Exclusion globs (`!name`) for each sensitive pattern. The rules only
/// know `*` and `?`, so every other glob metacharacter is escaped.
fn sensitive_exclusions(rules: &SensitivePathRules) -> impl Iterator<Item = String> + '_ {
    rules.patterns().map(|pattern| {
        let name = pattern.strip_suffix('/').unwrap_or(pattern);
        let mut glob = String::from("!");
        for c in name.chars() {
            if matches!(c, '\\' | '[' | ']' | '{' | '}' | '!') {
                glob.push('\\');
            }
            glob.push(c);
        }
        glob
    })
}

/// Fails on `rg`'s error exit when nothing useful was printed; partial
/// errors (an unreadable directory) only produce a warning.
pub(crate) fn check_rg_status(
    status: ExitStatus,
    stderr: &[u8],
    produced_output: bool,
) -> Result<(), SearchError> {
    match status.code() {
        Some(RG_ERROR_EXIT) => {
            let message = String::from_utf8_lossy(stderr).trim().to_owned();
            if produced_output {
                tracing::warn!(%message, "rg reported errors");
                Ok(())
            } else if is_pattern_error(&message) {
                Err(SearchError::InvalidPattern(message))
            } else {
                Err(SearchError::Io(message))
            }
        }
        Some(_) => Ok(()),
        None => Err(SearchError::Cancelled),
    }
}

/// Whether `rg`'s stderr is about the regex or a glob.
fn is_pattern_error(stderr: &str) -> bool {
    stderr.contains("regex parse error")
        || stderr.contains("error parsing glob")
        || stderr.contains("regex could not be compiled")
        || stderr.contains("not allowed in a regex")
        || stderr.contains("exceeds size limit")
}

/// A path printed by `rg --files`; non-UTF-8 or invalid ones are skipped.
fn parse_listed(line: &[u8]) -> Option<WorkspacePath> {
    let Ok(text) = std::str::from_utf8(line) else {
        tracing::debug!(path = %String::from_utf8_lossy(line), "skipping non-UTF-8 path");
        return None;
    };
    match WorkspacePath::parse(text) {
        Ok(path) if !path.is_root() => Some(path),
        Ok(_) => None,
        Err(err) => {
            tracing::debug!(%err, "skipping path printed by rg");
            None
        }
    }
}

/// Walks `root` with the `ignore` crate, as `rg --files` does.
fn walk(root: &Path, globs: &[String]) -> Result<Vec<WorkspacePath>, SearchError> {
    let mut overrides = ignore::overrides::OverrideBuilder::new(root);
    for glob in globs {
        overrides
            .add(glob)
            .map_err(|e| SearchError::InvalidPattern(e.to_string()))?;
    }
    let overrides = overrides
        .build()
        .map_err(|e| SearchError::InvalidPattern(e.to_string()))?;
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .add_custom_ignore_filename(".rgignore")
        .overrides(overrides)
        .build();
    let mut paths = Vec::new();
    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                tracing::warn!(%err, "error while walking the workspace");
                continue;
            }
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        if let Some(path) = parse_listed(relative.as_os_str().as_encoded_bytes()) {
            paths.push(path);
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::time::Duration;

    use application::ports::search::TreeNodeKind;

    use super::*;
    #[cfg(unix)]
    use crate::test_support::fake_tool;
    use crate::test_support::{Workspace, p, strs};

    /// The same workspace searched with `rg` and with the fallback walker.
    fn both(ws: &Workspace) -> [(&'static str, RipgrepSearch); 2] {
        [
            ("rg", ws.search()),
            (
                "walker",
                ws.search().with_rg_program("/nonexistent/crest-rg"),
            ),
        ]
    }

    fn sample(ws: &Workspace) {
        ws.write(".gitignore", "ignored.txt\nbuild/\n")
            .write("ignored.txt", "x")
            .write("build/out.o", "x")
            .write("src/main.rs", "fn main() {}")
            .write("src/lib.rs", "")
            .write(".config/app.toml", "")
            .write("README.md", "");
    }

    const EXPECTED: [&str; 5] = [
        ".config/app.toml",
        ".gitignore",
        "README.md",
        "src/lib.rs",
        "src/main.rs",
    ];

    #[tokio::test]
    async fn honors_gitignore_inside_a_git_repository() {
        let ws = Workspace::new();
        ws.git_init();
        sample(&ws);
        assert!(ws.root.join(".git").is_dir());
        for (name, search) in both(&ws) {
            let list = list_files(&search, ListFilesRequest::default())
                .await
                .unwrap();
            assert_eq!(strs(&list.paths), EXPECTED, "{name}");
            assert_eq!((list.total, list.truncated), (5, false), "{name}");
        }
    }

    #[tokio::test]
    async fn honors_gitignore_outside_a_git_repository() {
        let ws = Workspace::new();
        sample(&ws);
        for (name, search) in both(&ws) {
            let list = list_files(&search, ListFilesRequest::default())
                .await
                .unwrap();
            assert_eq!(strs(&list.paths), EXPECTED, "{name}");
        }
    }

    #[tokio::test]
    async fn filters_by_glob() {
        let ws = Workspace::new();
        sample(&ws);
        for (name, search) in both(&ws) {
            let request = ListFilesRequest {
                glob: Some("*.rs".into()),
                ..Default::default()
            };
            let list = list_files(&search, request).await.unwrap();
            assert_eq!(strs(&list.paths), ["src/lib.rs", "src/main.rs"], "{name}");
        }
    }

    #[tokio::test]
    async fn invalid_glob_is_an_invalid_pattern() {
        let ws = Workspace::new();
        sample(&ws);
        for (name, search) in both(&ws) {
            let request = ListFilesRequest {
                glob: Some("{a".into()),
                ..Default::default()
            };
            let err = list_files(&search, request).await.unwrap_err();
            assert!(
                matches!(err, SearchError::InvalidPattern(_)),
                "{name}: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn applies_the_limit_after_sorting() {
        let ws = Workspace::new();
        for i in 0..30 {
            ws.write(&format!("f{i:02}.txt"), "");
        }
        for (name, search) in both(&ws) {
            let request = ListFilesRequest {
                glob: None,
                limit: 3,
            };
            let list = list_files(&search, request).await.unwrap();
            assert_eq!(
                strs(&list.paths),
                ["f00.txt", "f01.txt", "f02.txt"],
                "{name}"
            );
            assert_eq!((list.total, list.truncated), (30, true), "{name}");
        }
    }

    #[tokio::test]
    async fn never_lists_sensitive_paths() {
        let ws = Workspace::new();
        ws.write(".env", "SECRET=1")
            .write("config/.env.local", "SECRET=2")
            .write("certs/server.pem", "")
            .write("id_rsa.pub", "")
            .write("secrets/token", "")
            .write("src/main.rs", "");
        for (name, search) in both(&ws) {
            let list = list_files(&search, ListFilesRequest::default())
                .await
                .unwrap();
            assert_eq!(
                strs(&list.paths),
                ["secrets/token", "src/main.rs"],
                "{name}"
            );

            // A user glob cannot bring them back.
            let request = ListFilesRequest {
                glob: Some(".env*".into()),
                ..Default::default()
            };
            let list = list_files(&search, request).await.unwrap();
            assert!(list.paths.is_empty(), "{name}: {:?}", list.paths);

            let rules = SensitivePathRules::new(["secrets/"]).unwrap();
            let custom = search.with_sensitive_rules(rules);
            let list = list_files(&custom, ListFilesRequest::default())
                .await
                .unwrap();
            assert!(!strs(&list.paths).contains(&"secrets/token"), "{name}");
            assert!(strs(&list.paths).contains(&".env"), "{name}");
        }
    }

    #[test]
    fn sensitive_exclusions_escape_glob_syntax() {
        let rules = SensitivePathRules::new([".git/", "*.pem", "a[1]{x}"]).unwrap();
        let globs: Vec<String> = sensitive_exclusions(&rules).collect();
        assert_eq!(globs, ["!.git", "!*.pem", "!a\\[1\\]\\{x\\}"]);
    }

    #[tokio::test]
    async fn tree_starts_at_path_and_hides_sensitive_files() {
        let ws = Workspace::new();
        sample(&ws);
        ws.write("src/.env", "x").write("src/deep/er/mod.rs", "");
        for (name, search) in both(&ws) {
            let request = TreeRequest {
                path: Some(p("src")),
                depth: 1,
            };
            let node = tree(&search, request).await.unwrap();
            let names: Vec<_> = node.children.iter().map(|c| c.name.as_str()).collect();
            assert_eq!(names, ["deep", "lib.rs", "main.rs"], "{name}");
            assert_eq!(node.children[0].kind, TreeNodeKind::Dir);
            assert_eq!(node.children[0].omitted_files, 1);
        }
    }

    #[tokio::test]
    async fn tree_of_the_root() {
        let ws = Workspace::new();
        sample(&ws);
        let node = tree(&ws.search(), TreeRequest::default()).await.unwrap();
        assert_eq!(node.name, ".");
        let names: Vec<_> = node.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, [".config", "src", ".gitignore", "README.md"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rg_timeout_is_reported() {
        let ws = Workspace::new();
        let bin = tempfile::tempdir().unwrap();
        let slow = fake_tool(bin.path(), "rg", "sleep 5");
        let search = ws
            .search()
            .with_rg_program(slow)
            .with_timeout(Duration::from_millis(200));
        let err = list_files(&search, ListFilesRequest::default())
            .await
            .unwrap_err();
        assert_eq!(
            err,
            SearchError::Timeout {
                after: Duration::from_millis(200)
            }
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rg_errors_without_output_fail() {
        let ws = Workspace::new();
        let bin = tempfile::tempdir().unwrap();
        let broken = fake_tool(bin.path(), "rg", "echo 'rg: boom' >&2; exit 2");
        let err = list_files(
            &ws.search().with_rg_program(broken),
            ListFilesRequest::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err, SearchError::Io("rg: boom".into()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn partial_rg_errors_keep_the_output() {
        let ws = Workspace::new();
        let bin = tempfile::tempdir().unwrap();
        let partial = fake_tool(
            bin.path(),
            "rg",
            "printf 'b\\na\\n../out\\n.env\\n'; echo 'rg: denied' >&2; exit 2",
        );
        let list = list_files(
            &ws.search().with_rg_program(partial),
            ListFilesRequest::default(),
        )
        .await
        .unwrap();
        assert_eq!(strs(&list.paths), ["a", "b"]);
    }
}
