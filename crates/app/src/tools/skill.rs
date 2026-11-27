//! `skill`: loads a skill (`SKILL.md`, a custom command) for the model, or
//! for the user's `/name`.
//!
//! What a call does, in order:
//!
//! 1. The skill's body gets its arguments (`$ARGUMENTS`, `$0`...) and
//!    variables (`${CLAUDE_SKILL_DIR}`).
//! 2. Each `` !`command` `` in it runs, and its output takes its place.
//!    A command is allowed by the policy, by the skill's own
//!    `allowed-tools`, or by approving the call: when one would ask, the
//!    whole call asks first ([`ToolEffect::Execute`]), naming them. A
//!    denied command (plan mode, a deny rule) is not run; the body says so.
//! 3. The skill's `allowed-tools` are granted until the turn ends
//!    ([`PermissionPolicy::grant`]). A project's skill (a cloned repository
//!    could ship one) that pre-approves more than reading makes the call
//!    ask first too, listing what it pre-approves; "allow always" remembers
//!    that skill with that list.
//! 4. Inline, the result is the body, with the skill's folder and its
//!    other files (read with `file`). Forked (`context: fork`), a subagent
//!    follows the body and its report is the result: the read-only `task`
//!    subagent for `agent: Explore` or `Plan`, the chat's agent otherwise,
//!    with the chat's approvals.
//!
//! The model only loads skills it may (`disable-model-invocation` false);
//! the user's calls ([`skills::by_user`]) load any skill they may run.
//!
//! [`PermissionPolicy::grant`]: domain::permission::PermissionPolicy::grant

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use async_trait::async_trait;
use domain::permission::{ActionRequest, Decision, PermissionMode, PermissionRule, ToolEffect};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::budget::ResultBudget;
use super::registry::{
    PreparedCall, Tool, ToolContext, ToolError, ToolOutput, ToolSource, Typed, TypedTool,
};
use super::task::started_text;
use crate::flows::implement::checks::command_request;
use crate::ports::shell::ShellPort;
use crate::ports::skills::{Skill, SkillContext, SkillKind, SkillScope, SkillsPort};
use crate::usecases::approval::SharedPermissions;
use crate::usecases::background::{AgentJob, AgentKind, AgentOrigin, AgentRunner};
use crate::usecases::skills::{self, SKILL_TOOL};

/// Seconds a `` !`command` `` may run.
pub const SNIPPET_TIMEOUT_S: u64 = 60;
/// Files of a skill's folder listed in its result.
const MAX_LISTED_FILES: usize = 50;
/// Bytes of a skill's file read with `file`.
const MAX_FILE_BYTES: u64 = 256 * 1024;

/// Input of `skill`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SkillInput {
    /// The skill's name, as listed (without a slash).
    pub skill: String,
    /// What the request gives the skill (a file, an issue number...).
    #[serde(default)]
    pub args: Option<String>,
    /// A file of the skill's folder to read instead (a path relative to
    /// it, as the skill's result lists them).
    #[serde(default)]
    pub file: Option<String>,
    /// For a skill that runs in a subagent: true runs it in the
    /// background, and its report arrives later in a message.
    #[serde(default)]
    pub background: Option<bool>,
}

/// What the `skill` tool runs on.
#[derive(Clone)]
pub struct SkillDeps {
    /// The skills.
    pub skills: Arc<dyn SkillsPort>,
    /// The chat's permissions: the commands are checked against them and
    /// `allowed-tools` granted in them.
    pub permissions: SharedPermissions,
    /// Runs the `` !`command` `` snippets.
    pub shell: Arc<dyn ShellPort>,
    /// Runs the forked skills (`context: fork`), once it exists.
    pub runner: Arc<OnceLock<Arc<AgentRunner>>>,
    /// The size results are cut to.
    pub budget: ResultBudget,
}

/// Offers the `skill` tool while there are skills, its description listing
/// them; rebuilt when they change.
pub struct SkillToolSource {
    deps: SkillDeps,
    /// The tools built last and the revision they were built at.
    cache: Mutex<Option<Built>>,
}

/// The tools of one revision of the skills.
type Built = (u64, Vec<Arc<dyn Tool>>);

impl SkillToolSource {
    /// The source over `deps`.
    #[must_use]
    pub fn new(deps: SkillDeps) -> Self {
        Self {
            deps,
            cache: Mutex::new(None),
        }
    }
}

impl ToolSource for SkillToolSource {
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        let revision = self.deps.skills.revision();
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((built, tools)) = cache.as_ref()
            && *built == revision
        {
            return tools.clone();
        }
        let list = self.deps.skills.list();
        let tools: Vec<Arc<dyn Tool>> = if list.is_empty() {
            Vec::new()
        } else {
            vec![Arc::new(Typed(SkillTool {
                deps: self.deps.clone(),
                description: skills::listing(&list),
            }))]
        };
        *cache = Some((revision, tools.clone()));
        tools
    }

    fn revision(&self) -> Option<u64> {
        Some(self.deps.skills.revision())
    }
}

/// `skill` (see the module docs).
pub struct SkillTool {
    deps: SkillDeps,
    description: String,
}

/// A call worked out before it runs.
struct Plan {
    skill: Skill,
    body: String,
    /// Each snippet: where it is, its line, the request and the decision.
    snippets: Vec<Snippet>,
    grants: Vec<PermissionRule>,
    /// What the user must approve first, if anything.
    asks: Vec<String>,
}

struct Snippet {
    range: std::ops::Range<usize>,
    line: String,
    run: Result<ActionRequest, String>,
    request: Option<crate::ports::shell::RunCommandRequest>,
    decision: Decision,
}

impl SkillTool {
    fn find(&self, name: &str) -> Result<Skill, ToolError> {
        let name = name.trim().trim_start_matches('/');
        self.deps.skills.load(name).ok_or_else(|| {
            let names: Vec<_> = self
                .deps
                .skills
                .list()
                .iter()
                .filter(|s| s.model_invocable)
                .map(|s| s.name.clone())
                .collect();
            ToolError::InvalidInput {
                tool: SKILL_TOOL.to_owned(),
                message: format!(
                    "no skill is called `{name}`; the skills are: {}",
                    if names.is_empty() {
                        "none".to_owned()
                    } else {
                        names.join(", ")
                    }
                ),
            }
        })
    }

    /// The call `input` worked out: the body with its arguments, the
    /// commands and what the policy says of each, the grants.
    fn plan(&self, input: &SkillInput, session: &str) -> Result<Plan, ToolError> {
        let skill = self.find(&input.skill)?;
        let dir = skill.meta.dir.display().to_string();
        let body = skills::expand(
            &skill.body,
            input.args.as_deref().unwrap_or_default(),
            &dir,
            session,
        );
        let grants: Vec<PermissionRule> = skill
            .meta
            .allowed_tools
            .iter()
            .flat_map(|entry| skills::translate_tool(entry))
            .collect();
        let mut policy = self
            .deps
            .permissions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        policy.grant(grants.iter().cloned());
        let line_shell = self.deps.shell.line_shell();
        let snippets: Vec<Snippet> = skills::shell_snippets(&body)
            .into_iter()
            .map(|(range, line)| {
                let request = command_request(&line, &line_shell).map(|mut request| {
                    request.timeout_s = SNIPPET_TIMEOUT_S;
                    request
                });
                let run = request.clone().map(|request| {
                    ActionRequest::new(
                        "run_command",
                        self.deps.shell.classify(&request),
                        request.display(),
                    )
                    .with_argv(request.argv())
                });
                let decision = match &run {
                    Ok(action) => policy.decide(action),
                    Err(_) => Decision::Allow,
                };
                Snippet {
                    range,
                    line,
                    run,
                    request: request.ok(),
                    decision,
                }
            })
            .collect();
        let mut asks: Vec<String> = snippets
            .iter()
            .filter(|s| s.decision == Decision::Ask)
            .map(|s| format!("runs `{}`", s.line))
            .collect();
        let risky: Vec<&str> = if skill.meta.scope == SkillScope::User {
            Vec::new()
        } else {
            skill
                .meta
                .allowed_tools
                .iter()
                .filter(|entry| {
                    skills::translate_tool(entry)
                        .iter()
                        .any(|rule| skills::rule_effect(&rule.tool) != ToolEffect::Read)
                })
                .map(String::as_str)
                .collect()
        };
        if !risky.is_empty() && policy.mode != PermissionMode::Plan {
            asks.push(format!("pre-approves {}", risky.join(", ")));
        }
        Ok(Plan {
            skill,
            body,
            snippets,
            grants,
            asks,
        })
    }
}

#[async_trait]
impl TypedTool for SkillTool {
    type Input = SkillInput;

    fn name(&self) -> &str {
        SKILL_TOOL
    }

    fn description(&self) -> &str {
        &self.description
    }

    /// A read, unless a command or what the skill pre-approves needs the
    /// user first ([`ToolEffect::Execute`], so the call asks).
    fn effect(&self, input: &SkillInput) -> ToolEffect {
        if input.file.is_some() {
            return ToolEffect::Read;
        }
        match self.plan(input, "") {
            Ok(plan) if !plan.asks.is_empty() => ToolEffect::Execute,
            _ => ToolEffect::Read,
        }
    }

    async fn prepare(
        &self,
        input: SkillInput,
        ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let by_user = skills::by_user(&ctx.tool_use_id);
        if let Some(file) = &input.file {
            let skill = self.find(&input.skill)?;
            let subject = format!("{} · {file}", skill.meta.name);
            let text = read_skill_file(&skill, file)?;
            let budget = self.deps.budget;
            let run = async move { Ok(ToolOutput::text(budget.truncate_text(&text))) };
            return Ok(PreparedCall::new(subject, run.boxed()));
        }
        let plan = self.plan(&input, &ctx.session_id.to_string())?;
        let meta = &plan.skill.meta;
        if by_user && !meta.user_invocable {
            return Err(ToolError::Execution(format!(
                "`{}` is not a command (user-invocable: false)",
                meta.name
            )));
        }
        if !by_user && !meta.model_invocable {
            return Err(ToolError::Execution(format!(
                "`{}` can only be run by the user (/{})",
                meta.name, meta.name
            )));
        }
        let mut subject = meta.name.clone();
        if let Some(args) = input
            .args
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
        {
            let _ = write!(subject, " {args}");
        }
        if !plan.asks.is_empty() {
            let _ = write!(subject, " ({})", plan.asks.join("; "));
        }
        // The user's forked skills run in the background unless asked
        // otherwise; the model's, when it asks.
        let background = input.background.unwrap_or(by_user);
        let deps = self.deps.clone();
        let run = async move { Ok(run_plan(deps, plan, background).await) };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// Runs the commands of `plan`, grants its rules and loads (or forks) it.
async fn run_plan(deps: SkillDeps, plan: Plan, background: bool) -> ToolOutput {
    let Plan {
        skill,
        body,
        snippets,
        grants,
        ..
    } = plan;
    let mut outputs = Vec::with_capacity(snippets.len());
    for snippet in &snippets {
        outputs.push(run_snippet(deps.shell.as_ref(), snippet).await);
    }
    let ranges: Vec<_> = snippets
        .iter()
        .map(|s| (s.range.clone(), s.line.clone()))
        .collect();
    let body = skills::splice(&body, &ranges, &outputs);
    if !grants.is_empty() {
        deps.permissions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .grant(grants);
    }
    let meta = &skill.meta;
    if meta.context == SkillContext::Fork {
        return fork(&deps, &skill, body, background).await;
    }
    let mut out = String::new();
    if meta.kind == SkillKind::Skill {
        let _ = writeln!(out, "Base directory for this skill: {}", meta.dir.display());
        let files = skill_files(&meta.dir);
        if !files.is_empty() {
            let _ = writeln!(
                out,
                "Its files (read one with skill(skill: \"{}\", file: \"<path>\")): {}",
                meta.name,
                files.join(", ")
            );
        }
        out.push('\n');
    }
    out.push_str(body.trim());
    ToolOutput::text(out)
}

/// The output of `snippet`, or why it did not run.
async fn run_snippet(shell: &dyn ShellPort, snippet: &Snippet) -> String {
    if let Decision::Deny { reason } = &snippet.decision {
        return format!("[`{}` was not run: {reason}]", snippet.line);
    }
    let (Ok(_), Some(request)) = (&snippet.run, &snippet.request) else {
        let error = snippet.run.as_ref().err().cloned().unwrap_or_default();
        return format!("[`{}` could not run: {error}]", snippet.line);
    };
    match shell.run(request.clone()).await {
        Ok(output) => {
            let mut text = output.stdout.trim_end().to_owned();
            if output.status != Some(0) {
                let stderr = output.stderr.trim_end();
                if !stderr.is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(stderr);
                }
                let _ = write!(
                    text,
                    "\n[`{}` exited with {}]",
                    snippet.line,
                    output
                        .status
                        .map_or_else(|| "a signal".to_owned(), |code| code.to_string())
                );
            }
            text
        }
        Err(err) => format!("[`{}` could not run: {err}]", snippet.line),
    }
}

/// A subagent follows `body`: in the background (the result says it
/// started) or while the call waits (its report is the result).
async fn fork(deps: &SkillDeps, skill: &Skill, body: String, background: bool) -> ToolOutput {
    let meta = &skill.meta;
    let agent = meta.agent.as_deref().unwrap_or("general-purpose");
    let Some(runner) = deps.runner.get() else {
        return ToolOutput::error(format!(
            "`{}` runs in a subagent (`{agent}`) that is not available here",
            meta.name
        ));
    };
    let job = AgentJob::new(AgentKind::named(agent), body, AgentOrigin::Model)
        .named(&meta.name)
        .with_model(meta.model.clone());
    if background && runner.background_available() {
        return match runner.spawn(job) {
            Ok(id) => ToolOutput::text(format!(
                "The skill `{}` runs in a subagent. {}",
                meta.name,
                started_text(id)
            )),
            Err(err) => ToolOutput::error(err),
        };
    }
    match runner.run(job).await {
        Ok(report) => ToolOutput::text(format!(
            "The skill `{}` ran in a subagent; its report:\n\n{report}",
            meta.name
        )),
        Err(err) => ToolOutput::error(err),
    }
}

/// The files of a skill's folder besides `SKILL.md`, relative to it.
fn skill_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut visited = BTreeSet::new();
    walk(dir, dir, 0, &mut visited, &mut out);
    out.sort();
    out
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    visited: &mut BTreeSet<PathBuf>,
    out: &mut Vec<String>,
) {
    if depth > 3 || out.len() >= MAX_LISTED_FILES {
        return;
    }
    let Ok(canonical) = dir.canonicalize() else {
        return;
    };
    if !visited.insert(canonical) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if out.len() >= MAX_LISTED_FILES {
            return;
        }
        let hidden = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, depth + 1, visited, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            if rel != "SKILL.md" {
                out.push(rel);
            }
        }
    }
}

/// The text of `file` in `skill`'s folder; never a file outside it.
fn read_skill_file(skill: &Skill, file: &str) -> Result<String, ToolError> {
    let error = |message: String| ToolError::InvalidInput {
        tool: SKILL_TOOL.to_owned(),
        message,
    };
    if skill.meta.kind != SkillKind::Skill {
        return Err(error(format!(
            "`{}` has no folder of files",
            skill.meta.name
        )));
    }
    let dir = skill
        .meta
        .dir
        .canonicalize()
        .map_err(|e| error(format!("the skill's folder: {e}")))?;
    let path = dir
        .join(file.trim().trim_start_matches("./"))
        .canonicalize()
        .map_err(|e| error(format!("{file}: {e}")))?;
    if !path.starts_with(&dir) || !path.is_file() {
        return Err(error(format!(
            "{file} is not a file of the skill `{}`",
            skill.meta.name
        )));
    }
    let size = path.metadata().map(|m| m.len()).unwrap_or_default();
    if size > MAX_FILE_BYTES {
        return Err(error(format!(
            "{file} is larger than {} KB",
            MAX_FILE_BYTES / 1024
        )));
    }
    let bytes = std::fs::read(&path).map_err(|e| ToolError::Execution(format!("{file}: {e}")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use domain::permission::PermissionPolicy;
    use serde_json::{Value, json};

    use super::*;
    use crate::ports::shell::CommandOutput;
    use crate::ports::skills::SkillMeta;
    use crate::testing::fs::ScriptedShell;
    use crate::testing::skills::FixedSkills;

    struct Fixture {
        skills: Arc<FixedSkills>,
        shell: Arc<ScriptedShell>,
        permissions: SharedPermissions,
        source: SkillToolSource,
    }

    fn fixture(skills: FixedSkills, shell: ScriptedShell, mode: PermissionMode) -> Fixture {
        let skills = Arc::new(skills);
        let shell = Arc::new(shell);
        let permissions = Arc::new(Mutex::new(PermissionPolicy::new(mode)));
        let source = SkillToolSource::new(SkillDeps {
            skills: Arc::clone(&skills) as _,
            permissions: Arc::clone(&permissions),
            shell: Arc::clone(&shell) as _,
            runner: Arc::default(),
            budget: ResultBudget::default(),
        });
        Fixture {
            skills,
            shell,
            permissions,
            source,
        }
    }

    fn ok(stdout: &str) -> CommandOutput {
        CommandOutput {
            status: Some(0),
            stdout: stdout.into(),
            ..CommandOutput::default()
        }
    }

    fn ctx(id: &str) -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new(id))
    }

    impl Fixture {
        fn tool(&self) -> Arc<dyn Tool> {
            self.source.tools().pop().expect("the skill tool")
        }

        /// The effect, subject and output of a call with `input` by `id`.
        async fn call(&self, id: &str, input: Value) -> (ToolEffect, String, ToolOutput) {
            let tool = self.tool();
            let effect = tool.effect(&input);
            match tool.prepare(input, &ctx(id)).await {
                Ok(prepared) => {
                    let subject = prepared.subject.clone();
                    (effect, subject, prepared.run.await.unwrap())
                }
                Err(err) => (effect, String::new(), ToolOutput::from(err)),
            }
        }
    }

    #[tokio::test]
    async fn there_is_no_tool_without_skills_and_it_lists_them() {
        let f = fixture(
            FixedSkills::new(),
            ScriptedShell::new(),
            PermissionMode::Ask,
        );
        assert!(f.source.tools().is_empty());
        let before = f.source.revision();
        f.skills
            .add(SkillMeta::new("review", "Reviews the diff"), "Review it.");
        assert_ne!(f.source.revision(), before);
        let tool = f.tool();
        assert_eq!(tool.name(), SKILL_TOOL);
        assert!(tool.description().contains("- review: Reviews the diff"));
    }

    #[tokio::test]
    async fn an_inline_skill_returns_its_body_with_the_arguments() {
        let skills = FixedSkills::new().with(
            SkillMeta::new("fix", "Fixes an issue"),
            "Fix issue $ARGUMENTS following our style.",
        );
        let f = fixture(skills, ScriptedShell::new(), PermissionMode::Ask);
        let (effect, subject, output) = f
            .call("toolu_1", json!({ "skill": "fix", "args": "#42" }))
            .await;
        assert_eq!(effect, ToolEffect::Read);
        assert_eq!(subject, "fix #42");
        assert!(!output.is_error);
        let text = output.text_content();
        assert!(
            text.starts_with("Base directory for this skill: .claude/skills/fix"),
            "{text}"
        );
        assert!(
            text.ends_with("Fix issue #42 following our style."),
            "{text}"
        );
        let (_, _, missing) = f.call("toolu_2", json!({ "skill": "nope" })).await;
        assert!(missing.is_error);
        assert!(missing.text_content().contains("the skills are: fix"));
    }

    #[tokio::test]
    async fn who_may_load_a_skill_is_checked() {
        let mut only_user = SkillMeta::new("deploy", "Deploys");
        only_user.model_invocable = false;
        let mut only_model = SkillMeta::new("context", "Background");
        only_model.user_invocable = false;
        let skills = FixedSkills::new()
            .with(only_user, "Deploy.")
            .with(only_model, "Context.");
        let f = fixture(skills, ScriptedShell::new(), PermissionMode::Ask);
        let (_, _, by_model) = f.call("toolu_1", json!({ "skill": "deploy" })).await;
        assert!(by_model.is_error);
        assert!(by_model.text_content().contains("only be run by the user"));
        let (_, _, by_user) = f.call("skill_user_1", json!({ "skill": "deploy" })).await;
        assert!(!by_user.is_error, "{}", by_user.text_content());
        let (_, _, user_on_model_only) =
            f.call("skill_user_2", json!({ "skill": "context" })).await;
        assert!(user_on_model_only.is_error);
        assert!(!f.tool().description().contains("deploy"));
    }

    #[tokio::test]
    async fn snippets_run_and_ask_unless_allowed() {
        let mut review = SkillMeta::new("review", "Reviews");
        review.scope = SkillScope::User;
        review.allowed_tools = vec!["Bash(npm test:*)".into()];
        let skills = FixedSkills::new()
            .with(review, "Status:\n!`git status --short`\nTests: !`npm test`")
            .with(SkillMeta::new("build", "Builds"), "Output: !`make build`");
        let shell = ScriptedShell::new()
            .on("git", ["status", "--short"], ok("M a.rs\n"))
            .on("npm", ["test"], ok("1 passing"))
            .on("make", ["build"], ok("done"));
        let f = fixture(skills, shell, PermissionMode::Ask);
        // git status is a read, npm test is allowed by the skill itself.
        let (effect, _, output) = f.call("toolu_1", json!({ "skill": "review" })).await;
        assert_eq!(effect, ToolEffect::Read);
        assert!(
            output
                .text_content()
                .ends_with("Status:\nM a.rs\nTests: 1 passing"),
            "{}",
            output.text_content()
        );
        // make build needs the user: the call asks, naming it.
        let (effect, subject, output) = f.call("toolu_2", json!({ "skill": "build" })).await;
        assert_eq!(effect, ToolEffect::Execute);
        assert_eq!(subject, "build (runs `make build`)");
        assert!(output.text_content().ends_with("Output: done"));
        assert_eq!(f.shell.calls().len(), 3);
    }

    #[tokio::test]
    async fn denied_snippets_do_not_run() {
        let skills =
            FixedSkills::new().with(SkillMeta::new("build", "Builds"), "Output: !`make build`");
        let f = fixture(skills, ScriptedShell::new(), PermissionMode::Plan);
        let (effect, _, output) = f.call("toolu_1", json!({ "skill": "build" })).await;
        assert_eq!(effect, ToolEffect::Read);
        assert!(output.text_content().contains("[`make build` was not run:"));
        assert!(f.shell.calls().is_empty());
    }

    #[tokio::test]
    async fn allowed_tools_are_granted_and_a_projects_must_be_approved() {
        let mut mine = SkillMeta::new("mine", "Mine");
        mine.scope = SkillScope::User;
        mine.allowed_tools = vec!["Edit".into(), "Bash(cargo test:*)".into()];
        let mut repo = SkillMeta::new("repo", "From the repository");
        repo.allowed_tools = vec!["Read".into(), "Bash(cargo test:*)".into()];
        let mut reads = SkillMeta::new("reads", "Only reads");
        reads.allowed_tools = vec!["Read".into(), "Grep".into()];
        let skills = FixedSkills::new()
            .with(mine, "x")
            .with(repo, "y")
            .with(reads, "z");
        let f = fixture(skills, ScriptedShell::new(), PermissionMode::Ask);
        let (effect, _, _) = f.call("toolu_1", json!({ "skill": "mine" })).await;
        assert_eq!(
            effect,
            ToolEffect::Read,
            "the user's own skills are trusted"
        );
        assert_eq!(f.permissions.lock().unwrap().grants.len(), 3);
        let (effect, subject, _) = f.call("toolu_2", json!({ "skill": "repo" })).await;
        assert_eq!(effect, ToolEffect::Execute);
        assert_eq!(subject, "repo (pre-approves Bash(cargo test:*))");
        let (effect, _, _) = f.call("toolu_3", json!({ "skill": "reads" })).await;
        assert_eq!(effect, ToolEffect::Read);
    }

    #[tokio::test]
    async fn a_skills_files_are_listed_and_read_only_inside_its_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("pdf");
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("SKILL.md"), "x").unwrap();
        std::fs::write(dir.join("forms.md"), "Fill the form.").unwrap();
        std::fs::write(dir.join("scripts/fill.py"), "print()").unwrap();
        std::fs::write(tmp.path().join("secret.txt"), "no").unwrap();
        let mut meta = SkillMeta::new("pdf", "PDFs");
        meta.dir = dir.canonicalize().unwrap();
        let f = fixture(
            FixedSkills::new().with(meta, "See forms.md."),
            ScriptedShell::new(),
            PermissionMode::Ask,
        );
        let (_, _, output) = f.call("toolu_1", json!({ "skill": "pdf" })).await;
        assert!(
            output.text_content().contains("forms.md, scripts/fill.py"),
            "{}",
            output.text_content()
        );
        let (_, subject, file) = f
            .call("toolu_2", json!({ "skill": "pdf", "file": "forms.md" }))
            .await;
        assert_eq!(subject, "pdf · forms.md");
        assert_eq!(file.text_content(), "Fill the form.");
        let (_, _, outside) = f
            .call(
                "toolu_3",
                json!({ "skill": "pdf", "file": "../secret.txt" }),
            )
            .await;
        assert!(outside.is_error);
    }

    #[tokio::test]
    async fn a_fork_without_its_subagent_says_so() {
        let mut meta = SkillMeta::new("research", "Researches");
        meta.context = SkillContext::Fork;
        let f = fixture(
            FixedSkills::new().with(meta, "Research it."),
            ScriptedShell::new(),
            PermissionMode::Ask,
        );
        let (_, _, output) = f.call("toolu_1", json!({ "skill": "research" })).await;
        assert!(output.is_error);
        assert!(output.text_content().contains("not available here"));
    }
}
