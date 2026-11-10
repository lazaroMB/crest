//! `/open` over a project's roots, and over none.

use super::*;
use crate::testing::fs::{InMemoryFs, standalone_fs, test_root};

/// `/open` in a chat without a directory asks for one, and an absolute path
/// is checked against every root (the fake has one).
#[test]
fn parse_target_needs_a_root_and_checks_absolute_paths_against_them() {
    assert_eq!(
        parse_target(&standalone_fs(), "@README.md"),
        Err(OpenError::NoDirectory)
    );
    assert!(
        OpenError::NoDirectory
            .to_string()
            .contains("this chat has no directory")
    );
    let fs = InMemoryFs::new(test_root()).with_file("README.md", "# Hi");
    let inside = format!("@{}/README.md", test_root().as_path().display());
    let got = parse_target(&fs, &inside).map(|p| p.to_string());
    assert_eq!(got, Ok("README.md".to_owned()));
    assert!(matches!(
        parse_target(&fs, "@/elsewhere/README.md"),
        Err(OpenError::Outside { .. })
    ));
}
