//! The guard over several roots: a project's directories are one workspace,
//! and every rule of a single root holds in each of them.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use application_core::ports::fs::{
    EditFileRequest, FileSystemPort, FsError, ReadFileRequest, WriteFileRequest,
};
use domain::roots::WorkspaceRoots;
use domain::workspace::{PathError, WorkspacePath};

use super::*;
use crate::guard::canonical_root;

/// Roots `b`, `c`, `d` next to `bc` (not a root, but `b` is its prefix) and
/// `outside`. `c` holds a sensitive `.env` and `.git/config` and a symlink
/// out; `b` holds a symlink into `c`.
struct Fixture {
    _dir: tempfile::TempDir,
    base: PathBuf,
    fs: LocalFs,
}

fn p(path: &str) -> WorkspacePath {
    WorkspacePath::parse_any(path).unwrap()
}

/// The files of the fixture (each holds its own name).
const FILES: &str = "b/one.txt c/src/two.txt d/three.txt bc/x.txt outside/secret.txt c/.git/config";

fn populate(base: &Path) {
    for file in FILES.split(' ') {
        let path = base.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, path.file_name().unwrap().as_encoded_bytes()).unwrap();
    }
    std::fs::write(base.join("c/.env"), "KEY=1").unwrap();
    symlink(base.join("outside"), base.join("c/out")).unwrap();
    symlink(base.join("c"), base.join("b/into-c")).unwrap();
}

/// A port whose roots are the directories `names` of `base`.
fn open_roots(base: &Path, names: &[&str]) -> LocalFs {
    let roots = names
        .iter()
        .map(|name| canonical_root(&base.join(name)).unwrap());
    LocalFs::new(WorkspaceGuard::new(WorkspaceRoots::new(roots)))
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    populate(&base);
    let fs = open_roots(&base, &["b", "c", "d"]);
    Fixture {
        _dir: dir,
        base,
        fs,
    }
}

impl Fixture {
    fn abs(&self, path: &str) -> String {
        self.base.join(path).display().to_string()
    }
}

#[tokio::test]
async fn a_path_under_any_root_resolves_and_one_under_none_is_outside() {
    let f = fixture();
    // (input, the address the tools see, or None when it must be refused)
    let cases = [
        ("one.txt", Some("one.txt".to_owned())),
        (&f.abs("b/one.txt"), Some("one.txt".to_owned())),
        (&f.abs("c/src/two.txt"), Some(f.abs("c/src/two.txt"))),
        (&f.abs("d/three.txt"), Some(f.abs("d/three.txt"))),
        // `b` is a prefix of `bc`, and `bc` is in no root.
        (&f.abs("bc/x.txt"), None),
        (&f.abs("outside/secret.txt"), None),
        // `..` out of a root, even into another root's sibling.
        (&f.abs("c/../outside/secret.txt"), None),
        (&f.abs("d/../bc/x.txt"), None),
        ("../outside/secret.txt", None),
        // Symlinks: out of a root is refused, into another root is inside.
        (&f.abs("c/out/secret.txt"), None),
        (
            &f.abs("b/into-c/src/two.txt"),
            Some("into-c/src/two.txt".to_owned()),
        ),
    ];
    for (input, expected) in cases {
        let got = f.fs.guard().resolve_existing(input).await;
        match (got, expected) {
            (Ok((path, _)), Some(address)) => assert_eq!(path.as_str(), address, "{input}"),
            (Err(FsError::Path(PathError::OutsideWorkspace { .. })), None) => {}
            (got, expected) => panic!("{input}: {got:?} vs {expected:?}"),
        }
    }
}

#[tokio::test]
async fn the_file_tools_read_and_write_in_the_second_root() {
    let f = fixture();
    let two = f.abs("c/src/two.txt");
    let read = f.fs.read_file(ReadFileRequest::new(p(&two))).await.unwrap();
    assert_eq!(read.render_numbered().trim(), "1\ttwo.txt");

    let edit = EditFileRequest {
        path: p(&two),
        old: "two".into(),
        new: "2".into(),
    };
    let pending = f.fs.prepare_edit(edit).await.unwrap();
    // The change remembers its root, so `apply` writes in the same place.
    assert_eq!(pending.path().as_str(), two);
    f.fs.apply(pending).await.unwrap();
    assert_eq!(std::fs::read_to_string(&two).unwrap(), "2.txt");

    let created = f.abs("d/new/dir/out.txt");
    let write = WriteFileRequest {
        path: p(&created),
        content: "hi".into(),
    };
    let pending = f.fs.prepare_write(write).await.unwrap();
    assert!(pending.creates());
    f.fs.apply(pending).await.unwrap();
    assert_eq!(std::fs::read_to_string(&created).unwrap(), "hi");
    assert!(
        !f.base.join("b/new").exists(),
        "nothing landed in the first root"
    );

    // A relative path is the first root's, never the second's.
    assert!(matches!(
        f.fs.read_file(ReadFileRequest::new(p("src/two.txt"))).await,
        Err(FsError::NotFound { .. })
    ));
    let listed = f.fs.list_dir(&p(&f.abs("c"))).await.unwrap();
    let names: Vec<_> = listed.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["out", "src"], ".env and .git are left out");
}

#[tokio::test]
async fn sensitive_paths_are_denied_in_each_root_relative_to_that_root() {
    let f = fixture();
    for denied in ["c/.env", "c/.git/config"] {
        let got =
            f.fs.read_file(ReadFileRequest::new(p(&f.abs(denied))))
                .await;
        assert!(
            matches!(got, Err(FsError::SensitivePath { .. })),
            "{denied}: {got:?}"
        );
        let write = WriteFileRequest {
            path: p(&f.abs(denied)),
            content: "x".into(),
        };
        let got = f.fs.prepare_write(write).await;
        assert!(
            matches!(got, Err(FsError::SensitivePath { .. })),
            "{denied}: {got:?}"
        );
    }
    // A root that sits under a directory the rules name is not denied: the
    // rules see the path inside the root, not the one above it.
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap().join(".git/proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "a").unwrap();
    let guard = WorkspaceGuard::new(WorkspaceRoots::new([
        canonical_root(&f.base.join("b")).unwrap(),
        canonical_root(&root).unwrap(),
    ]));
    let fs = LocalFs::new(guard);
    let abs = root.join("a.txt").display().to_string();
    assert!(fs.read_file(ReadFileRequest::new(p(&abs))).await.is_ok());
}

#[tokio::test]
async fn a_root_listed_twice_changes_nothing() {
    let f = fixture();
    let fs = open_roots(&f.base, &["b", "b", "c", "b"]);
    assert_eq!(fs.roots().len(), 2);
    let two = p(&f.abs("c/src/two.txt"));
    assert!(fs.read_file(ReadFileRequest::new(two)).await.is_ok());
}
