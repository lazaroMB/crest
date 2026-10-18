//! The `crest.sysmodel/v1` format: components with ports, a flat state
//! machine each, connections between ports, invariants and scenarios.
//!
//! Every struct denies unknown fields and every enum is closed, so a model
//! with a typo is rejected when it is read instead of being half-ignored.
//! Durations are integer milliseconds in the file; the simulator works in
//! integer microseconds.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Version tag of the format.
pub const SYSMODEL_VERSION: &str = "crest.sysmodel/v1";

/// The only version this crate reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub enum Version {
    /// `crest.sysmodel/v1`.
    #[default]
    #[serde(rename = "crest.sysmodel/v1")]
    V1,
}

/// An executable sketch of a proposed system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SysModel {
    /// Always `crest.sysmodel/v1`.
    pub version: Version,
    /// The Spec this model sketches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_ref: Option<SpecRef>,
    /// Parts of the system (chips on the board).
    pub components: Vec<Component>,
    /// Traces between an `out` port and an `in` port.
    #[serde(default)]
    pub connections: Vec<Connection>,
    /// Expressions that must hold after every event of every scenario,
    /// e.g. `pg.conns <= 20`.
    #[serde(default)]
    pub invariants: Vec<String>,
    /// Inputs to run and what must happen.
    #[serde(default)]
    pub scenarios: Vec<Scenario>,
}

/// Where the model comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecRef {
    /// Spec folder, e.g. `0007-session-cache`.
    pub solution: String,
    /// Evidence ids of the Spec the model relies on, e.g. `E3`.
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// What a component is; picks its look on the board.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ComponentKind {
    /// A caller outside the system (user, browser, other service).
    Client,
    /// A service or process.
    Service,
    /// A message queue or log.
    Queue,
    /// A database.
    Db,
    /// A cache.
    Cache,
    /// A load balancer or router.
    Lb,
    /// A third-party system.
    External,
    /// A clock that sends messages on a schedule.
    Timer,
}

impl ComponentKind {
    /// Every kind, in declaration order.
    pub const ALL: [Self; 8] = [
        Self::Client,
        Self::Service,
        Self::Queue,
        Self::Db,
        Self::Cache,
        Self::Lb,
        Self::External,
        Self::Timer,
    ];

    /// The snake_case name used in the file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Service => "service",
            Self::Queue => "queue",
            Self::Db => "db",
            Self::Cache => "cache",
            Self::Lb => "lb",
            Self::External => "external",
            Self::Timer => "timer",
        }
    }
}

/// A part of the system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Component {
    /// Unique id, `[a-z][a-z0-9_]*`, used in expressions (`api.inflight`).
    pub id: String,
    /// What it is.
    pub kind: ComponentKind,
    /// Name shown on the board; the id when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Spec elements or evidence ids this component comes from (`E3`, the
    /// name of a node of the black-box diagram).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cites: Vec<String>,
    /// Where messages enter and leave.
    #[serde(default)]
    pub ports: Vec<Port>,
    /// Concurrency and queue limits; unlimited when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity: Option<Capacity>,
    /// Time to handle one message; zero when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<Latency>,
    /// Integer variables and their initial values.
    #[serde(default)]
    pub vars: BTreeMap<String, i64>,
    /// How it reacts to messages.
    pub machine: Machine,
    /// Grid cell hint; auto-layout otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<GridPos>,
}

impl Component {
    /// The label, or the id.
    #[must_use]
    pub fn name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.id)
    }

    /// The port `id`.
    #[must_use]
    pub fn port(&self, id: &str) -> Option<&Port> {
        self.ports.iter().find(|p| p.id == id)
    }
}

/// Direction of a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortDir {
    /// Messages arrive here.
    In,
    /// Messages leave from here.
    Out,
}

/// A port of a component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Port {
    /// Id, unique in the component. `self` is reserved.
    pub id: String,
    /// Direction.
    pub dir: PortDir,
    /// Message types an `in` port accepts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepts: Vec<String>,
    /// Message types an `out` port emits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emits: Vec<String>,
}

/// What happens to a message that finds the component and its queue full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnFull {
    /// Refused; counted as `rejected`.
    #[default]
    Reject,
    /// Lost silently; counted as `dropped`.
    Drop,
}

/// Limits of a component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capacity {
    /// Messages handled at the same time (at least 1).
    pub concurrency: u32,
    /// Messages that may wait in the FIFO queue.
    #[serde(default)]
    pub queue: u32,
    /// What happens when both are full.
    #[serde(default)]
    pub on_full: OnFull,
}

/// Time to handle one message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "dist", rename_all = "snake_case", deny_unknown_fields)]
pub enum Latency {
    /// Always `ms`.
    Fixed {
        /// Milliseconds.
        ms: u64,
    },
    /// Uniform between `ms[0]` and `ms[1]`, drawn from the scenario's seed.
    Uniform {
        /// Lowest and highest milliseconds.
        ms: [u64; 2],
    },
}

/// A flat state machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    /// Initial state.
    pub initial: String,
    /// Every state.
    pub states: Vec<String>,
    /// Transitions, tried in order; the first whose `from`, `on` and guard
    /// match fires.
    #[serde(default)]
    pub transitions: Vec<Transition>,
}

/// One transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    /// State it leaves, or `*` for any state.
    pub from: String,
    /// Message type that triggers it.
    pub on: String,
    /// Expression over the component's variables that must be true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<String>,
    /// State it enters; `from` when absent (a self-loop).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// What it does, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
}

/// One effect of a transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Effect {
    /// Assigns an integer expression to a variable.
    Set(SetEffect),
    /// Sends a message out of a port (or to itself with `port: "self"`).
    Emit(EmitEffect),
}

/// `{ "set": "inflight", "expr": "inflight + 1" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetEffect {
    /// Variable of the component.
    pub set: String,
    /// Integer expression.
    pub expr: String,
}

/// `{ "emit": "Query", "port": "db", "after_ms": 2 }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmitEffect {
    /// Message type.
    pub emit: String,
    /// `out` port it leaves from, or `self` for a timer.
    pub port: String,
    /// Delay before it leaves.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub after_ms: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if signature.
const fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// Grid cell of a component on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GridPos {
    /// Column, from 0, left to right.
    pub col: u32,
    /// Row, from 0, back to front.
    pub row: u32,
}

/// A trace between two ports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    /// Unique id.
    pub id: String,
    /// `component.port` of an `out` port.
    pub from: String,
    /// `component.port` of an `in` port.
    pub to: String,
    /// Travel time.
    #[serde(default)]
    pub latency_ms: u64,
}

/// One run of the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Unique id, e.g. `cache_miss_storm`.
    pub id: String,
    /// What it shows, in one sentence.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Seed of the random latencies.
    #[serde(default)]
    pub seed: u64,
    /// Simulated time to run.
    pub until_ms: u64,
    /// Messages put into the board.
    #[serde(default)]
    pub inject: Vec<Inject>,
    /// Components killed for a while.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub faults: Vec<Fault>,
    /// What must happen.
    #[serde(default)]
    pub expect: Vec<Expectation>,
    /// Hypotheses and test-protocol items of the Spec it covers (`H1`,
    /// `T3`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covers: Vec<String>,
}

/// A message put into the board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Inject {
    /// When.
    pub at_ms: u64,
    /// `component.port`: an `in` port receives it, an `out` port sends it.
    pub to: String,
    /// Message type.
    pub msg: String,
    /// Sends it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<Repeat>,
}

/// Repetition of an injection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Repeat {
    /// Time between two sends.
    pub every_ms: u64,
    /// Total sends, including the first.
    pub count: u32,
}

/// A component that goes down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fault {
    /// When.
    pub at_ms: u64,
    /// Component id.
    pub kill: String,
    /// How long until it restarts (initial state and variables); never
    /// when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_ms: Option<u64>,
}

/// What a scenario must show. Exactly one of the forms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Expectation {
    /// `{ "always": "api.inflight <= 4" }`.
    Always(AlwaysExpect),
    /// `{ "never": "pg.state == 'down'" }`.
    Never(NeverExpect),
    /// `{ "eventually": "client.state == 'done'", "within_ms": 200 }`.
    Eventually(EventuallyExpect),
    /// `{ "count": "Resp", "at": "client", "min": 50 }`.
    Count(CountExpect),
    /// `{ "path": ["client", "api", "db", "api", "client"] }`.
    Path(PathExpect),
}

/// See [`Expectation::Always`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AlwaysExpect {
    /// Boolean expression that holds after every event.
    pub always: String,
}

/// See [`Expectation::Never`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NeverExpect {
    /// Boolean expression that is false after every event.
    pub never: String,
}

/// See [`Expectation::Eventually`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventuallyExpect {
    /// Boolean expression that becomes true.
    pub eventually: String,
    /// Deadline from the start of the scenario.
    pub within_ms: u64,
}

/// What a count counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CountEvent {
    /// Messages a transition handled.
    #[default]
    Handled,
    /// Messages no transition matched.
    Unhandled,
    /// Messages refused by a full component (`on_full: reject`).
    Rejected,
    /// Messages lost: full with `on_full: drop`, or the component down.
    Dropped,
}

/// See [`Expectation::Count`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CountExpect {
    /// Message type.
    pub count: String,
    /// Component id.
    pub at: String,
    /// What to count.
    #[serde(default)]
    pub event: CountEvent,
    /// At least this many.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<u64>,
    /// At most this many.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u64>,
}

/// See [`Expectation::Path`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathExpect {
    /// Components one injected message and the messages it causes visit,
    /// in order, starting where it was injected.
    pub path: Vec<String>,
}

impl Expectation {
    /// One line describing it, e.g. `always api.inflight <= 4`.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Always(e) => format!("always {}", e.always),
            Self::Never(e) => format!("never {}", e.never),
            Self::Eventually(e) => format!("eventually {} within {} ms", e.eventually, e.within_ms),
            Self::Count(e) => {
                let event = match e.event {
                    CountEvent::Handled => "handled",
                    CountEvent::Unhandled => "unhandled",
                    CountEvent::Rejected => "rejected",
                    CountEvent::Dropped => "dropped",
                };
                let range = match (e.min, e.max) {
                    (Some(a), Some(b)) if a == b => format!("== {a}"),
                    (Some(a), Some(b)) => format!("in {a}..={b}"),
                    (Some(a), None) => format!(">= {a}"),
                    (None, Some(b)) => format!("<= {b}"),
                    (None, None) => "any".to_owned(),
                };
                format!("count {} {event} at {} {range}", e.count, e.at)
            }
            Self::Path(e) => format!("path {}", e.path.join(" → ")),
        }
    }
}

impl SysModel {
    /// The component `id`.
    #[must_use]
    pub fn component(&self, id: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.id == id)
    }

    /// The scenario `id`.
    #[must_use]
    pub fn scenario(&self, id: &str) -> Option<&Scenario> {
        self.scenarios.iter().find(|s| s.id == id)
    }

    /// Reads a model from JSON, rejecting unknown fields and versions.
    ///
    /// # Errors
    ///
    /// The deserializer's message (field path and what was expected).
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Reads a model from a JSON value.
    ///
    /// # Errors
    ///
    /// As [`SysModel::from_json`].
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    /// Pretty JSON with a trailing newline.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).map_or_else(|_| String::new(), |s| s + "\n")
    }

    /// Stable hash of the model (FNV-1a over its compact JSON), as 16 hex
    /// digits. Not cryptographic: it names a model version in evidence.
    #[must_use]
    pub fn hash(&self) -> String {
        fnv_hex(serde_json::to_string(self).unwrap_or_default().as_bytes())
    }
}

/// The JSON Schema of [`SysModel`].
#[must_use]
pub fn schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(SysModel)).unwrap_or_default()
}

/// FNV-1a 64 of `bytes`, as 16 hex digits.
#[must_use]
pub fn fnv_hex(bytes: &[u8]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let hash = bytes
        .iter()
        .fold(OFFSET, |h, b| (h ^ u64::from(*b)).wrapping_mul(PRIME));
    format!("{hash:016x}")
}

/// Splits `component.port` (or `component.var`) at its first dot.
#[must_use]
pub fn split_ref(reference: &str) -> Option<(&str, &str)> {
    let (a, b) = reference.split_once('.')?;
    (!a.is_empty() && !b.is_empty()).then_some((a, b))
}
