//! What a simulation run recorded: every event in order, the state of every
//! component at the start, and checkpoints of the state every
//! [`CHECKPOINT_EVERY`] events so any instant can be rebuilt quickly.
//!
//! [`Snapshot::apply`] is the only place where events change state: the
//! simulator uses it while running and the checker and the board replay
//! the same function, so they cannot disagree.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Events between two checkpoints.
pub const CHECKPOINT_EVERY: usize = 256;

/// The state of one component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompState {
    /// Current state of its machine.
    pub state: String,
    /// Its variables.
    pub vars: BTreeMap<String, i64>,
    /// Messages waiting, oldest first.
    pub queue: Vec<QueuedMsg>,
    /// Messages being handled.
    pub busy: u32,
    /// Whether it is running (not killed by a fault).
    pub up: bool,
}

/// A message waiting in a queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QueuedMsg {
    /// Message id.
    pub msg: u32,
    /// Message type.
    pub kind: String,
    /// Injection it descends from.
    pub flow: u32,
}

/// The state of every component, by id.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Snapshot {
    /// Component id → state.
    pub components: BTreeMap<String, CompState>,
}

/// One variable assigned by a transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VarChange {
    /// Variable.
    pub var: String,
    /// New value.
    pub value: i64,
}

/// Why a message was lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    /// The component and its queue were full (`on_full: drop`).
    Full,
    /// The component was down.
    Down,
    /// It was emitted on a port with no connection.
    Unrouted,
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    /// A message left a port and travels a connection until `arrive_us`.
    Sent {
        /// Message id.
        msg: u32,
        /// Message type.
        kind: String,
        /// Injection it descends from.
        flow: u32,
        /// Sending component.
        from: String,
        /// Its `out` port.
        port: String,
        /// Connection id.
        conn: String,
        /// Receiving component.
        to: String,
        /// When it arrives.
        arrive_us: u64,
    },
    /// A component started handling a message; it finishes at `done_us`.
    Started {
        /// Message id.
        msg: u32,
        /// Message type.
        kind: String,
        /// Injection it descends from.
        flow: u32,
        /// Component.
        comp: String,
        /// Whether it came from the queue.
        from_queue: bool,
        /// When it finishes.
        done_us: u64,
    },
    /// A message waits in the queue of a busy component.
    Enqueued {
        /// Message id.
        msg: u32,
        /// Message type.
        kind: String,
        /// Injection it descends from.
        flow: u32,
        /// Component.
        comp: String,
    },
    /// A transition handled a message.
    Handled {
        /// Message id.
        msg: u32,
        /// Message type.
        kind: String,
        /// Injection it descends from.
        flow: u32,
        /// Component.
        comp: String,
        /// State before.
        from_state: String,
        /// State after.
        to_state: String,
        /// Index of the transition in the machine.
        transition: u32,
        /// Variables assigned, in order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        changes: Vec<VarChange>,
    },
    /// No transition matched the message in the current state.
    Unhandled {
        /// Message id.
        msg: u32,
        /// Message type.
        kind: String,
        /// Injection it descends from.
        flow: u32,
        /// Component.
        comp: String,
        /// State it was in.
        state: String,
    },
    /// A full component refused the message.
    Rejected {
        /// Message id.
        msg: u32,
        /// Message type.
        kind: String,
        /// Injection it descends from.
        flow: u32,
        /// Component.
        comp: String,
    },
    /// A message was lost.
    Dropped {
        /// Message id.
        msg: u32,
        /// Message type.
        kind: String,
        /// Injection it descends from.
        flow: u32,
        /// Component that lost it.
        comp: String,
        /// Why.
        reason: DropReason,
    },
    /// A fault killed a component; its queue and work in progress are lost.
    Killed {
        /// Component.
        comp: String,
        /// Messages lost with it.
        lost: u32,
    },
    /// A killed component came back in its initial state.
    Restarted {
        /// Component.
        comp: String,
        /// Its initial state.
        state: String,
        /// Its initial variables.
        vars: BTreeMap<String, i64>,
    },
}

impl EventKind {
    /// The component the event happened at (the sender for `Sent`).
    #[must_use]
    pub fn comp(&self) -> &str {
        match self {
            Self::Sent { from, .. } => from,
            Self::Started { comp, .. }
            | Self::Enqueued { comp, .. }
            | Self::Handled { comp, .. }
            | Self::Unhandled { comp, .. }
            | Self::Rejected { comp, .. }
            | Self::Dropped { comp, .. }
            | Self::Killed { comp, .. }
            | Self::Restarted { comp, .. } => comp,
        }
    }

    /// One line describing it.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Sent {
                kind,
                from,
                port,
                to,
                ..
            } => format!("{from}.{port} sent {kind} to {to}"),
            Self::Started { kind, comp, .. } => format!("{comp} started {kind}"),
            Self::Enqueued { kind, comp, .. } => format!("{comp} queued {kind}"),
            Self::Handled {
                kind,
                comp,
                from_state,
                to_state,
                ..
            } => format!("{comp} handled {kind}: {from_state} → {to_state}"),
            Self::Unhandled {
                kind, comp, state, ..
            } => format!("{comp} ignored {kind} in {state}"),
            Self::Rejected { kind, comp, .. } => format!("{comp} rejected {kind} (full)"),
            Self::Dropped {
                kind, comp, reason, ..
            } => format!("{comp} dropped {kind} ({reason:?})").to_lowercase(),
            Self::Killed { comp, lost } => format!("{comp} killed ({lost} messages lost)"),
            Self::Restarted { comp, state, .. } => format!("{comp} restarted in {state}"),
        }
    }
}

/// One event at an instant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TraceEvent {
    /// Simulated time, in microseconds.
    pub at_us: u64,
    /// What happened.
    #[serde(flatten)]
    pub kind: EventKind,
}

/// The state after the first `event` events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Checkpoint {
    /// Events applied.
    pub event: usize,
    /// Time of the last of them.
    pub at_us: u64,
    /// State.
    pub snapshot: Snapshot,
}

/// A run stopped by an error of the model (a guard that does not evaluate).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunError {
    /// Events recorded before the error.
    pub event: usize,
    /// When.
    pub at_us: u64,
    /// What failed.
    pub message: String,
}

/// A recorded run of one scenario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Trace {
    /// Scenario id.
    pub scenario: String,
    /// Seed used.
    pub seed: u64,
    /// Simulated time.
    pub until_us: u64,
    /// [`super::SysModel::hash`] of the model that ran.
    pub model_hash: String,
    /// State at time 0.
    pub initial: Snapshot,
    /// Events, ordered by time and then by occurrence.
    pub events: Vec<TraceEvent>,
    /// State every [`CHECKPOINT_EVERY`] events.
    #[serde(default)]
    pub checkpoints: Vec<Checkpoint>,
    /// Whether the run stopped at the event limit.
    #[serde(default)]
    pub truncated: bool,
    /// The model error that stopped the run, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RunError>,
}

impl Snapshot {
    /// Applies `event` to the state.
    pub fn apply(&mut self, event: &TraceEvent) {
        let Some(comp) = self.components.get_mut(event.kind.comp()) else {
            return;
        };
        match &event.kind {
            EventKind::Sent { .. } | EventKind::Rejected { .. } | EventKind::Dropped { .. } => {}
            EventKind::Started {
                from_queue: true, ..
            } => {
                if !comp.queue.is_empty() {
                    comp.queue.remove(0);
                }
                comp.busy += 1;
            }
            EventKind::Started { .. } => comp.busy += 1,
            EventKind::Enqueued {
                msg, kind, flow, ..
            } => comp.queue.push(QueuedMsg {
                msg: *msg,
                kind: kind.clone(),
                flow: *flow,
            }),
            EventKind::Handled {
                to_state, changes, ..
            } => {
                comp.busy = comp.busy.saturating_sub(1);
                comp.state.clone_from(to_state);
                for change in changes {
                    comp.vars.insert(change.var.clone(), change.value);
                }
            }
            EventKind::Unhandled { .. } => comp.busy = comp.busy.saturating_sub(1),
            EventKind::Killed { .. } => {
                comp.up = false;
                comp.busy = 0;
                comp.queue.clear();
            }
            EventKind::Restarted { state, vars, .. } => {
                comp.up = true;
                comp.state.clone_from(state);
                comp.vars.clone_from(vars);
            }
        }
    }
}

impl Trace {
    /// The state after the first `n` events.
    #[must_use]
    pub fn snapshot_after(&self, n: usize) -> Snapshot {
        let n = n.min(self.events.len());
        let base = self.checkpoints.iter().rev().find(|c| c.event <= n);
        let (mut snap, from) = match base {
            Some(c) => (c.snapshot.clone(), c.event),
            None => (self.initial.clone(), 0),
        };
        for event in &self.events[from..n] {
            snap.apply(event);
        }
        snap
    }

    /// Events that happened at or before `t_us`.
    #[must_use]
    pub fn events_until(&self, t_us: u64) -> usize {
        self.events.partition_point(|e| e.at_us <= t_us)
    }

    /// The state at `t_us`.
    #[must_use]
    pub fn snapshot_at(&self, t_us: u64) -> Snapshot {
        self.snapshot_after(self.events_until(t_us))
    }

    /// Time of the first event after `t_us`, if any.
    #[must_use]
    pub fn next_event_after(&self, t_us: u64) -> Option<u64> {
        self.events.get(self.events_until(t_us)).map(|e| e.at_us)
    }

    /// Time of the last event before `t_us`, if any.
    #[must_use]
    pub fn prev_event_before(&self, t_us: u64) -> Option<u64> {
        let n = self.events.partition_point(|e| e.at_us < t_us);
        n.checked_sub(1).map(|i| self.events[i].at_us)
    }

    /// Pretty JSON with a trailing newline.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).map_or_else(|_| String::new(), |s| s + "\n")
    }

    /// Stable hash of the trace, as 16 hex digits.
    #[must_use]
    pub fn hash(&self) -> String {
        super::model::fnv_hex(serde_json::to_string(self).unwrap_or_default().as_bytes())
    }
}
