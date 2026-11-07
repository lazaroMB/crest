//! Searches that span every root of a project.
//!
//! A project can have several directories. Each search runs in each of them,
//! with the same ignore files, sensitive-path rules and sandbox checks as a
//! single root (the per-root search is the one in [`crate::files`],
//! [`crate::find`] and [`crate::grep`], run on a copy scoped to the root),
//! and the answers are merged:
//!
//! - Paths are named as [`domain::roots::RootedPath::address`] does: relative
//!   for the first root, absolute for the others, so every path can be given
//!   back to `read_file` as it is. The notes start with a legend of the roots.
//! - The limit applies to the merged answer, shared fairly between the roots
//!   ([`fair_shares`]); `total` and `truncated` count every root.
//! - A root that is missing or cannot be read adds a note and the others
//!   still answer; when no root answers, the first error is the error.
//! - `tree` without a path shows each root as a top-level folder named after
//!   its directory.
//!
//! With one root (or none) nothing here applies: the answer is the single
//! root's, unchanged.

use std::future::Future;
use std::path::Path;

use application_core::ports::search::{
    FileList, FindFileRequest, GrepRequest, GrepResult, ListFilesRequest, SearchError, TreeNode,
    TreeNodeKind, TreeRequest,
};
use domain::roots::WorkspaceRoots;
use domain::workspace::{WorkspacePath, WorkspaceRoot};

use crate::{RipgrepSearch, files, find, grep};

/// What the roots answered: `(root index, answer)`, and a note for each root
/// that did not answer.
struct Settled<T> {
    answers: Vec<(usize, T)>,
    notes: Vec<String>,
}

impl RipgrepSearch {
    /// A copy that searches `root` alone.
    fn scoped(&self, root: &WorkspaceRoot) -> Self {
        let mut copy = self.clone();
        copy.roots = vec![root.clone()];
        copy
    }

    /// Short names of the roots: the directory name, or the whole path when
    /// two roots share one.
    fn labels(&self) -> Vec<String> {
        let names: Vec<String> = self.roots.iter().map(|r| name_of(r.as_path())).collect();
        names
            .iter()
            .zip(&self.roots)
            .map(|(name, root)| {
                if names.iter().filter(|other| *other == name).count() > 1 {
                    root.as_path().display().to_string()
                } else {
                    name.clone()
                }
            })
            .collect()
    }

    /// `path` of root `index` as the tools name it.
    fn address(&self, index: usize, path: &WorkspacePath) -> WorkspacePath {
        if index == 0 {
            return path.clone();
        }
        let full = self.roots[index].as_path().join(path.as_str());
        WorkspacePath::parse_any(&full.to_string_lossy()).unwrap_or_else(|_| path.clone())
    }

    /// Which root a request path is in, and the path inside it; `None` for
    /// no path (every root).
    fn place(
        &self,
        path: Option<&WorkspacePath>,
    ) -> Result<Option<(usize, WorkspacePath)>, SearchError> {
        let Some(path) = path.filter(|p| !p.is_root()) else {
            return Ok(None);
        };
        let roots = WorkspaceRoots::new(self.roots.iter().cloned());
        let rooted = roots.resolve(path.as_str())?;
        Ok(Some((rooted.index(), rooted.path().clone())))
    }

    /// Runs `f` on a copy scoped to each root (only `only` when given),
    /// after checking that the directory can be read.
    ///
    /// # Errors
    ///
    /// The first error, when no root answered.
    async fn each_root<T, F, Fut>(
        &self,
        only: Option<usize>,
        f: F,
    ) -> Result<Settled<T>, SearchError>
    where
        F: Fn(RipgrepSearch) -> Fut,
        Fut: Future<Output = Result<T, SearchError>>,
    {
        let labels = self.labels();
        let mut settled = Settled {
            answers: Vec::new(),
            notes: Vec::new(),
        };
        let mut first_error = None;
        for (index, root) in self.roots.iter().enumerate() {
            if only.is_some_and(|only| only != index) {
                continue;
            }
            let result = match tokio::fs::read_dir(root.as_path()).await {
                Ok(_) => f(self.scoped(root)).await,
                Err(err) => Err(SearchError::Io(format!("cannot read the directory: {err}"))),
            };
            match result {
                Ok(answer) => settled.answers.push((index, answer)),
                Err(err) => {
                    settled
                        .notes
                        .push(format!("{}: not searched ({err})", labels[index]));
                    first_error.get_or_insert(err);
                }
            }
        }
        match first_error {
            Some(err) if settled.answers.is_empty() => Err(err),
            _ => Ok(settled),
        }
    }

    /// The legend that opens the notes of a merged answer.
    fn legend(&self) -> String {
        let parts: Vec<String> = self
            .labels()
            .iter()
            .zip(&self.roots)
            .enumerate()
            .map(|(index, (label, root))| {
                let kind = if index == 0 { "relative" } else { "absolute" };
                format!("{label} = {} ({kind} paths)", root.as_path().display())
            })
            .collect();
        format!("directories: {}", parts.join("; "))
    }

    /// Whether a search spans several roots.
    fn is_multi(&self) -> bool {
        self.roots.len() > 1
    }

    fn notes_of<T>(&self, settled: &Settled<T>) -> Vec<String> {
        std::iter::once(self.legend())
            .chain(settled.notes.iter().cloned())
            .collect()
    }
}

fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// How many of each list (by its length) to keep so that `limit` are kept
/// in all: an equal share each, and what a short list leaves unused goes to
/// the others.
pub(crate) fn fair_shares(lens: &[usize], limit: usize) -> Vec<usize> {
    let mut shares = vec![0; lens.len()];
    let mut left = limit;
    loop {
        let open: Vec<usize> = (0..lens.len()).filter(|&i| shares[i] < lens[i]).collect();
        if open.is_empty() || left == 0 {
            return shares;
        }
        let each = (left / open.len()).max(1);
        for i in open {
            let take = each.min(lens[i] - shares[i]).min(left);
            shares[i] += take;
            left -= take;
        }
    }
}

/// How the matches of several roots are put together.
#[derive(Clone, Copy)]
enum Merge {
    /// Each root's share in turn, roots in order (sorted listings).
    Shares,
    /// The best match of each root in turn (ranked matches).
    Ranks,
}

/// The first `limit` of the roots' matches, merged as `how`, named as the
/// tools name them.
fn merged_list(
    search: &RipgrepSearch,
    settled: &Settled<Vec<WorkspacePath>>,
    limit: usize,
    how: Merge,
) -> FileList {
    let lens: Vec<usize> = settled.answers.iter().map(|(_, l)| l.len()).collect();
    let total: usize = lens.iter().sum();
    let shares = fair_shares(&lens, limit);
    let mut paths = Vec::new();
    match how {
        Merge::Shares => {
            for ((index, list), share) in settled.answers.iter().zip(shares) {
                paths.extend(list.iter().take(share).map(|p| search.address(*index, p)));
            }
        }
        Merge::Ranks => {
            let mut turns = settled
                .answers
                .iter()
                .map(|(i, l)| (*i, l.iter()))
                .collect::<Vec<_>>();
            while paths.len() < limit.min(total) {
                for (index, list) in &mut turns {
                    if let Some(path) = list.next().filter(|_| paths.len() < limit) {
                        paths.push(search.address(*index, path));
                    }
                }
            }
        }
    }
    FileList {
        truncated: total > paths.len(),
        paths,
        total,
        notes: search.notes_of(settled),
    }
}

/// A file search: `list_files` or `find_file`.
#[derive(Clone)]
pub enum Files {
    List(ListFilesRequest),
    Find(FindFileRequest),
}

/// `list_files` or `find_file` over every root. A listing shares the limit
/// between the roots; the matches of a find are taken best first, a root at
/// a time, so that the best of every root is shown.
pub async fn files(search: &RipgrepSearch, which: Files) -> Result<FileList, SearchError> {
    if !search.is_multi() {
        return match which {
            Files::List(request) => files::list_files(search, request).await,
            Files::Find(request) => find::find_file(search, request).await,
        };
    }
    let (limit, how) = match &which {
        Files::List(request) => (request.limit, Merge::Shares),
        Files::Find(request) => (request.limit, Merge::Ranks),
    };
    let settled = search
        .each_root(None, |one| {
            let which = which.clone();
            async move {
                match which {
                    Files::List(request) => one.all_files(request.glob.as_deref()).await,
                    Files::Find(request) => {
                        let all = FindFileRequest {
                            limit: usize::MAX,
                            ..request
                        };
                        find::find_file(&one, all).await.map(|list| list.paths)
                    }
                }
            }
        })
        .await?;
    Ok(merged_list(search, &settled, limit, how))
}

/// `grep` over every root, or the one a `path` names.
pub async fn grep(search: &RipgrepSearch, request: GrepRequest) -> Result<GrepResult, SearchError> {
    let placed = search.place(request.path.as_ref())?;
    if !search.is_multi() {
        let mut request = request;
        request.path = placed.map(|(_, path)| path);
        return grep::grep(search, request).await;
    }
    let only = placed.as_ref().map(|(index, _)| *index);
    let inside = placed.map(|(_, path)| path);
    let settled = search
        .each_root(only, |one| {
            let mut one_request = request.clone();
            one_request.path.clone_from(&inside);
            async move { grep::grep(&one, one_request).await }
        })
        .await?;
    let lens: Vec<usize> = settled.answers.iter().map(|(_, r)| r.hits.len()).collect();
    let shares = fair_shares(&lens, request.limit);
    let mut merged = GrepResult::default();
    for ((index, result), share) in settled.answers.iter().zip(shares) {
        merged.total += result.total;
        merged.truncated |= result.truncated;
        for hit in result.hits.iter().take(share) {
            let mut hit = hit.clone();
            hit.path = search.address(*index, &hit.path);
            merged.hits.push(hit);
        }
    }
    merged.truncated |= merged.total > merged.hits.len();
    merged.notes = search.notes_of(&settled);
    Ok(merged)
}

/// `tree` of the roots (each a folder named after its directory), or of the
/// directory a `path` names.
pub async fn tree(search: &RipgrepSearch, request: TreeRequest) -> Result<TreeNode, SearchError> {
    let placed = search.place(request.path.as_ref())?;
    if !search.is_multi() {
        let one = TreeRequest {
            path: placed.map(|(_, path)| path),
            depth: request.depth,
        };
        return files::tree(search, one).await;
    }
    let only = placed.as_ref().map(|(index, _)| *index);
    let inside = placed.map(|(_, path)| path);
    let depth = request.depth;
    let settled = search
        .each_root(only, |one| {
            let one_request = TreeRequest {
                path: inside.clone(),
                depth,
            };
            async move { files::tree(&one, one_request).await }
        })
        .await?;
    let readdressed = |index: usize, node: TreeNode| readdress(search, index, node);
    if only.is_some() {
        // `each_root` answered for the one root or failed.
        let (index, node) = settled
            .answers
            .into_iter()
            .next()
            .ok_or_else(|| SearchError::Io("the directory gave no answer".to_owned()))?;
        return Ok(readdressed(index, node));
    }
    let labels = search.labels();
    let children = settled
        .answers
        .into_iter()
        .map(|(index, node)| {
            let mut top = readdressed(index, node);
            top.name.clone_from(&labels[index]);
            top
        })
        .collect();
    Ok(TreeNode {
        name: ".".to_owned(),
        path: WorkspacePath::root(),
        kind: TreeNodeKind::Dir,
        children,
        omitted_files: 0,
        notes: settled.notes,
    })
}

/// `node` of root `index` with every path named as the tools name it.
fn readdress(search: &RipgrepSearch, index: usize, mut node: TreeNode) -> TreeNode {
    node.path = search.address(index, &node.path);
    node.children = node
        .children
        .into_iter()
        .map(|child| readdress(search, index, child))
        .collect();
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (lens, limit, shares)
    #[test]
    fn the_limit_is_shared_fairly_and_unused_shares_move_on() {
        let cases: [(&[usize], usize, &[usize]); 8] = [
            (&[10, 10], 6, &[3, 3]),
            (&[10, 10], 7, &[4, 3]),
            (&[2, 10], 6, &[2, 4]),
            (&[0, 10], 4, &[0, 4]),
            (&[3, 3], 100, &[3, 3]),
            (&[5, 5, 5], 2, &[1, 1, 0]),
            (&[4, 4], 0, &[0, 0]),
            (&[], 5, &[]),
        ];
        for (lens, limit, expected) in cases {
            assert_eq!(fair_shares(lens, limit), expected, "{lens:?} {limit}");
        }
    }
}

#[cfg(test)]
mod search_tests {
    use application_core::ports::search::{ContentSearchPort, FileSearchPort};
    use domain::workspace::PathError;

    use self::fixture::{grep_request, shown, two};
    use super::*;
    use crate::test_support::Workspace;

    /// (glob, limit, expected paths, total, truncated)
    type ListCase<'a> = (Option<&'a str>, usize, &'a [&'a str], usize, bool);
    /// (pattern, path, limit, expected `path:line`, total)
    type GrepCase<'a> = (&'a str, Option<&'a str>, usize, Vec<&'a str>, usize);

    mod fixture {
        use super::super::*;
        use crate::test_support::Workspace;

        /// Two projects' directories: `a` (first) and `b`.
        pub fn two() -> (Workspace, Workspace, RipgrepSearch) {
            let (a, b) = (Workspace::new(), Workspace::new());
            a.write("src/main.rs", "needle one\n")
                .write("README.md", "")
                .write(".env", "needle secret\n");
            b.write("lib/needle.rs", "needle two\nneedle three\n")
                .write("notes.txt", "needle four\n")
                .write("id_rsa", "needle key\n");
            let search = a.search().with_roots(&WorkspaceRoots::new([
                WorkspaceRoot::new(&a.root).unwrap(),
                WorkspaceRoot::new(&b.root).unwrap(),
            ]));
            (a, b, search)
        }

        /// The paths of `list` with `b`'s directory written `<b>`.
        pub fn shown(list: &FileList, b: &Workspace) -> Vec<String> {
            let b = b.root.to_string_lossy().into_owned();
            list.paths
                .iter()
                .map(|p| p.as_str().replace(&b, "<b>"))
                .collect()
        }

        pub fn grep_request(pattern: &str, path: Option<&str>, limit: usize) -> GrepRequest {
            GrepRequest {
                path: path.map(|p| WorkspacePath::parse_any(p).unwrap()),
                limit,
                ..GrepRequest::new(pattern)
            }
        }
    }

    #[tokio::test]
    async fn one_root_answers_exactly_as_before() {
        let a = Workspace::new();
        a.write("src/main.rs", "needle\n").write("README.md", "");
        let plain = a.search();
        let listed = plain.list_files(ListFilesRequest::default()).await.unwrap();
        assert_eq!(listed.notes, Vec::<String>::new());
        assert_eq!(
            listed.paths.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            ["README.md", "src/main.rs"]
        );
        let found = plain.find_file(FindFileRequest::new("main")).await.unwrap();
        assert_eq!((found.paths.len(), found.notes.len()), (1, 0));
        let grepped = plain.grep(GrepRequest::new("needle")).await.unwrap();
        assert_eq!(grepped.hits[0].path.as_str(), "src/main.rs");
        assert!(grepped.notes.is_empty());
        let tree = plain.tree(TreeRequest::default()).await.unwrap();
        assert_eq!(tree.name, ".");
        assert_eq!(tree.children[0].name, "src");
        assert!(tree.notes.is_empty());
        // The same single root given as a list changes nothing.
        let listed_again = plain
            .clone()
            .with_roots(&WorkspaceRoots::new([WorkspaceRoot::new(&a.root).unwrap()]));
        assert_eq!(
            listed_again
                .list_files(ListFilesRequest::default())
                .await
                .unwrap(),
            listed
        );
        // An absolute path inside the root is the same as the relative one.
        let inside = a.root.join("src").to_string_lossy().into_owned();
        let hits = plain
            .grep(grep_request("needle", Some(&inside), 100))
            .await
            .unwrap();
        assert_eq!(hits, grepped);
    }

    /// (glob, limit, expected paths, total, truncated)
    #[tokio::test]
    async fn list_files_merges_labels_and_shares_the_limit() {
        let (_a, b, search) = two();
        let cases: [ListCase; 4] = [
            (
                None,
                500,
                &[
                    "README.md",
                    "src/main.rs",
                    "<b>/lib/needle.rs",
                    "<b>/notes.txt",
                ],
                4,
                false,
            ),
            (None, 2, &["README.md", "<b>/lib/needle.rs"], 4, true),
            (
                None,
                3,
                &["README.md", "src/main.rs", "<b>/lib/needle.rs"],
                4,
                true,
            ),
            (
                Some("*.rs"),
                500,
                &["src/main.rs", "<b>/lib/needle.rs"],
                2,
                false,
            ),
        ];
        for (glob, limit, expected, total, truncated) in cases {
            let request = ListFilesRequest {
                glob: glob.map(str::to_owned),
                limit,
            };
            let list = search.list_files(request).await.unwrap();
            assert_eq!(shown(&list, &b), expected, "{glob:?} {limit}");
            assert_eq!((list.total, list.truncated), (total, truncated));
            // Sensitive files of any root never show; the legend leads.
            assert!(
                list.notes[0].starts_with("directories: "),
                "{:?}",
                list.notes
            );
            assert_eq!(list.notes.len(), 1);
        }
    }

    #[tokio::test]
    async fn find_file_takes_the_best_of_each_root_in_turn() {
        let (_a, _b, search) = two();
        // The order within a root is fzf's; the turns between roots are not.
        let cases: [(&str, usize, usize, [bool; 2]); 3] = [
            ("needle", 20, 1, [true, false]),
            (".", 2, 4, [false, true]),
            ("zzz", 20, 0, [false, false]),
        ];
        for (query, limit, total, absolute) in cases {
            let request = FindFileRequest {
                limit,
                ..FindFileRequest::new(query)
            };
            let list = search.find_file(request).await.unwrap();
            let kinds: Vec<bool> = list.paths.iter().map(WorkspacePath::is_absolute).collect();
            assert_eq!(kinds, absolute[..list.paths.len()], "{query}");
            assert_eq!(list.total, total, "{query}");
            assert_eq!(list.paths.len(), total.min(limit).min(2), "{query}");
        }
    }

    /// (pattern, path in `a`/`b` terms, limit, expected `path:line`, total)
    #[tokio::test]
    async fn grep_spans_the_roots_or_the_one_a_path_names() {
        let (a, b, search) = two();
        let in_b = b.root.join("lib").to_string_lossy().into_owned();
        let in_a = a.root.join("src").to_string_lossy().into_owned();
        let cases: Vec<GrepCase> = vec![
            (
                "needle",
                None,
                100,
                vec![
                    "src/main.rs:1",
                    "<b>/lib/needle.rs:1",
                    "<b>/lib/needle.rs:2",
                    "<b>/notes.txt:1",
                ],
                4,
            ),
            (
                "needle",
                Some(&in_b),
                100,
                vec!["<b>/lib/needle.rs:1", "<b>/lib/needle.rs:2"],
                2,
            ),
            ("needle", Some(&in_a), 100, vec!["src/main.rs:1"], 1),
            ("zzz", None, 100, vec![], 0),
        ];
        let b_dir = b.root.to_string_lossy().into_owned();
        for (pattern, path, limit, expected, total) in cases {
            let result = search
                .grep(grep_request(pattern, path, limit))
                .await
                .unwrap();
            let got: Vec<String> = result
                .hits
                .iter()
                .map(|h| {
                    format!(
                        "{}:{}",
                        h.path.as_str().replace(&b_dir, "<b>"),
                        h.line_number
                    )
                })
                .collect();
            let (mut got, mut expected) = (got, expected);
            // `rg` walks in parallel: the order inside a root is its own.
            got.sort();
            expected.sort();
            assert_eq!(got, expected, "{pattern} {path:?} {limit}");
            assert_eq!(result.total, total, "{pattern} {path:?} {limit}");
            assert_eq!(result.truncated, total > expected.len());
        }
    }

    #[tokio::test]
    async fn grep_limit_is_shared_by_the_roots() {
        let (_a, _b, search) = two();
        let result = search.grep(grep_request("needle", None, 2)).await.unwrap();
        let kinds: Vec<bool> = result.hits.iter().map(|h| h.path.is_absolute()).collect();
        assert_eq!(kinds, [false, true]);
        assert_eq!((result.total, result.truncated), (4, true));
    }

    #[tokio::test]
    async fn tree_shows_each_root_as_a_folder_and_descends_by_absolute_path() {
        let (a, b, search) = two();
        let tree = search.tree(TreeRequest::default()).await.unwrap();
        let tops: Vec<(&str, bool)> = tree
            .children
            .iter()
            .map(|c| (c.name.as_str(), c.path.is_absolute()))
            .collect();
        let (a_name, b_name) = (name_of(&a.root), name_of(&b.root));
        assert_eq!(tops, [(a_name.as_str(), false), (b_name.as_str(), true)]);
        let lib = &tree.children[1].children[0];
        assert_eq!(lib.path.as_str(), b.root.join("lib").to_string_lossy());
        // The path of a folder of the second root leads back into it.
        let request = TreeRequest {
            path: Some(lib.path.clone()),
            depth: 3,
        };
        let sub = search.tree(request).await.unwrap();
        assert_eq!(
            sub.children[0].path.as_str(),
            format!("{}/needle.rs", lib.path)
        );
        let outside = TreeRequest {
            path: Some(WorkspacePath::parse_any("/definitely/not/here").unwrap()),
            depth: 1,
        };
        assert!(matches!(
            search.tree(outside).await,
            Err(SearchError::Path(PathError::OutsideWorkspace { .. }))
        ));
    }

    #[tokio::test]
    async fn a_missing_root_is_noted_and_the_others_still_answer() {
        let (a, b, search) = two();
        std::fs::remove_dir_all(&b.root).unwrap();
        let list = search.list_files(ListFilesRequest::default()).await;
        let list = list.unwrap();
        let hits = search.grep(GrepRequest::new("needle")).await.unwrap();
        let tree = search.tree(TreeRequest::default()).await.unwrap();
        // (what answered, notes: the legend and the missing root)
        let seen = [
            (shown(&list, &b).len(), list.notes.len()),
            (hits.hits.len(), hits.notes.len()),
            (tree.children.len(), tree.notes.len()),
        ];
        assert_eq!(seen, [(2, 2), (1, 2), (1, 1)]);
        assert!(list.notes[1].contains("not searched"), "{:?}", list.notes);
        // With no readable root the failure is the answer.
        std::fs::remove_dir_all(&a.root).unwrap();
        let failed = search.list_files(ListFilesRequest::default()).await;
        assert!(matches!(failed, Err(SearchError::Io(_))), "{failed:?}");
    }

    #[tokio::test]
    async fn a_chat_without_a_directory_still_asks_for_one() {
        let search = RipgrepSearch::standalone().with_roots(&WorkspaceRoots::default());
        let inside = Some(WorkspacePath::parse_any("/x/y").unwrap());
        let errors = [
            search.grep(GrepRequest::new("a")).await.err(),
            search.list_files(ListFilesRequest::default()).await.err(),
            search.find_file(FindFileRequest::new("a")).await.err(),
            search.tree(TreeRequest::default()).await.err(),
            search
                .tree(TreeRequest {
                    path: inside,
                    depth: 1,
                })
                .await
                .err(),
        ];
        let expected = Some(SearchError::Path(PathError::NoRoots));
        assert!(errors.iter().all(|e| *e == expected), "{errors:?}");
    }
}
