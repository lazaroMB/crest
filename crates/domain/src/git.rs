//! The state of the workspace's git repository, as the status bar shows it:
//! branch, distance to its upstream and how many files changed.
//!
//! Pure: [`GitStatus::parse`] reads the output of
//! `git status --porcelain=v2 --branch`; running git is the job of the
//! application layer.

use std::fmt;

/// Where `HEAD` points and what changed in the working tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitStatus {
    /// Branch name, or `None` on a detached `HEAD`.
    pub branch: Option<String>,
    /// Short id of the commit `HEAD` points at (`None` before the first
    /// commit).
    pub commit: Option<String>,
    /// Commits ahead of the upstream.
    pub ahead: u32,
    /// Commits behind the upstream.
    pub behind: u32,
    /// Files with staged changes.
    pub staged: u32,
    /// Files with unstaged changes.
    pub modified: u32,
    /// Untracked files.
    pub untracked: u32,
    /// Files with merge conflicts.
    pub conflicted: u32,
}

/// Length of the short commit id shown for a detached `HEAD`.
const SHORT_ID: usize = 7;

impl GitStatus {
    /// Parses `git status --porcelain=v2 --branch` (lines separated by
    /// `\n`). Unknown lines are ignored.
    #[must_use]
    pub fn parse(output: &str) -> Self {
        let mut status = Self::default();
        for line in output.lines() {
            if let Some(header) = line.strip_prefix("# ") {
                status.header(header);
                continue;
            }
            let mut fields = line.splitn(3, ' ');
            match (fields.next(), fields.next()) {
                (Some("1" | "2"), Some(xy)) => {
                    let mut xy = xy.chars();
                    if xy.next().is_some_and(|x| x != '.') {
                        status.staged += 1;
                    }
                    if xy.next().is_some_and(|y| y != '.') {
                        status.modified += 1;
                    }
                }
                (Some("u"), Some(_)) => status.conflicted += 1,
                (Some("?"), Some(_)) => status.untracked += 1,
                _ => {}
            }
        }
        status
    }

    fn header(&mut self, header: &str) {
        let Some((key, value)) = header.split_once(' ') else {
            return;
        };
        match key {
            "branch.oid" if value != "(initial)" => {
                self.commit = Some(value.chars().take(SHORT_ID).collect());
            }
            "branch.head" if value != "(detached)" => self.branch = Some(value.to_owned()),
            "branch.ab" => {
                for part in value.split_whitespace() {
                    let count = |n: &str| n.parse().unwrap_or(0);
                    if let Some(n) = part.strip_prefix('+') {
                        self.ahead = count(n);
                    } else if let Some(n) = part.strip_prefix('-') {
                        self.behind = count(n);
                    }
                }
            }
            _ => {}
        }
    }

    /// Files that differ from `HEAD` in any way.
    #[must_use]
    pub fn changes(&self) -> u32 {
        self.staged + self.modified + self.untracked + self.conflicted
    }

    /// Whether nothing changed.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.changes() == 0
    }

    /// The branch, or the short commit of a detached `HEAD`.
    #[must_use]
    pub fn head(&self) -> String {
        match (&self.branch, &self.commit) {
            (Some(branch), _) => branch.clone(),
            (None, Some(commit)) => format!("({commit})"),
            (None, None) => "(no commits)".to_owned(),
        }
    }
}

impl fmt::Display for GitStatus {
    /// `main +2 ~3 ?1 !1 ↑1 ↓2`: staged, modified, untracked, conflicted
    /// and distance to the upstream, each only when not zero.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.head())?;
        for (count, mark) in [
            (self.staged, "+"),
            (self.modified, "~"),
            (self.untracked, "?"),
            (self.conflicted, "!"),
            (self.ahead, "↑"),
            (self.behind, "↓"),
        ] {
            if count > 0 {
                write!(f, " {mark}{count}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_distance_and_changes() {
        let output = "# branch.oid 010c40a1b2c3d4e5f6\n\
                      # branch.head main\n\
                      # branch.upstream origin/main\n\
                      # branch.ab +2 -1\n\
                      1 M. N... 100644 100644 100644 a b src/lib.rs\n\
                      1 .M N... 100644 100644 100644 a b README.md\n\
                      1 MM N... 100644 100644 100644 a b Cargo.toml\n\
                      2 R. N... 100644 100644 100644 a b R100 new.rs\told.rs\n\
                      u UU N... 100644 100644 100644 100644 a b c conflict.rs\n\
                      ? notes.txt\n\
                      ? tmp/\n\
                      ! target/\n";
        let status = GitStatus::parse(output);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.commit.as_deref(), Some("010c40a"));
        assert_eq!((status.ahead, status.behind), (2, 1));
        assert_eq!(status.staged, 3);
        assert_eq!(status.modified, 2);
        assert_eq!(status.untracked, 2);
        assert_eq!(status.conflicted, 1);
        assert_eq!(status.changes(), 8);
        assert_eq!(status.to_string(), "main +3 ~2 ?2 !1 ↑2 ↓1");
    }

    #[test]
    fn a_clean_tree_shows_only_the_branch() {
        let status = GitStatus::parse("# branch.oid abcdef0123\n# branch.head feat/x\n");
        assert!(status.is_clean());
        assert_eq!(status.to_string(), "feat/x");
    }

    #[test]
    fn detached_and_initial_heads() {
        let detached = GitStatus::parse("# branch.oid abcdef0123\n# branch.head (detached)\n");
        assert_eq!(detached.head(), "(abcdef0)");
        let initial = GitStatus::parse("# branch.oid (initial)\n# branch.head main\n? a\n");
        assert_eq!(initial.commit, None);
        assert_eq!(initial.to_string(), "main ?1");
        assert_eq!(GitStatus::parse("").head(), "(no commits)");
    }
}
