//! Project permission rules: the "allow always" and "deny" rules of a
//! workspace, kept in `<workspace>/.crest/permissions.local.toml`.
//!
//! ```toml
//! # written by crest, editable by hand
//! allow = ["run_command(cargo test *)", "edit_file(src/*)"]
//! deny  = ["run_command(git push *)"]
//! ```
//!
//! Each entry uses the rule notation of [`domain::permission`]. Only the
//! `.local` file is read, never a versioned `permissions.toml`: a cloned
//! repository must not be able to grant itself permissions. Problems never
//! stop the app: a file that does not parse, or an entry that does not make
//! sense, becomes a warning and is skipped.
//!
//! Parsing and writing the text are pure and live here; reading and
//! writing the file is the adapter's job ([`PermissionStorePort`]).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use domain::permission::{PermissionRule, RuleDecision, RuleScope};
use toml::Value;

/// Name of the project permissions file, in the workspace's `.crest`.
pub const PERMISSIONS_FILE: &str = "permissions.local.toml";

/// Where the project rules of `workspace` live:
/// `<workspace>/.crest/permissions.local.toml`.
#[must_use]
pub fn permissions_file(workspace: &Path) -> PathBuf {
    workspace.join(".crest").join(PERMISSIONS_FILE)
}

/// The project rules read from a file, and the problems found in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedRules {
    /// The rules, all with [`RuleScope::Project`]: the allowed ones first,
    /// then the denied ones, each in file order, without duplicates.
    pub rules: Vec<PermissionRule>,
    /// Problems to show; the entries they name were skipped.
    pub warnings: Vec<String>,
}

/// Why the project rules could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PermissionStoreError(pub String);

/// Reads and writes the project rules of a workspace.
#[async_trait]
pub trait PermissionStorePort: Send + Sync {
    /// The rules in [`permissions_file`] of `workspace`, parsed with
    /// [`parse_permissions_toml`]. A missing file is no rules and no
    /// warnings.
    async fn load(&self, workspace: &Path) -> Result<LoadedRules, PermissionStoreError>;

    /// Replaces the rules of `workspace` with `rules`, written with
    /// [`permissions_toml`]. Must be atomic (temporary file and rename) and
    /// create `.crest` when missing.
    async fn save(
        &self,
        workspace: &Path,
        rules: &[PermissionRule],
    ) -> Result<(), PermissionStoreError>;
}

/// The rules of a `permissions.local.toml` read from `text`, and the
/// problems found (prefixed with `file`).
#[must_use]
pub fn parse_permissions_toml(text: &str, file: &str) -> LoadedRules {
    let mut loaded = LoadedRules::default();
    let table: toml::Table = match text.parse() {
        Ok(table) => table,
        Err(err) => {
            let err: toml::de::Error = err;
            let at = err
                .span()
                .map(|span| {
                    let line = text[..span.start.min(text.len())].matches('\n').count() + 1;
                    format!(":{line}")
                })
                .unwrap_or_default();
            loaded.warnings.push(format!(
                "{file}{at}: {}; the file is ignored",
                err.message().trim()
            ));
            return loaded;
        }
    };
    for key in table.keys() {
        if key != "allow" && key != "deny" {
            loaded.warnings.push(format!(
                "{file}: unknown key `{key}` (expected `allow` and `deny`)"
            ));
        }
    }
    for (key, decision) in [
        ("allow", RuleDecision::AllowAlways),
        ("deny", RuleDecision::Deny),
    ] {
        let Some(value) = table.get(key) else {
            continue;
        };
        let Value::Array(entries) = value else {
            loaded
                .warnings
                .push(format!("{file}: `{key}` must be a list of strings"));
            continue;
        };
        for (n, entry) in entries.iter().enumerate() {
            let at = format!("{file}: {key} #{}", n + 1);
            let Value::String(text) = entry else {
                loaded
                    .warnings
                    .push(format!("{at}: not a string; the entry is ignored"));
                continue;
            };
            match PermissionRule::parse(text, decision, RuleScope::Project) {
                Ok(rule) if !loaded.rules.contains(&rule) => loaded.rules.push(rule),
                Ok(_) => {}
                Err(err) => loaded
                    .warnings
                    .push(format!("{at}: {err}; the entry is ignored")),
            }
        }
    }
    loaded
}

/// The text of a `permissions.local.toml` holding `rules` (their scope is
/// not looked at), which [`parse_permissions_toml`] reads back.
#[must_use]
pub fn permissions_toml(rules: &[PermissionRule]) -> String {
    let mut out = String::from(
        "# Project permission rules of crest: written by crest, editable by hand.\n\
         # Only this .local file is read; keep it out of version control.\n",
    );
    for (key, decision) in [
        ("allow", RuleDecision::AllowAlways),
        ("deny", RuleDecision::Deny),
    ] {
        let entries: Vec<String> = rules
            .iter()
            .filter(|rule| rule.decision == decision)
            .map(|rule| toml_string(&rule.to_string()))
            .collect();
        if entries.is_empty() {
            out.push_str(&format!("{key} = []\n"));
        } else {
            out.push_str(&format!("{key} = [\n"));
            for entry in entries {
                out.push_str(&format!("    {entry},\n"));
            }
            out.push_str("]\n");
        }
    }
    out
}

/// `text` as a TOML basic string, in double quotes.
fn toml_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use domain::permission::RuleMatcher;

    use super::*;

    fn project(text: &str, decision: RuleDecision) -> PermissionRule {
        PermissionRule::parse(text, decision, RuleScope::Project).unwrap()
    }

    #[test]
    fn parses_allow_and_deny_lists() {
        let loaded = parse_permissions_toml(
            r#"
allow = ["run_command(cargo test *)", "edit_file(src/*)", "run_command(cargo test *)"]
deny  = ["run_command(git push *)"]
"#,
            "p.toml",
        );
        assert_eq!(loaded.warnings, Vec::<String>::new());
        assert_eq!(
            loaded.rules,
            [
                project("run_command(cargo test *)", RuleDecision::AllowAlways),
                project("edit_file(src/*)", RuleDecision::AllowAlways),
                project("run_command(git push *)", RuleDecision::Deny),
            ]
        );
        assert!(
            loaded
                .rules
                .iter()
                .all(|rule| rule.scope == RuleScope::Project)
        );
        assert_eq!(
            loaded.rules[0].matcher,
            RuleMatcher::Command(vec!["cargo".into(), "test".into()])
        );
    }

    #[test]
    fn problems_become_warnings() {
        let cases = [
            ("allow = [", "p.toml:1: ", "the file is ignored"),
            ("allow = \"x\"", "p.toml: `allow` must be a list", ""),
            ("allow = [1]", "p.toml: allow #1: not a string", ""),
            (
                "deny = [\"run_command(\"]",
                "p.toml: deny #1: invalid permission rule",
                "the entry is ignored",
            ),
            ("ask = []", "p.toml: unknown key `ask`", ""),
        ];
        for (text, start, end) in cases {
            let loaded = parse_permissions_toml(text, "p.toml");
            assert!(loaded.rules.is_empty(), "{text}");
            assert_eq!(loaded.warnings.len(), 1, "{text}: {:?}", loaded.warnings);
            let warning = &loaded.warnings[0];
            assert!(warning.starts_with(start), "{text}: {warning}");
            assert!(warning.ends_with(end), "{text}: {warning}");
        }
        let loaded = parse_permissions_toml("allow = [\"bad(\", \"edit_file(src/*)\"]", "p.toml");
        assert_eq!(loaded.rules.len(), 1, "good entries are kept");
        assert_eq!(loaded.warnings.len(), 1);
    }

    #[test]
    fn empty_text_is_no_rules() {
        assert_eq!(parse_permissions_toml("", "p.toml"), LoadedRules::default());
    }

    #[test]
    fn written_text_reads_back() {
        let rules = vec![
            project("run_command(cargo test *)", RuleDecision::AllowAlways),
            project(
                r#"run_command(echo "quoted" 'x y')"#,
                RuleDecision::AllowAlways,
            ),
            project("mcp__gh__issue", RuleDecision::AllowAlways),
            project("run_command(printf a\\b\tc)", RuleDecision::AllowAlways),
            project("run_command(git push *)", RuleDecision::Deny),
        ];
        let text = permissions_toml(&rules);
        assert!(
            text.contains("allow = [\n    \"run_command(cargo test *)\",\n"),
            "{text}"
        );
        let loaded = parse_permissions_toml(&text, "p.toml");
        assert_eq!(loaded.warnings, Vec::<String>::new());
        assert_eq!(loaded.rules, rules);
        assert_eq!(
            permissions_toml(&[]).lines().skip(2).collect::<Vec<_>>(),
            ["allow = []", "deny = []"]
        );
    }

    #[test]
    fn file_is_in_the_workspace_crest_dir() {
        assert_eq!(
            permissions_file(Path::new("/w")),
            Path::new("/w/.crest/permissions.local.toml")
        );
    }

    #[test]
    fn port_is_object_safe() {
        fn assert_dyn(_: Option<std::sync::Arc<dyn PermissionStorePort>>) {}
        assert_dyn(None);
    }
}
