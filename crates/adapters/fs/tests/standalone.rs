//! A chat without a directory has no root: its file port asks for a
//! directory and never falls back to the process's working directory.
//!
//! This is its own test binary because it changes the working directory of
//! the process, which no other test may see.

use adapter_fs::{LocalFs, WorkspaceGuard};
use application_core::ports::fs::{FileSystemPort, FsError, ReadFileRequest, WriteFileRequest};
use domain::roots::WorkspaceRoots;
use domain::workspace::{NO_DIRECTORY, PathError, WorkspacePath};

#[tokio::test]
async fn a_standalone_chat_does_not_read_the_process_directory() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("secret.txt"), "found in the cwd").unwrap();
    std::env::set_current_dir(cwd.path()).unwrap();
    let fs = LocalFs::new(WorkspaceGuard::new(WorkspaceRoots::default()));
    let absolute = std::fs::canonicalize(cwd.path().join("secret.txt")).unwrap();
    let path = |p: &str| WorkspacePath::parse_any(p).unwrap();

    assert!(fs.roots().is_empty());
    for target in ["secret.txt", ".", &absolute.display().to_string()] {
        let results = [
            fs.read_file(ReadFileRequest::new(path(target)))
                .await
                .map(drop),
            fs.read_bytes(&path(target), 100).await.map(drop),
            fs.exists(&path(target)).await.map(drop),
            fs.list_dir(&path(target)).await.map(drop),
            fs.prepare_write(WriteFileRequest {
                path: path(target),
                content: "x".into(),
            })
            .await
            .map(drop),
        ];
        for result in results {
            let err = result.unwrap_err();
            assert_eq!(err, FsError::Path(PathError::NoRoots), "{target}");
            assert_eq!(err.to_string(), NO_DIRECTORY);
        }
    }
    assert_eq!(
        std::fs::read_to_string(cwd.path().join("secret.txt")).unwrap(),
        "found in the cwd",
        "nothing was written either"
    );
}
