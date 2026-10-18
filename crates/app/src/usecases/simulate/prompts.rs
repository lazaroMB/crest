//! What the model is told: the format of `crest.sysmodel/v1`, the frozen
//! assertions and, in a repair round, what was wrong.

use std::fmt::Write as _;

use super::spec::SpecDoc;

/// System prompt of the call that derives the assertions.
pub(crate) const PLAN_SYSTEM: &str = "You turn the test protocol of a software Spec into \
checkable assertions for an executable behavioural model, BEFORE the model exists. Name the \
components of the black-box diagram with short ids ([a-z][a-z0-9_]*) and write one scenario \
per test-protocol item (or group of items), each with the assertions that item demands. Be \
faithful to the protocol: do not weaken a requirement to make it easy to pass. Reply by \
calling the submit_assertions tool.";

/// System prompt of the call that writes the model.
pub(crate) const MODEL_SYSTEM: &str = "You write executable behavioural sketches of software \
designs in the crest.sysmodel/v1 JSON format, from a Spec's diagrams. Model only what the Spec \
says; every component cites the Spec element or evidence id it comes from. Reply by calling \
the submit_sysmodel tool.";

/// Description of the assertions tool.
pub(crate) const PLAN_TOOL: &str = "Submits the components and the frozen scenarios with \
their assertions.";

/// Description of the model tool.
pub(crate) const MODEL_TOOL: &str = "Submits the crest.sysmodel/v1 model.";

/// The semantics the model must follow.
pub(crate) const FORMAT: &str = "\
Format rules (crest.sysmodel/v1):
- components: id, kind (client|service|queue|db|cache|lb|external|timer), cites (Spec element \
or evidence ids), ports [{id, dir: in|out, accepts|emits: [message types]}], optional capacity \
{concurrency, queue, on_full: reject|drop}, optional latency {dist: fixed, ms: N} or {dist: \
uniform, ms: [lo, hi]}, vars {name: integer}, machine {initial, states, transitions}.
- transitions are tried in order; the first with matching from (a state or \"*\"), on (message \
type) and guard fires. effects run in order: {\"set\": var, \"expr\": integer expression} or \
{\"emit\": type, \"port\": out port or \"self\", \"after_ms\": N}. to defaults to from.
- a message reaching a component is handled after its latency if fewer than concurrency are in \
progress, waits in the FIFO queue if there is room, else is rejected or dropped.
- connections {id, from: \"comp.outport\", to: \"comp.inport\", latency_ms}. A request/response \
needs a connection each way.
- expressions: integers, 'strings', true/false, + - * / %, comparisons, && || !, parentheses. \
Guards and set expressions read the component's own vars and state, queue, busy, up, now. \
Assertions read comp.var, comp.state, comp.queue, comp.busy, comp.up and now.
- scenarios: id, seed, until_ms, inject [{at_ms, to: \"comp.port\" (an out port sends, an in \
port receives), msg, repeat: {every_ms, count}}], faults [{at_ms, kill, for_ms}], expect, \
covers.
- expectations: {\"always\": expr}, {\"never\": expr}, {\"eventually\": expr, \"within_ms\": \
N}, {\"count\": type, \"at\": comp, \"event\": handled|unhandled|rejected|dropped, \"min\", \
\"max\"}, {\"path\": [comp, ...]} (the components one injected message and the messages it \
causes visit, starting where it was injected).";

fn diagrams(out: &mut String, title: &str, list: &[String]) {
    for d in list {
        let _ = write!(out, "\n{title}:\n```mermaid\n{d}\n```\n");
    }
}

fn spec_context(doc: &SpecDoc) -> String {
    let mut out = format!("Spec: {}\n", doc.title);
    diagrams(&mut out, "Black-box diagram", &doc.black_box);
    diagrams(&mut out, "Sequence diagram", &doc.sequences);
    diagrams(&mut out, "State diagram", &doc.states);
    out
}

/// The request for the frozen assertions.
pub(crate) fn plan_prompt(doc: &SpecDoc) -> String {
    let mut out = spec_context(doc);
    let _ = write!(
        out,
        "\nTest protocol:\n{}\n\nTest-protocol items: {}\nHypotheses: {}\n\n{FORMAT}\n\n\
Return the component ids (one per element of the black-box diagram that takes part) and the \
scenarios: id, description, covers (the T and H ids it checks) and expect. Use only the \
expectation forms above. Assertions may name variables you expect the model to need \
(comp.var); the model will be written to provide them. Each sequence diagram should become a \
path assertion.",
        doc.test_protocol.trim(),
        if doc.test_items.is_empty() {
            "none numbered".to_owned()
        } else {
            doc.test_items.join(", ")
        },
        if doc.hypotheses.is_empty() {
            "none".to_owned()
        } else {
            doc.hypotheses.join(", ")
        },
    );
    out
}

/// The request for the model, with the frozen plan as JSON.
pub(crate) fn model_prompt(doc: &SpecDoc, plan_json: &str, evidence: &[String]) -> String {
    let mut out = spec_context(doc);
    let _ = write!(
        out,
        "\nDesign sections:\n{}\n\nFrozen plan (components and scenarios):\n```json\n{plan_json}\n```\n\n\
{FORMAT}\n\nWrite the whole model: use exactly the planned component ids, give every scenario \
of the plan with the same id, covers and expect (they are frozen and will be restored if \
changed), and choose its injections, faults, seed and until_ms. Declare every variable the \
assertions read. Known evidence ids: {}.",
        doc.design.trim(),
        if evidence.is_empty() {
            "none".to_owned()
        } else {
            evidence.join(", ")
        },
    );
    out
}

/// The request to fix a model.
pub(crate) fn repair_prompt(first: &str, model_json: &str, errors: &[String]) -> String {
    let list: String = errors.iter().map(|e| format!("\n- {e}")).collect();
    format!(
        "{first}\n\nYour previous model:\n```json\n{model_json}\n```\n\nIt is not valid for these \
reasons:{list}\n\nFix every reason and send the whole model again with the same tool. Do not \
change the frozen assertions: fix the model instead."
    )
}
