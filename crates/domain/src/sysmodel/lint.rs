//! Structured checks of a model before it runs.
//!
//! Errors make the model unusable (a reference that does not resolve, an
//! expression that does not parse or type-check); warnings point at a
//! model that runs but probably does not say what was meant (an emitted
//! message nobody handles, an unreachable state, a component that cites
//! nothing of the Spec).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::expr::{self, Ty};
use super::model::{Component, Effect, Expectation, Latency, PortDir, SysModel, split_ref};

/// How bad an issue is.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The model cannot be trusted to run.
    Error,
    /// The model runs but is probably wrong.
    Warning,
}

/// One problem of a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Issue {
    /// How bad.
    pub severity: Severity,
    /// Stable code, e.g. `unknown-port`.
    pub code: String,
    /// Where, e.g. `components[api].machine.transitions[0].guard`.
    pub at: String,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{level}[{}] {}: {}", self.code, self.at, self.message)
    }
}

/// Names a component may not use for variables: they are built in.
pub const RESERVED_VARS: [&str; 5] = ["state", "queue", "busy", "up", "now"];

struct Lint<'a> {
    model: &'a SysModel,
    issues: Vec<Issue>,
}

fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl<'a> Lint<'a> {
    fn push(&mut self, severity: Severity, code: &str, at: impl Into<String>, message: String) {
        self.issues.push(Issue {
            severity,
            code: code.to_owned(),
            at: at.into(),
            message,
        });
    }

    fn error(&mut self, code: &str, at: impl Into<String>, message: String) {
        self.push(Severity::Error, code, at, message);
    }

    fn warn(&mut self, code: &str, at: impl Into<String>, message: String) {
        self.push(Severity::Warning, code, at, message);
    }

    fn comp(&self, id: &str) -> Option<&'a Component> {
        self.model.component(id)
    }

    /// Checks `text` in component `c`'s scope and returns its type.
    fn local_expr(&mut self, c: &Component, text: &str, at: &str, want: Ty) {
        let e = match expr::parse(text) {
            Ok(e) => e,
            Err(err) => return self.error("bad-expression", at, err.to_string()),
        };
        for (scope, name) in e.names() {
            if let Some(scope) = scope
                && scope != c.id
            {
                self.error(
                    "foreign-name",
                    at,
                    format!(
                        "`{scope}.{name}`: a component only reads its own variables; send a \
                         message instead"
                    ),
                );
                return;
            }
        }
        let types = |_: Option<&str>, name: &str| local_type(c, name);
        match e.infer(&types) {
            Ok(t) if t == want => {}
            Ok(t) => self.error("type", at, format!("`{text}` is a {t}; expected a {want}")),
            Err(m) => self.error("type", at, m),
        }
    }

    /// Checks a global (assertion) expression.
    fn global_expr(&mut self, text: &str, at: &str) {
        let e = match expr::parse(text) {
            Ok(e) => e,
            Err(err) => return self.error("bad-expression", at, err.to_string()),
        };
        let model = self.model;
        let types = |scope: Option<&str>, name: &str| match scope {
            None => (name == "now").then_some(Ty::Int),
            Some(comp) => model.component(comp).and_then(|c| local_type(c, name)),
        };
        match e.infer(&types) {
            Ok(Ty::Bool) => {}
            Ok(t) => self.error("type", at, format!("`{text}` is a {t}; expected a boolean")),
            Err(m) => {
                let hint = if e.names().iter().any(|(s, n)| s.is_none() && *n != "now") {
                    " (assertions name variables as component.var)"
                } else {
                    ""
                };
                self.error("type", at, format!("{m}{hint}"));
            }
        }
    }

    fn components(&mut self) {
        let mut seen = BTreeSet::new();
        if self.model.components.is_empty() {
            self.error("empty", "components", "the model has no components".into());
        }
        for c in &self.model.components {
            let at = format!("components[{}]", c.id);
            if !seen.insert(c.id.as_str()) {
                self.error(
                    "duplicate-id",
                    &at,
                    format!("component `{}` is repeated", c.id),
                );
            }
            if !valid_id(&c.id) {
                self.error(
                    "bad-id",
                    &at,
                    format!("`{}` must match [a-z][a-z0-9_]*", c.id),
                );
            }
            if c.cites.is_empty() {
                self.warn(
                    "no-citation",
                    &at,
                    "cites no Spec element or evidence id".into(),
                );
            }
            let mut ports = BTreeSet::new();
            for p in &c.ports {
                if !ports.insert(p.id.as_str()) {
                    self.error(
                        "duplicate-id",
                        format!("{at}.ports[{}]", p.id),
                        format!("port `{}` is repeated", p.id),
                    );
                }
                if p.id == "self" {
                    self.error(
                        "reserved",
                        format!("{at}.ports[self]"),
                        "`self` is reserved for timers".into(),
                    );
                }
                match p.dir {
                    PortDir::In if !p.emits.is_empty() => self.warn(
                        "port-direction",
                        format!("{at}.ports[{}]", p.id),
                        "an `in` port lists `emits`; use `accepts`".into(),
                    ),
                    PortDir::Out if !p.accepts.is_empty() => self.warn(
                        "port-direction",
                        format!("{at}.ports[{}]", p.id),
                        "an `out` port lists `accepts`; use `emits`".into(),
                    ),
                    _ => {}
                }
            }
            for var in c.vars.keys() {
                if RESERVED_VARS.contains(&var.as_str()) {
                    self.error(
                        "reserved",
                        format!("{at}.vars.{var}"),
                        format!("`{var}` is built in; rename the variable"),
                    );
                }
            }
            if let Some(cap) = c.capacity
                && cap.concurrency == 0
            {
                self.error(
                    "capacity",
                    format!("{at}.capacity"),
                    "concurrency must be at least 1".into(),
                );
            }
            if let Some(Latency::Uniform { ms: [lo, hi] }) = c.latency
                && lo > hi
            {
                self.error(
                    "latency",
                    format!("{at}.latency"),
                    format!("the range {lo}..{hi} is reversed"),
                );
            }
            self.machine(c, &at);
        }
    }

    fn machine(&mut self, c: &Component, at: &str) {
        let m = &c.machine;
        let states: BTreeSet<&str> = m.states.iter().map(String::as_str).collect();
        if states.len() != m.states.len() {
            self.error(
                "duplicate-id",
                format!("{at}.machine.states"),
                "a state is repeated".into(),
            );
        }
        if !states.contains(m.initial.as_str()) {
            self.error(
                "unknown-state",
                format!("{at}.machine.initial"),
                format!("`{}` is not in states", m.initial),
            );
        }
        for (i, t) in m.transitions.iter().enumerate() {
            let tat = format!("{at}.machine.transitions[{i}]");
            if t.from != "*" && !states.contains(t.from.as_str()) {
                self.error(
                    "unknown-state",
                    format!("{tat}.from"),
                    format!("`{}` is not in states", t.from),
                );
            }
            if let Some(to) = &t.to
                && !states.contains(to.as_str())
            {
                self.error(
                    "unknown-state",
                    format!("{tat}.to"),
                    format!("`{to}` is not in states"),
                );
            }
            let accepted = c
                .ports
                .iter()
                .filter(|p| p.dir == PortDir::In)
                .any(|p| p.accepts.is_empty() || p.accepts.contains(&t.on));
            let self_sent = m.transitions.iter().any(|t2| {
                t2.effects
                    .iter()
                    .any(|e| matches!(e, Effect::Emit(e) if e.port == "self" && e.emit == t.on))
            });
            if !accepted && !self_sent && !injected(self.model, &c.id, &t.on) {
                self.warn(
                    "unreachable-message",
                    format!("{tat}.on"),
                    format!(
                        "no in port of `{}` accepts `{}` and nothing sends it",
                        c.id, t.on
                    ),
                );
            }
            if let Some(g) = &t.guard {
                self.local_expr(c, g, &format!("{tat}.guard"), Ty::Bool);
            }
            for (j, e) in t.effects.iter().enumerate() {
                let eat = format!("{tat}.effects[{j}]");
                match e {
                    Effect::Set(s) => {
                        if !c.vars.contains_key(&s.set) {
                            self.error(
                                "unknown-var",
                                &eat,
                                format!("`{}` is not a variable of `{}`", s.set, c.id),
                            );
                        }
                        self.local_expr(c, &s.expr, &eat, Ty::Int);
                    }
                    Effect::Emit(e) if e.port == "self" => {}
                    Effect::Emit(e) => match c.port(&e.port) {
                        None => self.error(
                            "unknown-port",
                            &eat,
                            format!("`{}` has no port `{}`", c.id, e.port),
                        ),
                        Some(p) if p.dir != PortDir::Out => self.error(
                            "port-direction",
                            &eat,
                            format!("`{}.{}` is an in port", c.id, e.port),
                        ),
                        Some(p) if !p.emits.is_empty() && !p.emits.contains(&e.emit) => self.error(
                            "message-type",
                            &eat,
                            format!("`{}.{}` does not list `{}` in emits", c.id, e.port, e.emit),
                        ),
                        Some(_) => self.handled_downstream(c, &e.port, &e.emit, &eat),
                    },
                }
            }
        }
        // Reachability from the initial state.
        let mut reached: BTreeSet<&str> = BTreeSet::from([m.initial.as_str()]);
        loop {
            let before = reached.len();
            for t in &m.transitions {
                if t.from == "*" || reached.contains(t.from.as_str()) {
                    reached.insert(t.to.as_deref().unwrap_or(&t.from));
                }
            }
            if reached.len() == before {
                break;
            }
        }
        for s in &m.states {
            if !reached.contains(s.as_str()) {
                self.warn(
                    "unreachable-state",
                    format!("{at}.machine.states"),
                    format!("`{s}` cannot be reached from `{}`", m.initial),
                );
            }
        }
    }

    /// Warns when `kind`, sent on `comp.port`, has no handler at the other
    /// end of a connection.
    fn handled_downstream(&mut self, c: &Component, port: &str, kind: &str, at: &str) {
        let from = format!("{}.{port}", c.id);
        for conn in self.model.connections.iter().filter(|k| k.from == from) {
            let Some((to, _)) = split_ref(&conn.to) else {
                continue;
            };
            let Some(target) = self.comp(to) else {
                continue;
            };
            if !target.machine.transitions.iter().any(|t| t.on == kind) {
                self.warn(
                    "unhandled-message",
                    at,
                    format!(
                        "`{to}` has no transition on `{kind}` (sent through {})",
                        conn.id
                    ),
                );
            }
        }
    }

    fn connections(&mut self) {
        let mut seen = BTreeSet::new();
        let mut connected: BTreeSet<String> = BTreeSet::new();
        for conn in &self.model.connections {
            let at = format!("connections[{}]", conn.id);
            if !seen.insert(conn.id.as_str()) {
                self.error(
                    "duplicate-id",
                    &at,
                    format!("connection `{}` is repeated", conn.id),
                );
            }
            let from = self.endpoint(&conn.from, PortDir::Out, &format!("{at}.from"));
            let to = self.endpoint(&conn.to, PortDir::In, &format!("{at}.to"));
            connected.insert(conn.from.clone());
            connected.insert(conn.to.clone());
            if let (Some(from), Some(to)) = (from, to)
                && !from.emits.is_empty()
                && !to.accepts.is_empty()
            {
                for kind in &from.emits {
                    if !to.accepts.contains(kind) {
                        self.warn(
                            "message-type",
                            &at,
                            format!(
                                "`{kind}` sent by {} is not accepted by {}",
                                conn.from, conn.to
                            ),
                        );
                    }
                }
            }
        }
        for c in &self.model.components {
            for p in &c.ports {
                let r = format!("{}.{}", c.id, p.id);
                let used = connected.contains(&r)
                    || self
                        .model
                        .scenarios
                        .iter()
                        .any(|s| s.inject.iter().any(|i| i.to == r));
                if !used {
                    self.warn(
                        "unconnected-port",
                        format!("components[{}].ports[{}]", c.id, p.id),
                        format!("`{r}` has no connection and no injection"),
                    );
                }
            }
        }
    }

    fn endpoint(&mut self, r: &str, dir: PortDir, at: &str) -> Option<&'a super::model::Port> {
        let Some((comp, port)) = split_ref(r) else {
            self.error("bad-reference", at, format!("`{r}` is not component.port"));
            return None;
        };
        let Some(c) = self.comp(comp) else {
            self.error("unknown-component", at, format!("no component `{comp}`"));
            return None;
        };
        let Some(p) = c.port(port) else {
            self.error("unknown-port", at, format!("`{comp}` has no port `{port}`"));
            return None;
        };
        if p.dir != dir {
            let want = if dir == PortDir::Out { "out" } else { "in" };
            self.error(
                "port-direction",
                at,
                format!("`{r}` must be an {want} port"),
            );
        }
        Some(p)
    }

    fn scenarios(&mut self) {
        for (i, inv) in self.model.invariants.iter().enumerate() {
            self.global_expr(inv, &format!("invariants[{i}]"));
        }
        let mut seen = BTreeSet::new();
        for s in &self.model.scenarios {
            let at = format!("scenarios[{}]", s.id);
            if !seen.insert(s.id.as_str()) {
                self.error(
                    "duplicate-id",
                    &at,
                    format!("scenario `{}` is repeated", s.id),
                );
            }
            if s.until_ms == 0 {
                self.error(
                    "duration",
                    format!("{at}.until_ms"),
                    "must be greater than 0".into(),
                );
            }
            if s.inject.is_empty() {
                self.warn(
                    "no-input",
                    format!("{at}.inject"),
                    "nothing is injected".into(),
                );
            }
            if s.expect.is_empty() {
                self.warn(
                    "no-expectation",
                    format!("{at}.expect"),
                    "checks nothing".into(),
                );
            }
            for (j, inj) in s.inject.iter().enumerate() {
                let iat = format!("{at}.inject[{j}]");
                match split_ref(&inj.to).and_then(|(c, p)| self.comp(c).map(|c| (c, p))) {
                    None => self.error(
                        "bad-reference",
                        &iat,
                        format!("`{}` is not component.port of the model", inj.to),
                    ),
                    Some((c, p)) => match c.port(p) {
                        None => self.error(
                            "unknown-port",
                            &iat,
                            format!("`{}` has no port `{p}`", c.id),
                        ),
                        Some(port) => {
                            let types = if port.dir == PortDir::In {
                                &port.accepts
                            } else {
                                &port.emits
                            };
                            if !types.is_empty() && !types.contains(&inj.msg) {
                                self.error(
                                    "message-type",
                                    &iat,
                                    format!("`{}` does not carry `{}`", inj.to, inj.msg),
                                );
                            }
                        }
                    },
                }
                if let Some(r) = inj.repeat
                    && (r.count == 0 || (r.count > 1 && r.every_ms == 0))
                {
                    self.error(
                        "repeat",
                        format!("{iat}.repeat"),
                        "count must be at least 1 and every_ms greater than 0".into(),
                    );
                }
            }
            for (j, f) in s.faults.iter().enumerate() {
                if self.comp(&f.kill).is_none() {
                    self.error(
                        "unknown-component",
                        format!("{at}.faults[{j}]"),
                        format!("no component `{}`", f.kill),
                    );
                }
            }
            for (j, e) in s.expect.iter().enumerate() {
                let eat = format!("{at}.expect[{j}]");
                match e {
                    Expectation::Always(e) => self.global_expr(&e.always, &eat),
                    Expectation::Never(e) => self.global_expr(&e.never, &eat),
                    Expectation::Eventually(e) => self.global_expr(&e.eventually, &eat),
                    Expectation::Count(e) => {
                        if self.comp(&e.at).is_none() {
                            self.error(
                                "unknown-component",
                                &eat,
                                format!("no component `{}`", e.at),
                            );
                        }
                        if let (Some(a), Some(b)) = (e.min, e.max)
                            && a > b
                        {
                            self.error("count", &eat, format!("min {a} is above max {b}"));
                        }
                    }
                    Expectation::Path(e) => {
                        if e.path.is_empty() {
                            self.error("path", &eat, "the path is empty".into());
                        }
                        for c in &e.path {
                            if self.comp(c).is_none() {
                                self.error(
                                    "unknown-component",
                                    &eat,
                                    format!("no component `{c}`"),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

fn injected(model: &SysModel, comp: &str, kind: &str) -> bool {
    model.scenarios.iter().any(|s| {
        s.inject
            .iter()
            .any(|i| i.msg == kind && split_ref(&i.to).is_some_and(|(c, _)| c == comp))
    })
}

fn local_type(c: &Component, name: &str) -> Option<Ty> {
    match name {
        "state" => Some(Ty::Str),
        "queue" | "busy" | "now" => Some(Ty::Int),
        "up" => Some(Ty::Bool),
        var => c.vars.contains_key(var).then_some(Ty::Int),
    }
}

/// Every issue of `model`, errors first, in a stable order.
#[must_use]
pub fn lint(model: &SysModel) -> Vec<Issue> {
    let mut l = Lint {
        model,
        issues: Vec::new(),
    };
    l.components();
    l.connections();
    l.scenarios();
    let mut issues = l.issues;
    issues.sort_by_key(|i| i.severity);
    issues
}

/// Whether any issue is an error.
#[must_use]
pub fn has_errors(issues: &[Issue]) -> bool {
    issues.iter().any(|i| i.severity == Severity::Error)
}

/// Counts issues by code (for summaries).
#[must_use]
pub fn by_code(issues: &[Issue]) -> BTreeMap<&str, usize> {
    let mut out = BTreeMap::new();
    for i in issues {
        *out.entry(i.code.as_str()).or_insert(0) += 1;
    }
    out
}

/// Warnings for the test-protocol (or hypothesis) `items` no scenario
/// lists in `covers`.
#[must_use]
pub fn uncovered(model: &SysModel, items: &[String]) -> Vec<Issue> {
    items
        .iter()
        .filter(|item| !model.scenarios.iter().any(|s| s.covers.contains(item)))
        .map(|item| Issue {
            severity: Severity::Warning,
            code: "uncovered".to_owned(),
            at: "scenarios".to_owned(),
            message: format!("no scenario covers `{item}` of the Spec"),
        })
        .collect()
}
