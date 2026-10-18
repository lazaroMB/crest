//! Step 4, Commit: one commit per task, with the task id in the message.
//!
//! Only the task's files (and the extra files the user allowed) that git
//! sees changed are staged, so stray changes never leak into the task's
//! commit. The message is `<tipo>(<scope>): <título>` with a `Task: <id>`
//! trailer; the type comes from the title and the scope from the crate of
//! the first file (its `Cargo.toml`) or its top directory.

use async_trait::async_trait;
use domain::plan::Task;
use domain::workspace::WorkspacePath;

use crate::flows::implement::gate::{GateChoice, parse_choice};
use crate::flows::implement::node::{ImplementNode, NodeError, StepContext, StepOutcome};
use crate::flows::implement::state::{EntryKind, Step, TaskStatus};
use crate::ports::fs::{FileSystemPort, ReadFileRequest};
use crate::ports::shell::{CommandOutput, RunCommandRequest, ShellPort};

/// The Conventional Commits type of a task, from the first word of its
/// title (Spanish or English); `feat` by default.
#[must_use]
pub fn commit_type(title: &str) -> &'static str {
    let first = title
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| first.starts_with(p));
    if starts(&["correg", "corrig", "arregl", "fix", "repar"]) {
        "fix"
    } else if starts(&["refactor", "mover", "move", "renombr", "rename", "extra"]) {
        "refactor"
    } else if starts(&["test", "prueb", "prob"]) {
        "test"
    } else if starts(&["document", "doc"]) {
        "docs"
    } else {
        "feat"
    }
}

/// `name = "..."` of the `[package]` table of a `Cargo.toml`.
fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package
            && let Some(value) = line.strip_prefix("name")
            && let Some(value) = value.trim_start().strip_prefix('=')
        {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }
    None
}

/// The scope of a commit touching `file`: the package of the nearest
/// `Cargo.toml` below the workspace root, else `crates/<x>`'s `x` or the
/// top directory; `None` for a file at the root.
pub async fn commit_scope(fs: &dyn FileSystemPort, file: &str) -> Option<String> {
    let path = WorkspacePath::parse(file).ok()?;
    let mut dir = path.parent();
    while let Some(current) = dir.filter(|d| !d.is_root()) {
        let manifest = format!("{}/Cargo.toml", current.as_str());
        if let Ok(manifest) = WorkspacePath::parse(&manifest) {
            let mut request = ReadFileRequest::new(manifest);
            request.limit = Some(200);
            if let Ok(content) = fs.read_file(request).await
                && let Some(name) = package_name(&content.lines.join("\n"))
            {
                return Some(name);
            }
        }
        dir = current.parent();
    }
    let parts: Vec<_> = path.components().collect();
    match parts.as_slice() {
        ["crates", "adapters", x, _, ..] | ["crates", x, _, ..] => Some((*x).to_owned()),
        [top, _, ..] => Some((*top).to_owned()),
        _ => None,
    }
}

/// The commit message of `task`.
#[must_use]
pub fn commit_message(task: &Task, scope: Option<&str>) -> String {
    let title = task.title.trim().trim_end_matches('.');
    let kind = commit_type(title);
    let head = match scope {
        Some(scope) => format!("{kind}({scope}): {title}"),
        None => format!("{kind}: {title}"),
    };
    format!("{head}\n\nTask: {}", task.id)
}

/// The paths of `git status --porcelain=v1 -z` output.
fn changed_paths(status: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut entries = status.split('\0').filter(|e| !e.is_empty());
    while let Some(entry) = entries.next() {
        let Some(path) = entry.get(3..) else { continue };
        paths.push(path.to_owned());
        if entry.starts_with('R') || entry.starts_with('C') {
            // The next entry is the origin of the rename or copy.
            entries.next();
        }
    }
    paths
}

fn git<I, S>(args: I) -> RunCommandRequest
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    RunCommandRequest::new("git", args)
}

/// What [`commit_task`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    /// Committed; the short hash.
    Committed(String),
    /// None of the task's files changed.
    NothingToCommit,
}

fn failure(what: &str, out: &CommandOutput) -> String {
    let text = if out.stderr.trim().is_empty() {
        out.stdout.trim()
    } else {
        out.stderr.trim()
    };
    format!("{what} falló: {text}")
}

/// Stages the changed files among `files` and commits them with
/// `message`.
///
/// # Errors
///
/// A message with git's output when a command cannot run or fails.
pub async fn commit_task(
    shell: &dyn ShellPort,
    files: &[String],
    message: &str,
) -> Result<CommitOutcome, String> {
    let run = |request: RunCommandRequest| async move {
        let display = request.display();
        shell
            .run(request)
            .await
            .map_err(|e| format!("{display}: {e}"))
    };
    let mut status = vec![
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--",
    ];
    status.extend(files.iter().map(String::as_str));
    let out = run(git(status)).await?;
    if !out.success() {
        return Err(failure("git status", &out));
    }
    let changed = changed_paths(&out.stdout);
    if changed.is_empty() {
        return Ok(CommitOutcome::NothingToCommit);
    }
    let mut add = vec!["add", "-A", "--"];
    add.extend(changed.iter().map(String::as_str));
    let out = run(git(add)).await?;
    if !out.success() {
        return Err(failure("git add", &out));
    }
    let out = run(git(["commit", "-m", message])).await?;
    if !out.success() {
        return Err(failure("git commit", &out));
    }
    let out = run(git(["rev-parse", "--short", "HEAD"])).await?;
    Ok(CommitOutcome::Committed(out.stdout.trim().to_owned()))
}

/// Step 4, Commit.
#[derive(Debug, Default)]
pub struct CommitTask;

impl CommitTask {
    /// The step.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ImplementNode for CommitTask {
    fn step(&self) -> Step {
        Step::Commit
    }

    async fn run(&self, ctx: &mut StepContext) -> Result<StepOutcome, NodeError> {
        let Some(task) = ctx.state().current_task().cloned() else {
            return Ok(StepOutcome::GoTo(Step::Verify));
        };
        if let Some(reply) = ctx.user_reply() {
            match parse_choice(reply) {
                GateChoice::Continue | GateChoice::Skip => {
                    ctx.log(
                        EntryKind::Deviation,
                        format!("Tarea {} sin commit, por decisión del usuario.", task.id),
                    );
                    ctx.state_mut().advance();
                    return Ok(StepOutcome::GoTo(Step::Implement));
                }
                GateChoice::Abort => {
                    return Ok(StepOutcome::Abort {
                        reason: format!("no se pudo hacer el commit de {}", task.id),
                    });
                }
                GateChoice::BackToSpec => {
                    return Ok(StepOutcome::BackToSpec {
                        reason: format!("no se pudo hacer el commit de {}", task.id),
                    });
                }
                GateChoice::Retry | GateChoice::Other(_) => {}
            }
        }
        let extra = ctx
            .state()
            .record(&task.id)
            .map(|r| r.extra_files.clone())
            .unwrap_or_default();
        let files: Vec<String> = task.files.iter().chain(&extra).cloned().collect();
        let scope = match task.files.first() {
            Some(file) => commit_scope(ctx.fs().as_ref(), file).await,
            None => None,
        };
        let message = commit_message(&task, scope.as_deref());
        match commit_task(ctx.shell().as_ref(), &files, &message).await {
            Ok(CommitOutcome::Committed(hash)) => {
                let record = ctx.state_mut().record_mut(&task.id);
                if record.status != TaskStatus::Failed {
                    record.status = TaskStatus::Committed;
                }
                record.commit = Some(hash.clone());
                let head = message.lines().next().unwrap_or_default().to_owned();
                ctx.log(
                    EntryKind::Achievement,
                    format!("Tarea {} en el commit {hash}: {head}", task.id),
                );
            }
            Ok(CommitOutcome::NothingToCommit) => {
                ctx.log(
                    EntryKind::Deviation,
                    format!(
                        "Tarea {}: ninguno de sus archivos cambió; no hay commit.",
                        task.id
                    ),
                );
            }
            Err(err) => {
                ctx.log(EntryKind::Failure, format!("Commit de {}: {err}", task.id));
                return Ok(StepOutcome::Ask {
                    prompt: format!(
                        "No pude hacer el commit de la tarea **{}**: {err}. Responde \
                         «reintentar» cuando lo corrijas, «saltar» para seguir sin commit, \
                         «abortar» o «volver al spec».",
                        task.id
                    ),
                });
            }
        }
        ctx.state_mut().advance();
        Ok(StepOutcome::GoTo(Step::Implement))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use super::*;
    use crate::flows::implement::nodes::tests::{Harness, exit, loaded_state};
    use crate::testing::fs::ScriptedShell;

    #[test]
    fn types_come_from_the_title() {
        assert_eq!(commit_type("Agregar SessionCache"), "feat");
        assert_eq!(commit_type("Corregir el parser"), "fix");
        assert_eq!(commit_type("Mover el módulo"), "refactor");
        assert_eq!(commit_type("Pruebas del caché"), "test");
        assert_eq!(commit_type("Documentar la API"), "docs");
    }

    #[tokio::test]
    async fn scopes_come_from_the_crate_or_the_top_directory() {
        let h = Harness::without_shell([]);
        h.fs.set_file(
            "crates/app/Cargo.toml",
            "[package]\nname = \"application\"\n\n[dependencies]\nname = \"x\"\n",
        );
        let fs = h.fs.as_ref();
        assert_eq!(
            commit_scope(fs, "crates/app/src/ports/store.rs")
                .await
                .as_deref(),
            Some("application")
        );
        assert_eq!(
            commit_scope(fs, "crates/adapters/store/src/lib.rs")
                .await
                .as_deref(),
            Some("store")
        );
        assert_eq!(commit_scope(fs, "docs/a.md").await.as_deref(), Some("docs"));
        assert_eq!(commit_scope(fs, "README.md").await, None);
    }

    fn status_args(files: &[&str]) -> Vec<String> {
        [
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
        ]
        .iter()
        .chain(files)
        .map(|s| (*s).to_owned())
        .collect()
    }

    #[tokio::test]
    async fn one_commit_per_task_with_its_id() {
        let message = "feat(src): Hacer T1\n\nTask: T1";
        let shell = Arc::new(
            ScriptedShell::new()
                .on(
                    "git",
                    status_args(&["src/t1.rs"]),
                    exit(0, "?? src/t1.rs\0"),
                )
                .on("git", ["add", "-A", "--", "src/t1.rs"], exit(0, ""))
                .on("git", ["commit", "-m", message], exit(0, ""))
                .on(
                    "git",
                    ["rev-parse", "--short", "HEAD"],
                    exit(0, "abc1234\n"),
                )
                .on("git", status_args(&["src/t2.rs"]), exit(0, "")),
        );
        let h = Harness::new([], Arc::clone(&shell) as _);
        let mut ctx = h.ctx(Step::Commit, loaded_state(&[("T1", &[]), ("T2", &["T1"])]));
        let out = CommitTask::new().run(&mut ctx).await.unwrap();
        assert_eq!(out, StepOutcome::GoTo(Step::Implement));
        let calls: Vec<_> = shell
            .calls()
            .iter()
            .map(RunCommandRequest::display)
            .collect();
        assert_eq!(calls.len(), 4, "{calls:?}");
        assert_eq!(calls[2], format!("git commit -m '{message}'"));
        let record = ctx.state().record(&ctx.state().order[0]).unwrap().clone();
        assert_eq!(
            (record.status, record.commit.as_deref()),
            (TaskStatus::Committed, Some("abc1234"))
        );
        assert_eq!(ctx.state().current, 1);

        // T2 changed nothing: no commit, a deviation in the journal.
        let out = CommitTask::new().run(&mut ctx).await.unwrap();
        assert_eq!(out, StepOutcome::GoTo(Step::Implement));
        assert_eq!(shell.calls().len(), 5);
        assert_eq!(ctx.state().entries(EntryKind::Deviation).count(), 1);
    }

    #[tokio::test]
    async fn a_failed_commit_stops_and_asks() {
        let shell = ScriptedShell::new()
            .on(
                "git",
                status_args(&["src/t1.rs"]),
                exit(0, " M src/t1.rs\0"),
            )
            .with_default(CommandOutput {
                status: Some(1),
                stderr: "hook rejected".into(),
                ..CommandOutput::default()
            });
        let h = Harness::new([], Arc::new(shell));
        let mut ctx = h.ctx(Step::Commit, loaded_state(&[("T1", &[])]));
        let out = CommitTask::new().run(&mut ctx).await.unwrap();
        assert!(
            matches!(&out, StepOutcome::Ask { prompt } if prompt.contains("hook rejected")),
            "{out:?}"
        );
        assert_eq!(ctx.state().current, 0);
    }

    /// A shell that runs real programs in `dir`, for the git test.
    struct DirShell(std::path::PathBuf);

    #[async_trait]
    impl ShellPort for DirShell {
        async fn run(
            &self,
            request: RunCommandRequest,
        ) -> Result<CommandOutput, crate::ports::shell::ShellError> {
            let out = std::process::Command::new(&request.command)
                .args(&request.args)
                .current_dir(&self.0)
                .env("GIT_AUTHOR_NAME", "crest")
                .env("GIT_AUTHOR_EMAIL", "crest@example.com")
                .env("GIT_COMMITTER_NAME", "crest")
                .env("GIT_COMMITTER_EMAIL", "crest@example.com")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .map_err(|e| crate::ports::shell::ShellError::Spawn(e.to_string()))?;
            Ok(CommandOutput {
                status: out.status.code(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                ..CommandOutput::default()
            })
        }
    }

    #[tokio::test]
    async fn commits_each_task_in_a_real_repository() {
        let dir = tempfile::tempdir().unwrap();
        let shell = Arc::new(DirShell(dir.path().to_owned()));
        let init = shell
            .run(RunCommandRequest::new("git", ["init", "-q", "-b", "main"]))
            .await;
        if !init.is_ok_and(|o| o.success()) {
            eprintln!("git is not available; skipping");
            return;
        }
        std::fs::write(dir.path().join("README.md"), "hola\n").unwrap();
        for args in [vec!["add", "README.md"], vec!["commit", "-q", "-m", "init"]] {
            assert!(
                shell
                    .run(RunCommandRequest::new("git", args))
                    .await
                    .unwrap()
                    .success()
            );
        }

        let h = Harness::new([], Arc::clone(&shell) as _);
        let mut state = loaded_state(&[("T1", &[]), ("T2", &["T1"])]);
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        for (i, task) in ["t1", "t2"].iter().enumerate() {
            std::fs::write(dir.path().join(format!("src/{task}.rs")), "// x\n").unwrap();
            // A stray change that must stay out of the task's commit.
            std::fs::write(dir.path().join("stray.txt"), format!("{i}\n")).unwrap();
            let mut ctx = h.ctx(Step::Commit, state);
            let out = CommitTask::new().run(&mut ctx).await.unwrap();
            assert_eq!(out, StepOutcome::GoTo(Step::Implement));
            state = ctx.into_state();
        }

        let log = shell
            .run(RunCommandRequest::new(
                "git",
                [
                    "log",
                    "--format=%s|%(trailers:key=Task,valueonly)",
                    "--name-only",
                ],
            ))
            .await
            .unwrap()
            .stdout;
        let log: Vec<_> = log.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(
            log,
            [
                "feat(src): Hacer T2|T2",
                "src/t2.rs",
                "feat(src): Hacer T1|T1",
                "src/t1.rs",
                "init|",
                "README.md"
            ]
        );
        for id in &state.order {
            assert_eq!(state.record(id).unwrap().status, TaskStatus::Committed);
        }
    }

    #[test]
    fn renames_skip_their_origin() {
        assert_eq!(
            changed_paths("R  new.rs\0old.rs\0 M a.rs\0?? b c.rs\0"),
            ["new.rs", "a.rs", "b c.rs"]
        );
    }
}
