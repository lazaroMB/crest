//! Permission policy: decides whether a tool action is allowed, needs the
//! user's approval, or is denied.
//!
//! Pure rules only; persisting "allow always" rules per workspace and asking
//! the user are done elsewhere (`ApprovalPort`, session store, project
//! permissions file).
//!
//! Rules have a text notation, used in `permissions.local.toml` and shown
//! in the UI, in the style of Claude Code:
//!
//! | Notation                      | Matcher                                 |
//! |-------------------------------|-----------------------------------------|
//! | `run_command(cargo test *)`   | [`RuleMatcher::Command`]: argv prefix   |
//! | `run_command(cargo test -p ui)` | [`RuleMatcher::Exact`]: this command  |
//! | `edit_file(src/ui/*)`         | [`RuleMatcher::Prefix`]: path prefix    |
//! | `mcp__server__tool`           | [`RuleMatcher::Any`]: any input         |
//!
//! [`rule_ladder`] builds the options the approval dialog offers, from the
//! exact action to broader patterns.

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Permission mode, cycled by the user with `<leader>p`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    /// Ask for approval before writing or executing.
    #[default]
    Ask,
    /// Edit without asking; executing still asks.
    AutoEdit,
    /// Read-only: writing and executing are denied.
    Plan,
}

impl PermissionMode {
    /// All modes, in cycling order.
    pub const ALL: [Self; 3] = [Self::Ask, Self::AutoEdit, Self::Plan];

    /// Next mode in the cycle `ask → auto-edit → plan → ask`.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Ask => Self::AutoEdit,
            Self::AutoEdit => Self::Plan,
            Self::Plan => Self::Ask,
        }
    }

    /// Stable name, as shown in the status bar and used in config.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::AutoEdit => "auto-edit",
            Self::Plan => "plan",
        }
    }
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error parsing a [`PermissionMode`] from text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown permission mode `{0}` (expected ask, auto-edit or plan)")]
pub struct ParsePermissionModeError(pub String);

impl FromStr for PermissionMode {
    type Err = ParsePermissionModeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str() == s)
            .ok_or_else(|| ParsePermissionModeError(s.to_owned()))
    }
}

/// The kind of side effect a tool has; every tool declares one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ToolEffect {
    /// Reads the workspace or repo (`read_file`, `grep`, `git log`...).
    Read,
    /// Creates or modifies files (`edit_file`, `write_file`).
    Write,
    /// Runs a process (`run_command`).
    Execute,
    /// Talks to the network (web fetch, remote MCP servers).
    Network,
}

/// A concrete action a tool wants to perform, as shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ActionRequest {
    /// Tool name, e.g. `edit_file`.
    pub tool: String,
    /// The tool's declared effect.
    pub effect: ToolEffect,
    /// Exact path or command the action applies to.
    pub subject: String,
    /// The program and its arguments, unquoted, when the action runs a
    /// command (`run_command`). [`RuleMatcher::Command`] compares its
    /// tokens with these; without them it falls back to the `subject`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
}

impl ActionRequest {
    /// Builds a request for `tool` with `effect` on `subject`.
    #[must_use]
    pub fn new(tool: impl Into<String>, effect: ToolEffect, subject: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            effect,
            subject: subject.into(),
            argv: None,
        }
    }

    /// Returns the same request with the command's `argv` (program first).
    #[must_use]
    pub fn with_argv(mut self, argv: Vec<String>) -> Self {
        self.argv = Some(argv);
        self
    }
}

/// Why an action was denied.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DenyReason {
    /// The policy is in plan (read-only) mode.
    PlanMode,
    /// The policy is read-only (subagents).
    ReadOnly,
    /// A user rule denies it.
    Rule {
        /// The rule that matched.
        rule: PermissionRule,
    },
}

impl fmt::Display for DenyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlanMode => f.write_str("plan mode is read-only"),
            Self::ReadOnly => f.write_str("this agent is read-only"),
            Self::Rule { rule } => write!(f, "denied by the rule `{rule}`"),
        }
    }
}

/// Outcome of evaluating an [`ActionRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "decision", rename_all = "kebab-case")]
pub enum Decision {
    /// Run without asking.
    Allow,
    /// Ask the user, showing the diff or exact command.
    Ask,
    /// Refuse; the reason is reported back to the model.
    Deny {
        /// Why it was denied.
        reason: DenyReason,
    },
}

/// Tool name in a [`PermissionRule`] that matches every tool.
pub const ANY_TOOL: &str = "*";

/// Which subjects (paths or commands) a rule covers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum RuleMatcher {
    /// Exactly this path or command.
    Exact(String),
    /// This path or command and anything below or after it, on a word or
    /// path boundary: `cargo test` covers `cargo test -p domain` but not
    /// `cargo testx`; `src` and `src/` cover `src/main.rs` but not `srcx`.
    Prefix(String),
    /// Commands whose argv starts with these tokens, compared token by
    /// token: `["cargo", "test"]` covers `cargo test -p ui` but not
    /// `cargo testx`. Requests without an argv are compared by their
    /// subject, as a [`Prefix`](Self::Prefix) of the quoted tokens. An
    /// empty list covers nothing.
    Command(Vec<String>),
    /// Every subject.
    Any,
}

impl RuleMatcher {
    /// Whether `subject` is covered by this matcher.
    #[must_use]
    pub fn matches(&self, subject: &str) -> bool {
        match self {
            Self::Exact(exact) => subject == exact,
            Self::Prefix(prefix) => prefix_matches(prefix, subject),
            Self::Command(tokens) => {
                !tokens.is_empty() && prefix_matches(&command_line(tokens), subject)
            }
            Self::Any => true,
        }
    }

    /// Whether `request` is covered: by its argv for
    /// [`Command`](Self::Command) when it has one, by its subject otherwise.
    #[must_use]
    pub fn matches_request(&self, request: &ActionRequest) -> bool {
        match (self, &request.argv) {
            (Self::Command(tokens), Some(argv)) => !tokens.is_empty() && argv.starts_with(tokens),
            _ => self.matches(&request.subject),
        }
    }

    /// The text between the parentheses of the rule notation:
    /// `cargo test *`, `cargo test -p ui`, `src/ui/*`; `None` for
    /// [`Any`](Self::Any), which has no parentheses.
    #[must_use]
    pub fn pattern(&self) -> Option<String> {
        match self {
            Self::Exact(exact) => Some(exact.clone()),
            Self::Prefix(prefix) => Some(format!("{prefix}*")),
            Self::Command(tokens) => Some(format!("{} *", command_line(tokens))),
            Self::Any => None,
        }
    }

    /// Parses the text between the parentheses of the rule notation (see
    /// [`pattern`](Self::pattern)): a trailing ` *` makes a
    /// [`Command`](Self::Command) of the shell-split tokens before it, a
    /// trailing `*` a [`Prefix`](Self::Prefix), a lone `*`
    /// [`Any`](Self::Any), and anything else an [`Exact`](Self::Exact).
    ///
    /// # Errors
    ///
    /// When the pattern is empty or its quotes are unbalanced.
    pub fn from_pattern(pattern: &str) -> Result<Self, String> {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return Err("the pattern is empty".to_owned());
        }
        if pattern == "*" {
            return Ok(Self::Any);
        }
        if let Some(command) = pattern.strip_suffix(" *") {
            let tokens = split_command(command)?;
            if tokens.is_empty() {
                return Err("no command before ` *`".to_owned());
            }
            return Ok(Self::Command(tokens));
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            return Ok(Self::Prefix(prefix.to_owned()));
        }
        Ok(Self::Exact(pattern.to_owned()))
    }
}

/// Quotes one argument the way commands are shown: unchanged when it is
/// made of letters, digits and `-_./=:,+@%`, else in single quotes
/// (`'my crate'`, `'it'\''s'`).
#[must_use]
pub fn quote_arg(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c));
    if plain {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// The command line of `argv`, each token quoted by [`quote_arg`] and
/// joined by spaces; this is the `subject` of a `run_command` request.
#[must_use]
pub fn command_line<S: AsRef<str>>(argv: &[S]) -> String {
    argv.iter()
        .map(|arg| quote_arg(arg.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Splits a command line into tokens like a POSIX shell, without
/// expanding anything: whitespace separates tokens, single quotes keep
/// everything literally, double quotes and backslashes escape. The inverse
/// of [`command_line`].
///
/// # Errors
///
/// When a quote is not closed.
pub fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            '\'' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => return Err("a single quote is not closed".to_owned()),
                    }
                }
            }
            '"' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => current.push(c),
                            Some(c) => {
                                current.push('\\');
                                current.push(c);
                            }
                            None => return Err("a double quote is not closed".to_owned()),
                        },
                        Some(c) => current.push(c),
                        None => return Err("a double quote is not closed".to_owned()),
                    }
                }
            }
            '\\' => {
                in_token = true;
                if let Some(c) = chars.next() {
                    current.push(c);
                }
            }
            c => {
                in_token = true;
                current.push(c);
            }
        }
    }
    if in_token {
        tokens.push(current);
    }
    Ok(tokens)
}

const fn is_boundary(c: char) -> bool {
    matches!(c, ' ' | '/')
}

fn prefix_matches(prefix: &str, subject: &str) -> bool {
    let Some(rest) = subject.strip_prefix(prefix) else {
        return false;
    };
    rest.is_empty() || prefix.ends_with(is_boundary) || rest.starts_with(is_boundary)
}

/// What a matching rule does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RuleDecision {
    /// "Allow always": turns an `Ask` into `Allow`.
    AllowAlways,
    /// Always deny, whatever the mode.
    Deny,
}

/// Where a rule lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RuleScope {
    /// Only this session; saved with it and back when it is resumed.
    #[default]
    Session,
    /// Every session of the workspace; saved in its
    /// `.crest/permissions.local.toml`.
    Project,
}

/// A user rule.
///
/// Its [`Display`](fmt::Display) and [`FromStr`] use the notation of the
/// module docs (`run_command(cargo test *)`); neither the decision nor the
/// scope are part of it: they come from where the rule is written.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PermissionRule {
    /// Tool name the rule applies to, or [`ANY_TOOL`].
    pub tool: String,
    /// Subjects covered.
    pub matcher: RuleMatcher,
    /// Effect of the rule.
    pub decision: RuleDecision,
    /// Where the rule lives; rules stored before scopes existed are
    /// session rules.
    #[serde(default)]
    pub scope: RuleScope,
}

impl PermissionRule {
    /// A session rule for `tool` on subjects covered by `matcher`.
    #[must_use]
    pub fn new(tool: impl Into<String>, matcher: RuleMatcher, decision: RuleDecision) -> Self {
        Self {
            tool: tool.into(),
            matcher,
            decision,
            scope: RuleScope::Session,
        }
    }

    /// Returns the same rule living in `scope`.
    #[must_use]
    pub fn with_scope(mut self, scope: RuleScope) -> Self {
        self.scope = scope;
        self
    }

    /// Returns the same rule with `decision`.
    #[must_use]
    pub fn with_decision(mut self, decision: RuleDecision) -> Self {
        self.decision = decision;
        self
    }

    /// Parses the notation `text` into a rule with `decision` in `scope`.
    ///
    /// # Errors
    ///
    /// When `text` is not a rule (see [`ParseRuleError`]).
    pub fn parse(
        text: &str,
        decision: RuleDecision,
        scope: RuleScope,
    ) -> Result<Self, ParseRuleError> {
        let error = |reason: &str| ParseRuleError {
            text: text.to_owned(),
            reason: reason.to_owned(),
        };
        let trimmed = text.trim();
        let (tool, matcher) = match trimmed.split_once('(') {
            Some((tool, rest)) => {
                let inner = rest
                    .strip_suffix(')')
                    .ok_or_else(|| error("it must end with `)`"))?;
                (
                    tool.trim(),
                    RuleMatcher::from_pattern(inner).map_err(|reason| error(&reason))?,
                )
            }
            None => (trimmed, RuleMatcher::Any),
        };
        if tool.is_empty() {
            return Err(error("the tool name is missing"));
        }
        if tool.contains(|c: char| c.is_whitespace() || c == '(' || c == ')') {
            return Err(error("the tool name has spaces or parentheses"));
        }
        Ok(Self {
            tool: tool.to_owned(),
            matcher,
            decision,
            scope,
        })
    }

    /// Whether the rule covers `request`.
    #[must_use]
    pub fn applies_to(&self, request: &ActionRequest) -> bool {
        (self.tool == ANY_TOOL || self.tool == request.tool)
            && self.matcher.matches_request(request)
    }

    /// Whether the rule has the same tool and matcher as `other`, whatever
    /// their decisions and scopes.
    #[must_use]
    pub fn same_pattern(&self, other: &Self) -> bool {
        self.tool == other.tool && self.matcher == other.matcher
    }
}

impl fmt::Display for PermissionRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.matcher.pattern() {
            Some(pattern) => write!(f, "{}({pattern})", self.tool),
            None => f.write_str(&self.tool),
        }
    }
}

/// Error parsing a [`PermissionRule`] from its notation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid permission rule `{text}`: {reason}")]
pub struct ParseRuleError {
    /// The text that was parsed.
    pub text: String,
    /// What is wrong with it.
    pub reason: String,
}

impl FromStr for PermissionRule {
    type Err = ParseRuleError;

    /// Parses an "allow always" session rule; see [`PermissionRule::parse`]
    /// for other decisions and scopes.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s, RuleDecision::AllowAlways, RuleScope::Session)
    }
}

/// Name of the tool that runs commands; its ladder is built from the argv.
pub const RUN_COMMAND_TOOL: &str = "run_command";

/// Tools whose subject is a workspace path; their ladder widens to the
/// file's folder and the folder above it.
pub const PATH_TOOLS: &[&str] = &["edit_file", "write_file"];

/// Programs for which the approval dialog only offers the exact command,
/// never a pattern: shells and wrappers run whatever their arguments say,
/// so `bash *` or `sudo *` would allow anything. Compared with the base
/// name of the program, without `.exe`.
pub const EXACT_ONLY_PROGRAMS: &[&str] = &[
    // Shells.
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "csh",
    "tcsh",
    "nu",
    "pwsh",
    "powershell",
    "cmd",
    // Wrappers that run another command.
    "env",
    "sudo",
    "doas",
    "xargs",
    "nohup",
    "time",
    "nice",
    "timeout",
    "stdbuf",
    "ionice",
    // Run any package from the registry.
    "npx",
    "bunx",
];

/// Interpreters and package runners that only get the exact command when
/// one of these arguments is present, because it runs code or a package
/// given right there (`python -c …`, `node -e …`, `deno eval …`,
/// `pnpm dlx …`). Names starting with `python` (`python3.12`) count as
/// `python`.
pub const EXACT_ONLY_WHEN: &[(&str, &[&str])] = &[
    ("python", &["-c"]),
    ("node", &["-e", "--eval", "-p", "--print"]),
    ("perl", &["-e", "-E"]),
    ("ruby", &["-e"]),
    ("php", &["-r"]),
    ("lua", &["-e"]),
    ("deno", &["eval"]),
    ("bun", &["-e", "--eval", "x"]),
    ("npm", &["exec"]),
    ("pnpm", &["dlx", "exec"]),
    ("yarn", &["dlx"]),
];

/// The name a program is compared by in [`EXACT_ONLY_PROGRAMS`] and
/// [`EXACT_ONLY_WHEN`]: its base name, lowercase, without `.exe`.
fn program_name(program: &str) -> String {
    let base = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let base = base.to_ascii_lowercase();
    let base = base.strip_suffix(".exe").unwrap_or(&base).to_owned();
    if base.starts_with("python") {
        "python".to_owned()
    } else {
        base
    }
}

/// Whether the approval dialog may only offer the exact command for
/// `argv` (see [`EXACT_ONLY_PROGRAMS`] and [`EXACT_ONLY_WHEN`]).
#[must_use]
pub fn is_exact_only(argv: &[String]) -> bool {
    let Some((program, args)) = argv.split_first() else {
        return true;
    };
    let name = program_name(program);
    EXACT_ONLY_PROGRAMS.contains(&name.as_str())
        || EXACT_ONLY_WHEN.iter().any(|(interpreter, flags)| {
            *interpreter == name && args.iter().any(|arg| flags.contains(&arg.as_str()))
        })
}

/// How broad a [`RuleOption`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RungKind {
    /// Exactly this command, path or input.
    Exact,
    /// The program and its subcommand: `cargo test *`.
    Subcommand,
    /// Any use of the program: `cargo *`.
    Program,
    /// Anything in the file's folder: `src/ui/*`.
    Folder,
    /// Anything in the folder above: `src/*`.
    ParentFolder,
    /// Any input of the tool.
    AnyInput,
}

/// One rung of the ladder the approval dialog offers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuleOption {
    /// The "allow always" rule it grants, as a session rule; the caller
    /// changes the scope with [`PermissionRule::with_scope`].
    pub rule: PermissionRule,
    /// How broad it is.
    pub kind: RungKind,
}

impl RuleOption {
    fn new(request: &ActionRequest, matcher: RuleMatcher, kind: RungKind) -> Self {
        Self {
            rule: PermissionRule::new(request.tool.clone(), matcher, RuleDecision::AllowAlways),
            kind,
        }
    }
}

/// The rules the approval dialog offers for `request`, narrowest first;
/// never empty, the first one is always the exact action.
///
/// - `run_command`: exact, program and subcommand (`cargo test *`, the
///   subcommand being the first argument not starting with `-`), program
///   (`cargo *`); only the exact one for [`is_exact_only`] commands.
/// - [`PATH_TOOLS`]: exact, the file's folder (`src/ui/*`), the folder
///   above (`src/*`).
/// - Any other tool (MCP, plugins, web): exact, any input.
///
/// Rungs that would repeat a narrower one are left out (`ls -la` gets only
/// the exact command and `ls *`). [`default_rung`] picks the one to
/// propose.
#[must_use]
pub fn rule_ladder(request: &ActionRequest) -> Vec<RuleOption> {
    let mut rungs = vec![RuleOption::new(
        request,
        RuleMatcher::Exact(request.subject.clone()),
        RungKind::Exact,
    )];
    let mut push = |matcher: RuleMatcher, kind: RungKind| {
        if !rungs.iter().any(|rung| rung.rule.matcher == matcher) {
            rungs.push(RuleOption::new(request, matcher, kind));
        }
    };
    if request.tool == RUN_COMMAND_TOOL {
        let argv = request
            .argv
            .clone()
            .or_else(|| split_command(&request.subject).ok())
            .unwrap_or_default();
        if !is_exact_only(&argv) {
            if let Some(sub) = argv.iter().skip(1).position(|arg| !arg.starts_with('-')) {
                push(
                    RuleMatcher::Command(argv[..sub + 2].to_vec()),
                    RungKind::Subcommand,
                );
            }
            push(RuleMatcher::Command(argv[..1].to_vec()), RungKind::Program);
        }
    } else if PATH_TOOLS.contains(&request.tool.as_str()) {
        let folder = request.subject.rsplit_once('/').map(|(dir, _)| dir);
        if let Some(folder) = folder.filter(|dir| !dir.is_empty()) {
            push(RuleMatcher::Prefix(format!("{folder}/")), RungKind::Folder);
            let parent = folder.rsplit_once('/').map(|(dir, _)| dir);
            if let Some(parent) = parent.filter(|dir| !dir.is_empty()) {
                push(
                    RuleMatcher::Prefix(format!("{parent}/")),
                    RungKind::ParentFolder,
                );
            }
        }
    } else {
        push(RuleMatcher::Any, RungKind::AnyInput);
    }
    rungs
}

/// Index in `ladder` of the rung to propose: the program and subcommand,
/// or the file's folder, when there is one; the exact action otherwise.
#[must_use]
pub fn default_rung(ladder: &[RuleOption]) -> usize {
    ladder
        .iter()
        .position(|rung| matches!(rung.kind, RungKind::Subcommand | RungKind::Folder))
        .unwrap_or(0)
}

/// Decides, per action, whether to allow, ask or deny.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct PermissionPolicy {
    /// Current mode.
    pub mode: PermissionMode,
    /// User rules, in the order they were added.
    #[serde(default)]
    pub rules: Vec<PermissionRule>,
    /// Locked to reads regardless of `mode` and rules (subagents).
    #[serde(default)]
    pub read_only: bool,
}

impl PermissionPolicy {
    /// A policy in `mode`.
    #[must_use]
    pub const fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            rules: Vec::new(),
            read_only: false,
        }
    }

    /// A locked read-only policy for subagents: everything that is not a
    /// read is denied, whatever the mode or rules, and the mode cannot be
    /// cycled.
    #[must_use]
    pub const fn read_only() -> Self {
        Self {
            mode: PermissionMode::Plan,
            rules: Vec::new(),
            read_only: true,
        }
    }

    /// Adds `rule` ("allow always" or a denial), ignoring exact duplicates.
    /// Persisting it per workspace is the caller's job.
    pub fn remember(&mut self, rule: PermissionRule) {
        if !self.rules.contains(&rule) {
            self.rules.push(rule);
        }
    }

    /// Advances to the next mode (`<leader>p`) and returns it. A
    /// [`read_only`](Self::read_only) policy stays where it is.
    pub fn cycle_mode(&mut self) -> PermissionMode {
        if !self.read_only {
            self.mode = self.mode.next();
        }
        self.mode
    }

    /// Decides what to do with `request`.
    ///
    /// Priority: a matching `Deny` rule always wins (even for reads); then
    /// the mode decides, except that a matching `AllowAlways` rule turns an
    /// `Ask` into `Allow`. `AllowAlways` never lifts a mode denial.
    /// A read-only policy denies every non-read before looking at rules.
    #[must_use]
    pub fn decide(&self, request: &ActionRequest) -> Decision {
        if self.read_only && request.effect != ToolEffect::Read {
            return Decision::Deny {
                reason: DenyReason::ReadOnly,
            };
        }
        let mut matching = self.rules.iter().filter(|rule| rule.applies_to(request));
        if let Some(rule) = matching
            .clone()
            .find(|rule| rule.decision == RuleDecision::Deny)
        {
            return Decision::Deny {
                reason: DenyReason::Rule { rule: rule.clone() },
            };
        }
        match mode_decision(self.mode, request.effect) {
            Decision::Ask if matching.any(|rule| rule.decision == RuleDecision::AllowAlways) => {
                Decision::Allow
            }
            decision => decision,
        }
    }
}

/// Base decision table, before rules.
const fn mode_decision(mode: PermissionMode, effect: ToolEffect) -> Decision {
    use PermissionMode as M;
    use ToolEffect as E;
    match (effect, mode) {
        (E::Read, _) | (E::Write, M::AutoEdit) => Decision::Allow,
        (E::Write | E::Execute | E::Network, M::Ask) | (E::Execute | E::Network, M::AutoEdit) => {
            Decision::Ask
        }
        (E::Write | E::Execute | E::Network, M::Plan) => Decision::Deny {
            reason: DenyReason::PlanMode,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_next_cycles_through_all_modes() {
        let cases = [
            (PermissionMode::Ask, PermissionMode::AutoEdit),
            (PermissionMode::AutoEdit, PermissionMode::Plan),
            (PermissionMode::Plan, PermissionMode::Ask),
        ];
        for (mode, expected) in cases {
            assert_eq!(mode.next(), expected, "{mode}.next()");
        }
    }

    #[test]
    fn mode_display_from_str_and_serde_agree() {
        let cases = [
            (PermissionMode::Ask, "ask"),
            (PermissionMode::AutoEdit, "auto-edit"),
            (PermissionMode::Plan, "plan"),
        ];
        for (mode, text) in cases {
            assert_eq!(mode.to_string(), text);
            assert_eq!(text.parse::<PermissionMode>(), Ok(mode));
            assert_eq!(serde_json::to_value(mode).unwrap(), serde_json::json!(text));
        }
    }

    #[test]
    fn mode_from_str_rejects_unknown_names() {
        for text in ["", "Ask", "auto_edit", "readonly"] {
            assert_eq!(
                text.parse::<PermissionMode>(),
                Err(ParsePermissionModeError(text.to_owned())),
                "{text:?}"
            );
        }
    }

    #[test]
    fn default_mode_is_ask() {
        assert_eq!(PermissionMode::default(), PermissionMode::Ask);
    }

    const PLAN_DENY: Decision = Decision::Deny {
        reason: DenyReason::PlanMode,
    };

    #[test]
    fn base_decision_table_by_mode_and_effect() {
        use Decision::{Allow, Ask};
        use PermissionMode as M;
        use ToolEffect as E;
        let cases = [
            (M::Ask, E::Read, Allow),
            (M::Ask, E::Write, Ask),
            (M::Ask, E::Execute, Ask),
            (M::Ask, E::Network, Ask),
            (M::AutoEdit, E::Read, Allow),
            (M::AutoEdit, E::Write, Allow),
            (M::AutoEdit, E::Execute, Ask),
            (M::AutoEdit, E::Network, Ask),
            (M::Plan, E::Read, Allow),
            (M::Plan, E::Write, PLAN_DENY),
            (M::Plan, E::Execute, PLAN_DENY),
            (M::Plan, E::Network, PLAN_DENY),
        ];
        for (mode, effect, expected) in cases {
            let request = ActionRequest::new("tool", effect, "subject");
            assert_eq!(
                PermissionPolicy::new(mode).decide(&request),
                expected,
                "{mode} / {effect:?}"
            );
        }
    }

    #[test]
    fn cycle_mode_changes_decisions() {
        let mut policy = PermissionPolicy::default();
        let write = ActionRequest::new("edit_file", ToolEffect::Write, "src/main.rs");
        let expected = [Decision::Allow, PLAN_DENY, Decision::Ask];
        for decision in expected {
            policy.cycle_mode();
            assert_eq!(policy.decide(&write), decision, "{}", policy.mode);
        }
    }

    #[test]
    fn matcher_table() {
        use RuleMatcher::{Any, Exact, Prefix};
        let p = |s: &str| Prefix(s.to_owned());
        let e = |s: &str| Exact(s.to_owned());
        let cases = [
            (e("cargo test"), "cargo test", true),
            (e("cargo test"), "cargo test -p domain", false),
            (e("src/main.rs"), "src/main.rs", true),
            (e("src/main.rs"), "src/main.rs.bak", false),
            (p("cargo test"), "cargo test", true),
            (p("cargo test"), "cargo test -p domain", true),
            (p("cargo test"), "cargo testx", false),
            (p("cargo test"), "cargo tes", false),
            (p("cargo "), "cargo build", true),
            (p("src/"), "src/main.rs", true),
            (p("src/"), "src/a/b.rs", true),
            (p("src"), "src/main.rs", true),
            (p("src"), "srcx/main.rs", false),
            (p("src/"), "src", false),
            (p(""), "anything", false),
            (Any, "anything", true),
            (Any, "", true),
        ];
        for (matcher, subject, expected) in cases {
            assert_eq!(
                matcher.matches(subject),
                expected,
                "{matcher:?} vs {subject:?}"
            );
        }
    }

    #[test]
    fn rule_applies_by_tool_and_subject() {
        let rule = |tool: &str| {
            PermissionRule::new(tool, RuleMatcher::Prefix("src/".into()), RuleDecision::Deny)
        };
        let cases = [
            (rule("edit_file"), "edit_file", "src/lib.rs", true),
            (rule("edit_file"), "write_file", "src/lib.rs", false),
            (rule("edit_file"), "edit_file", "docs/a.md", false),
            (rule(ANY_TOOL), "write_file", "src/lib.rs", true),
            (rule(ANY_TOOL), "write_file", "docs/a.md", false),
        ];
        for (rule, tool, subject, expected) in cases {
            let request = ActionRequest::new(tool, ToolEffect::Write, subject);
            assert_eq!(
                rule.applies_to(&request),
                expected,
                "{rule:?} vs {request:?}"
            );
        }
    }

    #[test]
    fn rule_serde_round_trip() {
        let rule = PermissionRule::new(
            "run_command",
            RuleMatcher::Prefix("cargo test".into()),
            RuleDecision::AllowAlways,
        );
        let json = serde_json::to_value(&rule).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "tool": "run_command",
                "matcher": { "kind": "prefix", "value": "cargo test" },
                "decision": "allow-always",
                "scope": "session",
            })
        );
        assert_eq!(
            serde_json::from_value::<PermissionRule>(json).unwrap(),
            rule
        );
        let any = serde_json::to_value(RuleMatcher::Any).unwrap();
        assert_eq!(any, serde_json::json!({ "kind": "any" }));
        assert_eq!(
            serde_json::from_value::<RuleMatcher>(any).unwrap(),
            RuleMatcher::Any
        );
    }

    #[test]
    fn rule_priority_table() {
        use Decision::{Allow, Ask};
        use PermissionMode as M;
        use ToolEffect as E;
        let allow_cargo_test = PermissionRule::new(
            "run_command",
            RuleMatcher::Prefix("cargo test".into()),
            RuleDecision::AllowAlways,
        );
        let deny_rm = PermissionRule::new(
            "run_command",
            RuleMatcher::Prefix("rm".into()),
            RuleDecision::Deny,
        );
        let allow_all_commands =
            PermissionRule::new("run_command", RuleMatcher::Any, RuleDecision::AllowAlways);
        let allow_src = PermissionRule::new(
            ANY_TOOL,
            RuleMatcher::Prefix("src/".into()),
            RuleDecision::AllowAlways,
        );
        let deny_env = PermissionRule::new(
            ANY_TOOL,
            RuleMatcher::Exact(".env".into()),
            RuleDecision::Deny,
        );
        let deny = |rule: &PermissionRule| Decision::Deny {
            reason: DenyReason::Rule { rule: rule.clone() },
        };
        let rules = vec![
            allow_cargo_test.clone(),
            allow_all_commands.clone(),
            deny_rm.clone(),
            allow_src,
            deny_env.clone(),
        ];

        let cases = [
            // AllowAlways turns Ask into Allow.
            (
                M::Ask,
                "run_command",
                E::Execute,
                "cargo test -p domain",
                Allow,
            ),
            (M::AutoEdit, "run_command", E::Execute, "cargo test", Allow),
            (M::Ask, "edit_file", E::Write, "src/lib.rs", Allow),
            // No matching rule: the mode decides.
            (M::Ask, "edit_file", E::Write, "docs/guia.md", Ask),
            (M::AutoEdit, "edit_file", E::Write, "docs/guia.md", Allow),
            (M::Ask, "web_fetch", E::Network, "https://example.com", Ask),
            // Deny wins over a matching AllowAlways, in every mode.
            (
                M::Ask,
                "run_command",
                E::Execute,
                "rm -rf target",
                deny(&deny_rm),
            ),
            (
                M::AutoEdit,
                "run_command",
                E::Execute,
                "rm -rf target",
                deny(&deny_rm),
            ),
            (
                M::Plan,
                "run_command",
                E::Execute,
                "rm -rf target",
                deny(&deny_rm),
            ),
            // Deny also applies to reads.
            (M::Ask, "read_file", E::Read, ".env", deny(&deny_env)),
            (M::Ask, "read_file", E::Read, "src/lib.rs", Allow),
            // AllowAlways never lifts the plan-mode denial.
            (M::Plan, "run_command", E::Execute, "cargo test", PLAN_DENY),
            (M::Plan, "edit_file", E::Write, "src/lib.rs", PLAN_DENY),
        ];
        for (mode, tool, effect, subject, expected) in cases {
            let policy = PermissionPolicy {
                mode,
                rules: rules.clone(),
                read_only: false,
            };
            let request = ActionRequest::new(tool, effect, subject);
            assert_eq!(policy.decide(&request), expected, "{mode} / {request:?}");
        }
    }

    #[test]
    fn remember_allow_always_skips_future_asks() {
        let mut policy = PermissionPolicy::new(PermissionMode::Ask);
        let request = ActionRequest::new("run_command", ToolEffect::Execute, "cargo test");
        assert_eq!(policy.decide(&request), Decision::Ask);

        let rule = PermissionRule::new(
            "run_command",
            RuleMatcher::Exact("cargo test".into()),
            RuleDecision::AllowAlways,
        );
        policy.remember(rule.clone());
        policy.remember(rule);
        assert_eq!(policy.rules.len(), 1, "duplicates are ignored");
        assert_eq!(policy.decide(&request), Decision::Allow);
        let other = ActionRequest::new("run_command", ToolEffect::Execute, "cargo build");
        assert_eq!(policy.decide(&other), Decision::Ask);
    }

    #[test]
    fn policy_deserializes_without_rules() {
        let policy: PermissionPolicy =
            serde_json::from_value(serde_json::json!({ "mode": "auto-edit" })).unwrap();
        assert_eq!(policy, PermissionPolicy::new(PermissionMode::AutoEdit));
    }

    #[test]
    fn read_only_policy_table() {
        use PermissionMode as M;
        use ToolEffect as E;
        const RO_DENY: Decision = Decision::Deny {
            reason: DenyReason::ReadOnly,
        };
        let allow_everything =
            PermissionRule::new(ANY_TOOL, RuleMatcher::Any, RuleDecision::AllowAlways);
        let deny_env = PermissionRule::new(
            ANY_TOOL,
            RuleMatcher::Exact(".env".into()),
            RuleDecision::Deny,
        );
        let cases = [
            (E::Read, "src/lib.rs", Decision::Allow),
            (
                E::Read,
                ".env",
                Decision::Deny {
                    reason: DenyReason::Rule {
                        rule: deny_env.clone(),
                    },
                },
            ),
            (E::Write, "src/lib.rs", RO_DENY),
            (E::Execute, "cargo test", RO_DENY),
            (E::Network, "https://example.com", RO_DENY),
        ];
        for mode in M::ALL {
            let mut policy = PermissionPolicy::read_only();
            policy.mode = mode;
            policy.remember(allow_everything.clone());
            policy.remember(deny_env.clone());
            for (effect, subject, expected) in &cases {
                let request = ActionRequest::new("tool", *effect, *subject);
                assert_eq!(&policy.decide(&request), expected, "{mode} / {request:?}");
            }
        }
    }

    #[test]
    fn read_only_policy_cannot_cycle_mode() {
        let mut policy = PermissionPolicy::read_only();
        assert_eq!(policy.cycle_mode(), PermissionMode::Plan);
        assert_eq!(policy, PermissionPolicy::read_only());
    }
    fn argv(line: &str) -> Vec<String> {
        split_command(line).unwrap()
    }

    fn command(line: &str) -> ActionRequest {
        ActionRequest::new(RUN_COMMAND_TOOL, ToolEffect::Execute, line).with_argv(argv(line))
    }

    #[test]
    fn command_matcher_compares_tokens() {
        let cmd = |line: &str| RuleMatcher::Command(argv(line));
        let cases = [
            (cmd("cargo test"), command("cargo test"), true),
            (cmd("cargo test"), command("cargo test -p ui"), true),
            (cmd("cargo test"), command("cargo testx"), false),
            (cmd("cargo test"), command("cargo"), false),
            (cmd("cargo"), command("cargo build --release"), true),
            (cmd("ls"), command("lsof -i"), false),
            (
                cmd("cargo test -p 'my crate'"),
                command("cargo test -p 'my crate' --lib"),
                true,
            ),
            (
                cmd("cargo test -p my"),
                command("cargo test -p 'my crate'"),
                false,
            ),
            (RuleMatcher::Command(Vec::new()), command("cargo"), false),
            // Without an argv the quoted subject is compared.
            (
                cmd("cargo test"),
                ActionRequest::new(RUN_COMMAND_TOOL, ToolEffect::Execute, "cargo test -p ui"),
                true,
            ),
            (
                cmd("cargo test"),
                ActionRequest::new(RUN_COMMAND_TOOL, ToolEffect::Execute, "cargo testx"),
                false,
            ),
            (
                cmd("cargo test -p 'my crate'"),
                ActionRequest::new(
                    RUN_COMMAND_TOOL,
                    ToolEffect::Execute,
                    "cargo test -p 'my crate'",
                ),
                true,
            ),
            // Exact compares the subject, argv or not.
            (
                RuleMatcher::Exact("cargo test".into()),
                command("cargo test"),
                true,
            ),
            (
                RuleMatcher::Exact("cargo test".into()),
                command("cargo test -p ui"),
                false,
            ),
        ];
        for (matcher, request, expected) in cases {
            assert_eq!(
                matcher.matches_request(&request),
                expected,
                "{matcher:?} vs {request:?}"
            );
        }
    }

    #[test]
    fn command_line_and_split_command_are_inverse() {
        let cases: [&[&str]; 5] = [
            &["cargo", "test"],
            &["cargo", "test", "-p", "my crate"],
            &["echo", "it's", ""],
            &["grep", "-e", "a\"b", "c\\d"],
            &["printf", "%s\n", "$HOME"],
        ];
        for tokens in cases {
            let line = command_line(tokens);
            assert_eq!(split_command(&line).unwrap(), tokens, "{line}");
        }
        assert_eq!(
            command_line(&["cargo", "test", "-p", "my crate"]),
            "cargo test -p 'my crate'"
        );
        assert_eq!(argv(r#"a "b c" d\ e"#), ["a", "b c", "d e"]);
        assert!(split_command("echo 'open").is_err());
        assert!(split_command("echo \"open").is_err());
    }

    #[test]
    fn notation_round_trips() {
        let rule = |tool: &str, matcher: RuleMatcher| {
            PermissionRule::new(tool, matcher, RuleDecision::AllowAlways)
        };
        let cases = [
            (
                "run_command(cargo test *)",
                rule("run_command", RuleMatcher::Command(argv("cargo test"))),
            ),
            (
                "run_command(cargo test -p ui)",
                rule("run_command", RuleMatcher::Exact("cargo test -p ui".into())),
            ),
            (
                "run_command(cargo test -p 'my crate' *)",
                rule(
                    "run_command",
                    RuleMatcher::Command(argv("cargo test -p 'my crate'")),
                ),
            ),
            (
                "edit_file(src/ui/*)",
                rule("edit_file", RuleMatcher::Prefix("src/ui/".into())),
            ),
            (
                "edit_file(src/main.rs)",
                rule("edit_file", RuleMatcher::Exact("src/main.rs".into())),
            ),
            (
                "mcp__server__tool",
                rule("mcp__server__tool", RuleMatcher::Any),
            ),
            ("*(.env)", rule(ANY_TOOL, RuleMatcher::Exact(".env".into()))),
        ];
        for (text, expected) in cases {
            assert_eq!(
                text.parse::<PermissionRule>().as_ref(),
                Ok(&expected),
                "{text}"
            );
            assert_eq!(expected.to_string(), text);
        }
        // Lenient forms.
        assert_eq!(
            " run_command( cargo  test * ) "
                .parse::<PermissionRule>()
                .unwrap(),
            rule("run_command", RuleMatcher::Command(argv("cargo test")))
        );
        assert_eq!(
            "web_fetch(*)".parse::<PermissionRule>().unwrap(),
            rule("web_fetch", RuleMatcher::Any)
        );
        let parsed = PermissionRule::parse(
            "run_command(git push *)",
            RuleDecision::Deny,
            RuleScope::Project,
        )
        .unwrap();
        assert_eq!(parsed.decision, RuleDecision::Deny);
        assert_eq!(parsed.scope, RuleScope::Project);
    }

    #[test]
    fn notation_rejects_malformed_rules() {
        for text in [
            "",
            "run_command(",
            "run_command(cargo test",
            "run_command()",
            "(cargo test)",
            "run command",
            "run_command(echo 'open *)",
        ] {
            assert!(text.parse::<PermissionRule>().is_err(), "{text:?}");
        }
        let err = "run_command(".parse::<PermissionRule>().unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid permission rule `run_command(`: it must end with `)`"
        );
    }

    #[test]
    fn rules_stored_without_scope_are_session_rules() {
        let rule: PermissionRule = serde_json::from_value(serde_json::json!({
            "tool": "run_command",
            "matcher": { "kind": "exact", "value": "cargo test" },
            "decision": "allow-always",
        }))
        .unwrap();
        assert_eq!(rule.scope, RuleScope::Session);
        let command = PermissionRule::new(
            "run_command",
            RuleMatcher::Command(argv("cargo test")),
            RuleDecision::AllowAlways,
        )
        .with_scope(RuleScope::Project);
        let json = serde_json::to_value(&command).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "tool": "run_command",
                "matcher": { "kind": "command", "value": ["cargo", "test"] },
                "decision": "allow-always",
                "scope": "project",
            })
        );
        assert_eq!(
            serde_json::from_value::<PermissionRule>(json).unwrap(),
            command
        );
    }

    #[test]
    fn requests_without_argv_deserialize() {
        let request: ActionRequest = serde_json::from_value(serde_json::json!({
            "tool": "run_command",
            "effect": "execute",
            "subject": "ls",
        }))
        .unwrap();
        assert_eq!(request.argv, None);
    }

    #[test]
    fn project_deny_beats_session_allow() {
        let mut policy = PermissionPolicy::new(PermissionMode::Ask);
        policy.remember(PermissionRule::new(
            "run_command",
            RuleMatcher::Command(argv("git")),
            RuleDecision::AllowAlways,
        ));
        let deny = PermissionRule::parse(
            "run_command(git push *)",
            RuleDecision::Deny,
            RuleScope::Project,
        )
        .unwrap();
        policy.remember(deny.clone());
        assert_eq!(
            policy.decide(&command("git push origin main")),
            Decision::Deny {
                reason: DenyReason::Rule { rule: deny.clone() }
            }
        );
        assert_eq!(policy.decide(&command("git fetch")), Decision::Allow);
        assert_eq!(
            DenyReason::Rule { rule: deny }.to_string(),
            "denied by the rule `run_command(git push *)`"
        );
    }

    fn ladder(request: &ActionRequest) -> (Vec<String>, usize) {
        let rungs = rule_ladder(request);
        let texts = rungs.iter().map(|rung| rung.rule.to_string()).collect();
        (texts, default_rung(&rungs))
    }

    #[test]
    fn ladder_table() {
        let other =
            |tool: &str, subject: &str| ActionRequest::new(tool, ToolEffect::Write, subject);
        let cases: [(ActionRequest, &[&str], usize); 15] = [
            (
                command("cargo test -p ui"),
                &[
                    "run_command(cargo test -p ui)",
                    "run_command(cargo test *)",
                    "run_command(cargo *)",
                ],
                1,
            ),
            (
                command("cargo build"),
                &[
                    "run_command(cargo build)",
                    "run_command(cargo build *)",
                    "run_command(cargo *)",
                ],
                1,
            ),
            (
                command("ls -la"),
                &["run_command(ls -la)", "run_command(ls *)"],
                0,
            ),
            (command("ls"), &["run_command(ls)", "run_command(ls *)"], 0),
            (
                command("git -C sub status"),
                &[
                    "run_command(git -C sub status)",
                    "run_command(git -C sub *)",
                    "run_command(git *)",
                ],
                1,
            ),
            (
                command("sh -c 'rm -rf /'"),
                &["run_command(sh -c 'rm -rf /')"],
                0,
            ),
            (
                command("/usr/bin/sudo make install"),
                &["run_command(/usr/bin/sudo make install)"],
                0,
            ),
            (
                command("python3 -c 'print(1)'"),
                &["run_command(python3 -c 'print(1)')"],
                0,
            ),
            (
                command("python3 script.py"),
                &[
                    "run_command(python3 script.py)",
                    "run_command(python3 script.py *)",
                    "run_command(python3 *)",
                ],
                1,
            ),
            (command("npx foo"), &["run_command(npx foo)"], 0),
            (command("pnpm dlx foo"), &["run_command(pnpm dlx foo)"], 0),
            (
                // No argv: split from the subject.
                ActionRequest::new(
                    RUN_COMMAND_TOOL,
                    ToolEffect::Execute,
                    "cargo test -p 'my crate'",
                ),
                &[
                    "run_command(cargo test -p 'my crate')",
                    "run_command(cargo test *)",
                    "run_command(cargo *)",
                ],
                1,
            ),
            (
                other("edit_file", "src/ui/approval.rs"),
                &[
                    "edit_file(src/ui/approval.rs)",
                    "edit_file(src/ui/*)",
                    "edit_file(src/*)",
                ],
                1,
            ),
            (
                other("write_file", "Cargo.toml"),
                &["write_file(Cargo.toml)"],
                0,
            ),
            (
                other("mcp__gh__create_issue", r#"{"title":"x"}"#),
                &[
                    r#"mcp__gh__create_issue({"title":"x"})"#,
                    "mcp__gh__create_issue",
                ],
                0,
            ),
        ];
        for (request, expected, default) in cases {
            let (texts, index) = ladder(&request);
            assert_eq!(texts, expected, "{request:?}");
            assert_eq!(index, default, "{request:?}");
        }
    }

    #[test]
    fn ladder_rungs_cover_the_request() {
        for request in [
            command("cargo test -p ui"),
            command("ls -la"),
            ActionRequest::new("edit_file", ToolEffect::Write, "src/ui/approval.rs"),
            ActionRequest::new("mcp__a__b", ToolEffect::Execute, "{}"),
        ] {
            for rung in rule_ladder(&request) {
                assert!(
                    rung.rule.applies_to(&request),
                    "{} vs {request:?}",
                    rung.rule
                );
                assert_eq!(rung.rule.decision, RuleDecision::AllowAlways);
                assert_eq!(rung.rule.scope, RuleScope::Session);
            }
        }
    }

    #[test]
    fn exact_only_table() {
        let cases = [
            ("bash script.sh", true),
            ("zsh", true),
            ("env FOO=1 cargo test", true),
            ("xargs rm", true),
            ("node -e 1", true),
            ("node server.js", false),
            ("perl -E 1", true),
            ("deno eval 1", true),
            ("deno run main.ts", false),
            ("bunx cowsay", true),
            ("npm exec foo", true),
            ("npm test", false),
            ("cargo test", false),
            ("", true),
        ];
        for (line, expected) in cases {
            assert_eq!(is_exact_only(&argv(line)), expected, "{line}");
        }
        let windows = ["C:\\Tools\\PWSH.EXE".to_owned(), "-Command".to_owned()];
        assert!(is_exact_only(&windows));
    }
}
