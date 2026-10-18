//! The deterministic discrete-event simulator.
//!
//! One thread, a virtual clock in integer microseconds and an event queue
//! ordered by `(time, seq)`: two events at the same instant run in the
//! order they were scheduled. Random latencies come from a SplitMix64
//! generator seeded by the scenario, so the same model and seed always
//! give the same trace, byte for byte.
//!
//! Semantics (v1): a message that reaches a component starts being handled
//! if fewer than `capacity.concurrency` are in progress, waits in the FIFO
//! queue if there is room, and is rejected or dropped otherwise. Handling
//! takes the component's latency; then the first transition whose `from`,
//! `on` and guard match fires: its effects run in order and the machine
//! enters `to`. Emitted messages leave after `after_ms` and travel each
//! connection of the port for its latency. `port: "self"` delivers to the
//! same component (a timer). A killed component loses its queue and work
//! in progress and drops what arrives until it restarts.

use std::collections::BTreeMap;

use super::expr::{self, Env, Expr, Value};
use super::model::{Capacity, Effect, Latency, OnFull, PortDir, Scenario, SysModel, split_ref};
use super::trace::{
    CHECKPOINT_EVERY, Checkpoint, CompState, DropReason, EventKind, RunError, Snapshot, Trace,
    TraceEvent, VarChange,
};

/// Most events a run records before it stops (`truncated`).
pub const MAX_EVENTS: usize = 100_000;

/// Why a model cannot run at all (lint reports the same problems first).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the model cannot run: {0}")]
pub struct SimError(pub String);

/// SplitMix64: small, fast and fully specified, so other implementations
/// (the HTML export) could reproduce it.
#[derive(Debug, Clone)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    /// A generator seeded with `seed`.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `lo..=hi`.
    pub fn between(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        let span = hi - lo + 1;
        lo + self.next_u64() % span
    }
}

#[derive(Debug, Clone)]
struct Msg {
    id: u32,
    kind: String,
    flow: u32,
}

#[derive(Debug)]
enum Pending {
    Deliver { msg: Msg, to: usize },
    SendOut { msg: Msg, from: usize, port: String },
    Complete { comp: usize, msg: Msg, epoch: u32 },
    Kill { comp: usize },
    Restart { comp: usize },
}

/// Connections leaving each `(component, out port)`.
type Routes = BTreeMap<(usize, String), Vec<Route>>;

#[derive(Debug)]
struct Route {
    conn: String,
    to: usize,
    latency_us: u64,
}

#[derive(Debug)]
struct CTransition {
    from: Option<String>,
    on: String,
    guard: Option<Expr>,
    to: Option<String>,
    effects: Vec<CEffect>,
}

#[derive(Debug)]
enum CEffect {
    Set(String, Expr),
    Emit {
        kind: String,
        port: String,
        after_us: u64,
    },
}

#[derive(Debug)]
struct CComp {
    id: String,
    initial: String,
    vars: BTreeMap<String, i64>,
    capacity: Option<Capacity>,
    latency: Option<Latency>,
    transitions: Vec<CTransition>,
}

const fn ms(v: u64) -> u64 {
    v.saturating_mul(1000)
}

fn parse_expr(text: &str, at: &str) -> Result<Expr, SimError> {
    expr::parse(text).map_err(|e| SimError(format!("{at}: {e}")))
}

fn compile(model: &SysModel) -> Result<(Vec<CComp>, Routes), SimError> {
    let index: BTreeMap<&str, usize> = model
        .components
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.as_str(), i))
        .collect();
    let mut comps = Vec::with_capacity(model.components.len());
    for c in &model.components {
        let mut transitions = Vec::new();
        for (i, t) in c.machine.transitions.iter().enumerate() {
            let at = format!("{}.machine.transitions[{i}]", c.id);
            let guard = t
                .guard
                .as_deref()
                .map(|g| parse_expr(g, &format!("{at}.guard")))
                .transpose()?;
            let mut effects = Vec::new();
            for e in &t.effects {
                effects.push(match e {
                    Effect::Set(s) => CEffect::Set(s.set.clone(), parse_expr(&s.expr, &at)?),
                    Effect::Emit(e) => CEffect::Emit {
                        kind: e.emit.clone(),
                        port: e.port.clone(),
                        after_us: ms(e.after_ms),
                    },
                });
            }
            transitions.push(CTransition {
                from: (t.from != "*").then(|| t.from.clone()),
                on: t.on.clone(),
                guard,
                to: t.to.clone(),
                effects,
            });
        }
        comps.push(CComp {
            id: c.id.clone(),
            initial: c.machine.initial.clone(),
            vars: c.vars.clone(),
            capacity: c.capacity,
            latency: c.latency,
            transitions,
        });
    }
    let mut routes: Routes = BTreeMap::new();
    for conn in &model.connections {
        let resolve = |r: &str| {
            let (comp, port) = split_ref(r).ok_or_else(|| {
                SimError(format!(
                    "connection {}: `{r}` is not component.port",
                    conn.id
                ))
            })?;
            let ix = *index.get(comp).ok_or_else(|| {
                SimError(format!("connection {}: no component `{comp}`", conn.id))
            })?;
            Ok::<_, SimError>((ix, port.to_owned()))
        };
        let (from, port) = resolve(&conn.from)?;
        let (to, _) = resolve(&conn.to)?;
        routes.entry((from, port)).or_default().push(Route {
            conn: conn.id.clone(),
            to,
            latency_us: ms(conn.latency_ms),
        });
    }
    Ok((comps, routes))
}

/// The state of every component before anything happens.
#[must_use]
pub fn initial_snapshot(model: &SysModel) -> Snapshot {
    Snapshot {
        components: model
            .components
            .iter()
            .map(|c| {
                (
                    c.id.clone(),
                    CompState {
                        state: c.machine.initial.clone(),
                        vars: c.vars.clone(),
                        queue: Vec::new(),
                        busy: 0,
                        up: true,
                    },
                )
            })
            .collect(),
    }
}

/// Names a guard or effect of component `comp` reads: its own variables,
/// `state`, `queue`, `busy`, `up` and `now` (milliseconds).
struct LocalEnv<'a> {
    comp: &'a str,
    state: &'a CompState,
    vars: &'a BTreeMap<String, i64>,
    now_us: u64,
}

impl Env for LocalEnv<'_> {
    fn get(&self, scope: Option<&str>, name: &str) -> Option<Value> {
        if scope.is_some_and(|s| s != self.comp) {
            return None;
        }
        builtin(self.state, self.vars, name, self.now_us, scope.is_none())
    }
}

fn builtin(
    state: &CompState,
    vars: &BTreeMap<String, i64>,
    name: &str,
    now_us: u64,
    allow_now: bool,
) -> Option<Value> {
    match name {
        "state" => Some(Value::Str(state.state.clone())),
        "queue" => Some(Value::Int(
            i64::try_from(state.queue.len()).unwrap_or(i64::MAX),
        )),
        "busy" => Some(Value::Int(i64::from(state.busy))),
        "up" => Some(Value::Bool(state.up)),
        "now" if allow_now => Some(Value::Int(i64::try_from(now_us / 1000).unwrap_or(i64::MAX))),
        var => vars.get(var).copied().map(Value::Int),
    }
}

/// Names an assertion reads: `component.var`, `component.state`,
/// `component.queue`, `component.busy`, `component.up` and `now`.
pub struct GlobalEnv<'a> {
    /// State of every component.
    pub snapshot: &'a Snapshot,
    /// Current time.
    pub now_us: u64,
}

impl Env for GlobalEnv<'_> {
    fn get(&self, scope: Option<&str>, name: &str) -> Option<Value> {
        match scope {
            None if name == "now" => Some(Value::Int(
                i64::try_from(self.now_us / 1000).unwrap_or(i64::MAX),
            )),
            None => None,
            Some(comp) => {
                let state = self.snapshot.components.get(comp)?;
                builtin(state, &state.vars, name, self.now_us, false)
            }
        }
    }
}

struct Engine<'a> {
    model: &'a SysModel,
    comps: Vec<CComp>,
    routes: Routes,
    queue: BTreeMap<(u64, u64), Pending>,
    seq: u64,
    now: u64,
    rng: SplitMix64,
    snapshot: Snapshot,
    epochs: Vec<u32>,
    next_msg: u32,
    events: Vec<TraceEvent>,
    checkpoints: Vec<Checkpoint>,
}

impl Engine<'_> {
    fn schedule(&mut self, at: u64, pending: Pending) {
        self.queue.insert((at, self.seq), pending);
        self.seq += 1;
    }

    fn record(&mut self, kind: EventKind) {
        let event = TraceEvent {
            at_us: self.now,
            kind,
        };
        self.snapshot.apply(&event);
        self.events.push(event);
        if self.events.len().is_multiple_of(CHECKPOINT_EVERY) {
            self.checkpoints.push(Checkpoint {
                event: self.events.len(),
                at_us: self.now,
                snapshot: self.snapshot.clone(),
            });
        }
    }

    fn msg(&mut self, kind: &str, flow: u32) -> Msg {
        let id = self.next_msg;
        self.next_msg += 1;
        Msg {
            id,
            kind: kind.to_owned(),
            flow,
        }
    }

    fn state(&self, comp: usize) -> Option<&CompState> {
        self.snapshot.components.get(&self.comps[comp].id)
    }

    fn latency(&mut self, comp: usize) -> u64 {
        match self.comps[comp].latency {
            None => 0,
            Some(Latency::Fixed { ms: v }) => ms(v),
            Some(Latency::Uniform { ms: [lo, hi] }) => {
                let (lo, hi) = (ms(lo.min(hi)), ms(hi.max(lo)));
                self.rng.between(lo, hi)
            }
        }
    }

    fn start(&mut self, comp: usize, msg: Msg, from_queue: bool) {
        let done = self.now + self.latency(comp);
        self.record(EventKind::Started {
            msg: msg.id,
            kind: msg.kind.clone(),
            flow: msg.flow,
            comp: self.comps[comp].id.clone(),
            from_queue,
            done_us: done,
        });
        let epoch = self.epochs[comp];
        self.schedule(done, Pending::Complete { comp, msg, epoch });
    }

    fn deliver(&mut self, comp: usize, msg: Msg) {
        let Some(state) = self.state(comp) else {
            return;
        };
        let id = self.comps[comp].id.clone();
        if !state.up {
            self.record(EventKind::Dropped {
                msg: msg.id,
                kind: msg.kind,
                flow: msg.flow,
                comp: id,
                reason: DropReason::Down,
            });
            return;
        }
        let (busy, waiting) = (state.busy, state.queue.len());
        match self.comps[comp].capacity {
            None => self.start(comp, msg, false),
            Some(cap) if busy < cap.concurrency.max(1) => self.start(comp, msg, false),
            Some(cap) if waiting < cap.queue as usize => self.record(EventKind::Enqueued {
                msg: msg.id,
                kind: msg.kind,
                flow: msg.flow,
                comp: id,
            }),
            Some(Capacity {
                on_full: OnFull::Reject,
                ..
            }) => self.record(EventKind::Rejected {
                msg: msg.id,
                kind: msg.kind,
                flow: msg.flow,
                comp: id,
            }),
            Some(_) => self.record(EventKind::Dropped {
                msg: msg.id,
                kind: msg.kind,
                flow: msg.flow,
                comp: id,
                reason: DropReason::Full,
            }),
        }
    }

    fn send_out(&mut self, from: usize, port: &str, msg: &Msg) {
        if self.state(from).is_some_and(|s| !s.up) {
            return;
        }
        let routes: Vec<(String, usize, u64)> = self
            .routes
            .get(&(from, port.to_owned()))
            .map(|r| {
                r.iter()
                    .map(|r| (r.conn.clone(), r.to, r.latency_us))
                    .collect()
            })
            .unwrap_or_default();
        if routes.is_empty() {
            self.record(EventKind::Dropped {
                msg: msg.id,
                kind: msg.kind.clone(),
                flow: msg.flow,
                comp: self.comps[from].id.clone(),
                reason: DropReason::Unrouted,
            });
            return;
        }
        for (i, (conn, to, latency)) in routes.into_iter().enumerate() {
            // Fan-out: every extra copy is a new message of the same flow.
            let copy = if i == 0 {
                msg.clone()
            } else {
                self.msg(&msg.kind, msg.flow)
            };
            let arrive = self.now + latency;
            self.record(EventKind::Sent {
                msg: copy.id,
                kind: copy.kind.clone(),
                flow: copy.flow,
                from: self.comps[from].id.clone(),
                port: port.to_owned(),
                conn,
                to: self.comps[to].id.clone(),
                arrive_us: arrive,
            });
            self.schedule(arrive, Pending::Deliver { msg: copy, to });
        }
    }

    fn complete(&mut self, comp: usize, msg: Msg) -> Result<(), String> {
        let Some(state) = self.state(comp).cloned() else {
            return Ok(());
        };
        let id = self.comps[comp].id.clone();
        let mut fired = None;
        for (ix, t) in self.comps[comp].transitions.iter().enumerate() {
            if t.on != msg.kind || t.from.as_ref().is_some_and(|f| *f != state.state) {
                continue;
            }
            let ok = match &t.guard {
                None => true,
                Some(g) => g
                    .eval_bool(&LocalEnv {
                        comp: &id,
                        state: &state,
                        vars: &state.vars,
                        now_us: self.now,
                    })
                    .map_err(|e| format!("guard of {id}.machine.transitions[{ix}]: {e}"))?,
            };
            if ok {
                fired = Some(ix);
                break;
            }
        }
        let Some(ix) = fired else {
            self.record(EventKind::Unhandled {
                msg: msg.id,
                kind: msg.kind,
                flow: msg.flow,
                comp: id,
                state: state.state,
            });
            return Ok(());
        };
        let mut vars = state.vars.clone();
        let mut changes = Vec::new();
        let mut emits = Vec::new();
        for effect in &self.comps[comp].transitions[ix].effects {
            match effect {
                CEffect::Set(var, e) => {
                    let value = e
                        .eval_int(&LocalEnv {
                            comp: &id,
                            state: &state,
                            vars: &vars,
                            now_us: self.now,
                        })
                        .map_err(|err| {
                            format!("effect `set {var}` of {id}.machine.transitions[{ix}]: {err}")
                        })?;
                    vars.insert(var.clone(), value);
                    changes.push(VarChange {
                        var: var.clone(),
                        value,
                    });
                }
                CEffect::Emit {
                    kind,
                    port,
                    after_us,
                } => emits.push((kind.clone(), port.clone(), *after_us)),
            }
        }
        let to = self.comps[comp].transitions[ix]
            .to
            .clone()
            .unwrap_or_else(|| state.state.clone());
        self.record(EventKind::Handled {
            msg: msg.id,
            kind: msg.kind,
            flow: msg.flow,
            comp: id,
            from_state: state.state,
            to_state: to,
            transition: u32::try_from(ix).unwrap_or(u32::MAX),
            changes,
        });
        for (kind, port, after) in emits {
            let out = self.msg(&kind, msg.flow);
            let at = self.now + after;
            if port == "self" {
                self.schedule(at, Pending::Deliver { msg: out, to: comp });
            } else {
                self.schedule(
                    at,
                    Pending::SendOut {
                        msg: out,
                        from: comp,
                        port,
                    },
                );
            }
        }
        Ok(())
    }

    fn after_complete(&mut self, comp: usize) {
        let Some(state) = self.state(comp) else {
            return;
        };
        if !state.up || state.queue.is_empty() {
            return;
        }
        let limit = self.comps[comp]
            .capacity
            .map_or(u32::MAX, |c| c.concurrency.max(1));
        if state.busy >= limit {
            return;
        }
        let head = state.queue[0].clone();
        let msg = Msg {
            id: head.msg,
            kind: head.kind,
            flow: head.flow,
        };
        self.start(comp, msg, true);
    }

    fn step(&mut self, pending: Pending) -> Result<(), String> {
        match pending {
            Pending::Deliver { msg, to } => self.deliver(to, msg),
            Pending::SendOut { msg, from, port } => self.send_out(from, &port, &msg),
            Pending::Complete { comp, msg, epoch } => {
                if self.epochs[comp] == epoch {
                    self.complete(comp, msg)?;
                    self.after_complete(comp);
                }
            }
            Pending::Kill { comp } => {
                let lost = self
                    .state(comp)
                    .filter(|s| s.up)
                    .map(|s| s.busy + u32::try_from(s.queue.len()).unwrap_or(u32::MAX));
                if let Some(lost) = lost {
                    self.epochs[comp] += 1;
                    self.record(EventKind::Killed {
                        comp: self.comps[comp].id.clone(),
                        lost,
                    });
                }
            }
            Pending::Restart { comp } => {
                if self.state(comp).is_some_and(|s| !s.up) {
                    self.record(EventKind::Restarted {
                        comp: self.comps[comp].id.clone(),
                        state: self.comps[comp].initial.clone(),
                        vars: self.comps[comp].vars.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// Runs `scenario` of `model` with the scenario's seed.
///
/// # Errors
///
/// [`SimError`] when an expression does not parse or a reference does not
/// resolve; errors while running (a guard that does not evaluate) stop
/// the run and are recorded in [`Trace::error`] instead.
pub fn simulate(model: &SysModel, scenario: &Scenario) -> Result<Trace, SimError> {
    simulate_with_seed(model, scenario, scenario.seed)
}

/// Runs `scenario` with another `seed`.
///
/// # Errors
///
/// As [`simulate`].
pub fn simulate_with_seed(
    model: &SysModel,
    scenario: &Scenario,
    seed: u64,
) -> Result<Trace, SimError> {
    let (comps, routes) = compile(model)?;
    let index: BTreeMap<&str, usize> = model
        .components
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.as_str(), i))
        .collect();
    let initial = initial_snapshot(model);
    let mut engine = Engine {
        model,
        epochs: vec![0; comps.len()],
        comps,
        routes,
        queue: BTreeMap::new(),
        seq: 0,
        now: 0,
        rng: SplitMix64::new(seed),
        snapshot: initial.clone(),
        next_msg: 1,
        events: Vec::new(),
        checkpoints: Vec::new(),
    };
    // Faults first: at the same instant they act before the messages.
    for (i, fault) in scenario.faults.iter().enumerate() {
        let ix = *index.get(fault.kill.as_str()).ok_or_else(|| {
            SimError(format!(
                "scenario {} faults[{i}]: no component `{}`",
                scenario.id, fault.kill
            ))
        })?;
        engine.schedule(ms(fault.at_ms), Pending::Kill { comp: ix });
        if let Some(for_ms) = fault.for_ms {
            engine.schedule(ms(fault.at_ms + for_ms), Pending::Restart { comp: ix });
        }
    }
    let mut flow = 0;
    for (i, inject) in scenario.inject.iter().enumerate() {
        let at = format!("scenario {} inject[{i}]", scenario.id);
        let (comp, port) = split_ref(&inject.to)
            .ok_or_else(|| SimError(format!("{at}: `{}` is not component.port", inject.to)))?;
        let ix = *index
            .get(comp)
            .ok_or_else(|| SimError(format!("{at}: no component `{comp}`")))?;
        let dir = engine.model.components[ix]
            .port(port)
            .map(|p| p.dir)
            .ok_or_else(|| SimError(format!("{at}: `{comp}` has no port `{port}`")))?;
        let (count, every) = inject
            .repeat
            .map_or((1, 0), |r| (r.count.max(1), ms(r.every_ms)));
        for k in 0..u64::from(count) {
            flow += 1;
            let msg = engine.msg(&inject.msg, flow);
            let when = ms(inject.at_ms) + every * k;
            let pending = match dir {
                PortDir::In => Pending::Deliver { msg, to: ix },
                PortDir::Out => Pending::SendOut {
                    msg,
                    from: ix,
                    port: port.to_owned(),
                },
            };
            engine.schedule(when, pending);
        }
    }
    let until = ms(scenario.until_ms);
    let mut truncated = false;
    let mut error = None;
    while let Some(entry) = engine.queue.first_entry() {
        let (at, _) = *entry.key();
        if at > until {
            break;
        }
        if engine.events.len() >= MAX_EVENTS {
            truncated = true;
            break;
        }
        let pending = entry.remove();
        engine.now = at;
        if let Err(message) = engine.step(pending) {
            error = Some(RunError {
                event: engine.events.len(),
                at_us: at,
                message,
            });
            break;
        }
    }
    Ok(Trace {
        scenario: scenario.id.clone(),
        seed,
        until_us: until,
        model_hash: model.hash(),
        initial,
        events: engine.events,
        checkpoints: engine.checkpoints,
        truncated,
        error,
    })
}
