//! Checks the expectations of a scenario (and the model's invariants)
//! against a recorded [`Trace`], by replaying its snapshots.
//!
//! - `always` / invariants: true at the start and after every event;
//! - `never`: false at the start and after every event;
//! - `eventually … within`: true at some point up to the deadline;
//! - `count`: how many messages of a type a component handled, ignored,
//!   rejected or dropped, between `min` and `max`;
//! - `path`: some injected message and the messages it caused visited
//!   exactly these components, in order.
//!
//! A failure carries a counterexample: the event after which it failed,
//! its time and the state of every component then.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::expr::{self, Expr};
use super::model::{CountEvent, Expectation, Scenario, SysModel};
use super::sim::{self, GlobalEnv};
use super::trace::{EventKind, Snapshot, Trace};

/// Where a checked property comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "index", rename_all = "snake_case")]
pub enum Origin {
    /// `model.invariants[i]`.
    Invariant(usize),
    /// `scenario.expect[i]`.
    Expect(usize),
    /// The run itself (a model error stopped it).
    Run,
}

/// The state that shows a failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Counterexample {
    /// Events applied when it failed (0: at the start).
    pub event: usize,
    /// Time.
    pub at_us: u64,
    /// State of every component.
    pub snapshot: Snapshot,
}

/// The result of one property.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Verdict {
    /// Where it comes from.
    pub origin: Origin,
    /// What was checked, e.g. `always api.inflight <= 4`.
    pub property: String,
    /// Whether it held.
    pub passed: bool,
    /// Why it failed (or a short note when it passed).
    pub detail: String,
    /// Where it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<Counterexample>,
}

/// One scenario run and its verdicts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScenarioRun {
    /// Scenario id.
    pub scenario: String,
    /// Whether every verdict passed.
    pub passed: bool,
    /// Verdicts: invariants first, then the expectations in order.
    pub verdicts: Vec<Verdict>,
    /// The recorded run.
    pub trace: Trace,
}

impl ScenarioRun {
    /// The failed verdicts.
    pub fn failures(&self) -> impl Iterator<Item = &Verdict> {
        self.verdicts.iter().filter(|v| !v.passed)
    }
}

fn counterexample(trace: &Trace, event: usize) -> Counterexample {
    let at_us = event
        .checked_sub(1)
        .and_then(|i| trace.events.get(i))
        .map_or(0, |e| e.at_us);
    Counterexample {
        event,
        at_us,
        snapshot: trace.snapshot_after(event),
    }
}

fn fail(origin: Origin, property: String, detail: String, cx: Option<Counterexample>) -> Verdict {
    Verdict {
        origin,
        property,
        passed: false,
        detail,
        counterexample: cx,
    }
}

fn pass(origin: Origin, property: String, detail: String) -> Verdict {
    Verdict {
        origin,
        property,
        passed: true,
        detail,
        counterexample: None,
    }
}

fn ms_text(us: u64) -> String {
    if us.is_multiple_of(1000) {
        format!("{} ms", us / 1000)
    } else {
        format!("{:.3} ms", us as f64 / 1000.0)
    }
}

/// Evaluates `expr` at the start and after every event up to `limit`
/// events; returns the first event count where `stop(value)` is true, or
/// an evaluation error with its event count.
fn scan(
    trace: &Trace,
    expr: &Expr,
    limit: usize,
    stop: impl Fn(bool) -> bool,
) -> Result<Option<usize>, (usize, String)> {
    let mut snap = trace.initial.clone();
    let eval = |snap: &Snapshot, now_us: u64| {
        expr.eval_bool(&GlobalEnv {
            snapshot: snap,
            now_us,
        })
    };
    match eval(&snap, 0) {
        Ok(v) if stop(v) => return Ok(Some(0)),
        Ok(_) => {}
        Err(e) => return Err((0, e.to_string())),
    }
    for (i, event) in trace.events.iter().take(limit).enumerate() {
        snap.apply(event);
        match eval(&snap, event.at_us) {
            Ok(v) if stop(v) => return Ok(Some(i + 1)),
            Ok(_) => {}
            Err(e) => return Err((i + 1, e.to_string())),
        }
    }
    Ok(None)
}

fn check_state(origin: Origin, property: String, text: &str, want: bool, trace: &Trace) -> Verdict {
    let expr = match expr::parse(text) {
        Ok(e) => e,
        Err(e) => return fail(origin, property, e.to_string(), None),
    };
    match scan(trace, &expr, usize::MAX, |v| v != want) {
        Ok(None) => pass(
            origin,
            property,
            format!("held over {} events", trace.events.len()),
        ),
        Ok(Some(n)) => {
            let cx = counterexample(trace, n);
            let what = n.checked_sub(1).map_or_else(
                || "at the start".to_owned(),
                |i| format!("after event #{n} ({})", trace.events[i].kind.describe()),
            );
            let detail = format!(
                "{} {what} at {}",
                if want { "false" } else { "true" },
                ms_text(cx.at_us)
            );
            fail(origin, property, detail, Some(cx))
        }
        Err((n, e)) => {
            let cx = counterexample(trace, n);
            fail(origin, property, format!("cannot evaluate: {e}"), Some(cx))
        }
    }
}

fn check_eventually(
    origin: Origin,
    property: String,
    text: &str,
    within_ms: u64,
    trace: &Trace,
) -> Verdict {
    let expr = match expr::parse(text) {
        Ok(e) => e,
        Err(e) => return fail(origin, property, e.to_string(), None),
    };
    let deadline = within_ms.saturating_mul(1000);
    let limit = trace.events_until(deadline);
    match scan(trace, &expr, limit, |v| v) {
        Ok(Some(n)) => {
            let at = n.checked_sub(1).map_or(0, |i| trace.events[i].at_us);
            pass(origin, property, format!("true at {}", ms_text(at)))
        }
        Ok(None) => {
            let cx = counterexample(trace, limit);
            fail(
                origin,
                property,
                format!("still false at the deadline ({})", ms_text(deadline)),
                Some(Counterexample {
                    at_us: deadline.min(trace.until_us),
                    ..cx
                }),
            )
        }
        Err((n, e)) => {
            let cx = counterexample(trace, n);
            fail(origin, property, format!("cannot evaluate: {e}"), Some(cx))
        }
    }
}

fn counted(kind: &EventKind, msg_kind: &str, at: &str, event: CountEvent) -> bool {
    match (kind, event) {
        (EventKind::Handled { kind, comp, .. }, CountEvent::Handled)
        | (EventKind::Unhandled { kind, comp, .. }, CountEvent::Unhandled)
        | (EventKind::Rejected { kind, comp, .. }, CountEvent::Rejected)
        | (EventKind::Dropped { kind, comp, .. }, CountEvent::Dropped) => {
            kind == msg_kind && comp == at
        }
        _ => false,
    }
}

/// The components each flow visited, in order: where it was injected on
/// an `out` port, then every component that handled one of its messages.
#[must_use]
pub fn flow_paths(trace: &Trace) -> BTreeMap<u32, Vec<(String, usize)>> {
    let mut paths: BTreeMap<u32, Vec<(String, usize)>> = BTreeMap::new();
    for (i, event) in trace.events.iter().enumerate() {
        match &event.kind {
            EventKind::Sent { flow, from, .. } => {
                let path = paths.entry(*flow).or_default();
                if path.is_empty() {
                    path.push((from.clone(), i));
                }
            }
            EventKind::Handled { flow, comp, .. } => {
                paths.entry(*flow).or_default().push((comp.clone(), i));
            }
            _ => {}
        }
    }
    paths
}

fn check_path(origin: Origin, property: String, want: &[String], trace: &Trace) -> Verdict {
    let paths = flow_paths(trace);
    let mut best: Option<(usize, u32)> = None;
    for (flow, path) in &paths {
        let names: Vec<&str> = path.iter().map(|(c, _)| c.as_str()).collect();
        if names == want {
            return pass(origin, property, format!("followed by flow {flow}"));
        }
        let common = names
            .iter()
            .zip(want)
            .take_while(|(a, b)| **a == b.as_str())
            .count();
        if best.is_none_or(|(c, _)| common > c) {
            best = Some((common, *flow));
        }
    }
    let Some((common, flow)) = best else {
        return fail(origin, property, "no message moved".into(), None);
    };
    let path = &paths[&flow];
    let seen: Vec<&str> = path.iter().map(|(c, _)| c.as_str()).collect();
    let event = path
        .get(common)
        .or_else(|| path.last())
        .map_or(0, |(_, i)| i + 1);
    fail(
        origin,
        property,
        format!(
            "no flow followed it; the closest (flow {flow}) went {}",
            seen.join(" → ")
        ),
        Some(counterexample(trace, event)),
    )
}

/// Checks the model's invariants and `scenario.expect` against `trace`.
#[must_use]
pub fn check(model: &SysModel, scenario: &Scenario, trace: &Trace) -> Vec<Verdict> {
    let mut out = Vec::new();
    if let Some(err) = &trace.error {
        out.push(fail(
            Origin::Run,
            "the run completes".into(),
            format!("stopped at {}: {}", ms_text(err.at_us), err.message),
            Some(counterexample(trace, err.event)),
        ));
    }
    for (i, inv) in model.invariants.iter().enumerate() {
        out.push(check_state(
            Origin::Invariant(i),
            format!("invariant {inv}"),
            inv,
            true,
            trace,
        ));
    }
    for (i, expect) in scenario.expect.iter().enumerate() {
        let origin = Origin::Expect(i);
        let property = expect.describe();
        out.push(match expect {
            Expectation::Always(e) => check_state(origin, property, &e.always, true, trace),
            Expectation::Never(e) => check_state(origin, property, &e.never, false, trace),
            Expectation::Eventually(e) => {
                check_eventually(origin, property, &e.eventually, e.within_ms, trace)
            }
            Expectation::Count(e) => {
                let n = trace
                    .events
                    .iter()
                    .filter(|ev| counted(&ev.kind, &e.count, &e.at, e.event))
                    .count() as u64;
                let low = e.min.is_some_and(|min| n < min);
                let high = e.max.is_some_and(|max| n > max);
                if low || high {
                    fail(
                        origin,
                        property,
                        format!("counted {n}"),
                        Some(counterexample(trace, trace.events.len())),
                    )
                } else {
                    pass(origin, property, format!("counted {n}"))
                }
            }
            Expectation::Path(e) => check_path(origin, property, &e.path, trace),
        });
    }
    out
}

/// Simulates `scenario` and checks it.
///
/// # Errors
///
/// [`sim::SimError`] when the model cannot run.
pub fn run_scenario(model: &SysModel, scenario: &Scenario) -> Result<ScenarioRun, sim::SimError> {
    run_scenario_with_seed(model, scenario, scenario.seed)
}

/// Simulates `scenario` with `seed` and checks it.
///
/// # Errors
///
/// [`sim::SimError`] when the model cannot run.
pub fn run_scenario_with_seed(
    model: &SysModel,
    scenario: &Scenario,
    seed: u64,
) -> Result<ScenarioRun, sim::SimError> {
    let trace = sim::simulate_with_seed(model, scenario, seed)?;
    let verdicts = check(model, scenario, &trace);
    Ok(ScenarioRun {
        scenario: scenario.id.clone(),
        passed: verdicts.iter().all(|v| v.passed),
        verdicts,
        trace,
    })
}

/// Runs every scenario of `model`, in order.
///
/// # Errors
///
/// [`sim::SimError`] when the model cannot run.
pub fn run_all(model: &SysModel) -> Result<Vec<ScenarioRun>, sim::SimError> {
    model
        .scenarios
        .iter()
        .map(|s| run_scenario(model, s))
        .collect()
}
