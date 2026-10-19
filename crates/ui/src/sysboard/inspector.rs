//! Text of the state inspector: a component's machine as a Mermaid state
//! diagram with the current state lit, one-line summaries, what it is
//! connected to and what it did last.

use std::fmt::Write as _;

use domain::sysmodel::model::Component;
use domain::sysmodel::scene::Scene;
use domain::sysmodel::trace::{CompState, EventKind, Trace};

use super::playback::time_label;

/// Mermaid ids are words; state names may not be.
fn node(name: &str) -> String {
    let mut id: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if id.is_empty() || id.starts_with(|c: char| c.is_ascii_digit()) {
        id.insert(0, 's');
    }
    id
}

fn label(text: &str) -> String {
    text.replace([':', '\n', '"'], " ")
}

/// `stateDiagram-v2` of `comp`'s machine with `current` lit.
#[must_use]
pub fn machine_diagram(comp: &Component, current: Option<&str>) -> String {
    let m = &comp.machine;
    let mut out = String::from("stateDiagram-v2\n");
    for s in &m.states {
        let id = node(s);
        if id != *s {
            let _ = writeln!(out, "  state \"{}\" as {id}", label(s));
        }
    }
    let _ = writeln!(out, "  [*] --> {}", node(&m.initial));
    for t in &m.transitions {
        let froms: Vec<&String> = if t.from == "*" {
            m.states.iter().collect()
        } else {
            vec![&t.from]
        };
        for from in froms {
            let to = t.to.as_deref().unwrap_or(from);
            let guard = t
                .guard
                .as_deref()
                .map(|g| format!(" [{}]", label(g)))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "  {} --> {}: {}{guard}",
                node(from),
                node(to),
                label(&t.on)
            );
        }
    }
    if let Some(current) = current.filter(|c| m.states.iter().any(|s| s == c)) {
        let _ = writeln!(
            out,
            "  style {} fill:#f59e0b,stroke:#fde68a,stroke-width:3px,color:#111",
            node(current)
        );
    }
    out
}

/// What component `id` is connected to: `→ db (query)` for a trace that
/// leaves it, `← web (request)` for one that arrives.
#[must_use]
pub fn connections(scene: &Scene, id: &str) -> Vec<String> {
    let mut out = Vec::new();
    for ribbon in &scene.ribbons {
        if ribbon.from == id {
            out.push(format!("→ {} ({})", ribbon.to, ribbon.id));
        }
        if ribbon.to == id {
            out.push(format!("← {} ({})", ribbon.from, ribbon.id));
        }
    }
    out
}

/// The last `n` things component `id` did up to `t_us`, newest first:
/// `1.200 ms · sent query → db`, `received request`, `idle → busy on
/// request`...
#[must_use]
pub fn activity(trace: &Trace, id: &str, t_us: u64, n: usize) -> Vec<String> {
    let until = trace.events_until(t_us);
    trace.events[..until]
        .iter()
        .rev()
        .filter_map(|event| {
            let what = match &event.kind {
                EventKind::Sent { kind, from, to, .. } if from == id => {
                    format!("sent {kind} → {to}")
                }
                EventKind::Sent { kind, from, to, .. } if to == id => {
                    format!("{from} sent it {kind}")
                }
                EventKind::Started { kind, comp, .. } if comp == id => {
                    format!("handling {kind}")
                }
                EventKind::Enqueued { kind, comp, .. } if comp == id => format!("queued {kind}"),
                EventKind::Handled {
                    kind,
                    comp,
                    from_state,
                    to_state,
                    ..
                } if comp == id => {
                    if from_state == to_state {
                        format!("handled {kind} in {from_state}")
                    } else {
                        format!("{from_state} → {to_state} on {kind}")
                    }
                }
                EventKind::Unhandled {
                    kind, comp, state, ..
                } if comp == id => format!("ignored {kind} in {state}"),
                EventKind::Rejected { kind, comp, .. } if comp == id => format!("rejected {kind}"),
                EventKind::Dropped {
                    kind, comp, reason, ..
                } if comp == id => format!("dropped {kind} ({reason:?})"),
                EventKind::Killed { comp, lost } if comp == id => {
                    format!("went down ({lost} lost)")
                }
                EventKind::Restarted { comp, state, .. } if comp == id => {
                    format!("restarted in {state}")
                }
                _ => return None,
            };
            Some(format!("{} · {what}", time_label(event.at_us)))
        })
        .take(n)
        .collect()
}

/// `busy · 2 in progress · 3 queued` / `down`.
#[must_use]
pub fn status_line(state: &CompState) -> String {
    if !state.up {
        return format!("{} · down", state.state);
    }
    let mut parts = vec![state.state.clone()];
    if state.busy > 0 {
        parts.push(format!("{} in progress", state.busy));
    }
    if !state.queue.is_empty() {
        parts.push(format!("{} queued", state.queue.len()));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use domain::sysmodel::model::{ComponentKind, Machine, Transition};
    use domain::sysmodel::trace::QueuedMsg;

    use super::*;

    fn comp() -> Component {
        Component {
            id: "api".into(),
            kind: ComponentKind::Service,
            label: None,
            cites: Vec::new(),
            ports: Vec::new(),
            capacity: None,
            latency: None,
            vars: BTreeMap::new(),
            machine: Machine {
                initial: "idle".into(),
                states: vec!["idle".into(), "busy".into(), "half-open".into()],
                transitions: vec![
                    Transition {
                        from: "*".into(),
                        on: "Req".into(),
                        guard: Some("inflight < 4".into()),
                        to: Some("busy".into()),
                        effects: Vec::new(),
                    },
                    Transition {
                        from: "busy".into(),
                        on: "Fail".into(),
                        guard: None,
                        to: Some("half-open".into()),
                        effects: Vec::new(),
                    },
                ],
            },
            layout: None,
        }
    }

    #[test]
    fn the_diagram_lists_every_transition_and_lights_the_state() {
        let d = machine_diagram(&comp(), Some("busy"));
        assert!(d.starts_with("stateDiagram-v2\n"));
        assert!(d.contains("[*] --> idle"));
        assert!(d.contains("idle --> busy: Req [inflight < 4]"));
        assert!(d.contains("half_open --> busy: Req"));
        assert!(d.contains("state \"half-open\" as half_open"));
        assert!(d.contains("style busy fill:#f59e0b"));
        assert!(!machine_diagram(&comp(), Some("ghost")).contains("style"));
    }

    #[test]
    fn the_diagram_parses() {
        let d = machine_diagram(&comp(), Some("idle"));
        crate::blocks::mermaid::diagram::parse(&d).unwrap();
    }

    #[test]
    fn status_lines() {
        let mut s = CompState {
            state: "busy".into(),
            vars: BTreeMap::new(),
            queue: vec![QueuedMsg {
                msg: 1,
                kind: "Req".into(),
                flow: 1,
            }],
            busy: 2,
            up: true,
        };
        assert_eq!(status_line(&s), "busy · 2 in progress · 1 queued");
        s.up = false;
        assert_eq!(status_line(&s), "busy · down");
    }
}
