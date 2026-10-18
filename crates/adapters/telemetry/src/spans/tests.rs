use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use application::ports::telemetry::{LlmCallRecord, TelemetryPort, ToolCallRecord, TurnSpan};
use domain::conversation::{SessionId, StopReason, TurnId, Usage};
use domain::model::{ModelId, ModelRole};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{Layer, Registry};

use super::*;

/// What the capture layer saw of one span.
#[derive(Debug, Clone, Default)]
struct Captured {
    name: &'static str,
    parent: Option<u64>,
    fields: BTreeMap<String, String>,
    closed: bool,
}

/// Layer that records every span with its parent and fields, by span id.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<BTreeMap<u64, Captured>>>);

impl Capture {
    fn spans(&self) -> Vec<(u64, Captured)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(id, s)| (*id, s.clone()))
            .collect()
    }

    fn named(&self, name: &str) -> Vec<(u64, Captured)> {
        self.spans()
            .into_iter()
            .filter(|(_, s)| s.name == name)
            .collect()
    }
}

struct Fields<'a>(&'a mut BTreeMap<String, String>);

impl Visit for Fields<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }
}

impl<S> Layer<S> for Capture
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let parent = ctx
            .span(id)
            .and_then(|span| span.parent())
            .map(|p| p.id().into_u64());
        let mut captured = Captured {
            name: attrs.metadata().name(),
            parent,
            ..Captured::default()
        };
        attrs.record(&mut Fields(&mut captured.fields));
        self.0.lock().unwrap().insert(id.into_u64(), captured);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, _ctx: Context<'_, S>) {
        if let Some(span) = self.0.lock().unwrap().get_mut(&id.into_u64()) {
            values.record(&mut Fields(&mut span.fields));
        }
    }

    fn on_close(&self, id: Id, _ctx: Context<'_, S>) {
        if let Some(span) = self.0.lock().unwrap().get_mut(&id.into_u64()) {
            span.closed = true;
        }
    }
}

fn with_capture(f: impl FnOnce()) -> Capture {
    let capture = Capture::default();
    let subscriber = Registry::default().with(capture.clone());
    tracing::subscriber::with_default(subscriber, f);
    capture
}

fn turn() -> TurnSpan {
    TurnSpan {
        session_id: SessionId::new(),
        turn_id: TurnId::new(),
        role: ModelRole::Main,
        model: ModelId::new("claude-sonnet-5-5"),
    }
}

fn usage(input: u64, output: u64) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        ..Usage::default()
    }
}

#[test]
fn turn_started_returns_distinct_span_ids() {
    let telemetry = TracingTelemetry::new();
    let a = telemetry.turn_started(turn());
    let b = telemetry.turn_started(turn());
    assert_ne!(a, b);
    assert_eq!(telemetry.open_turns(), 2);
}

#[test]
fn turn_span_carries_ids_and_closes_on_finish() {
    let telemetry = TracingTelemetry::new();
    let started = turn();
    let session = started.session_id.to_string();
    let capture = with_capture(|| {
        let span = telemetry.turn_started(started);
        telemetry.turn_finished(span, usage(10, 5), Some(StopReason::EndTurn));
    });

    let turns = capture.named("turn");
    assert_eq!(turns.len(), 1);
    let (_, turn) = &turns[0];
    assert_eq!(turn.parent, None);
    assert!(turn.closed);
    assert_eq!(turn.fields["session_id"], session);
    assert_eq!(turn.fields["role"], "main");
    assert_eq!(turn.fields["model"], "claude-sonnet-5-5");
    assert_eq!(turn.fields["input_tokens"], "10");
    assert_eq!(turn.fields["output_tokens"], "5");
    assert_eq!(turn.fields["stop_reason"], "end_turn");
    assert_eq!(turn.fields["failed"], "false");
    assert_eq!(telemetry.open_turns(), 0);
}

#[test]
fn llm_and_tool_calls_are_children_of_their_turn() {
    let telemetry = TracingTelemetry::new();
    let capture = with_capture(|| {
        let first = telemetry.turn_started(turn());
        let second = telemetry.turn_started(turn());
        telemetry.llm_call(LlmCallRecord {
            parent: first,
            model: ModelId::new("claude-sonnet-5-5"),
            usage: usage(100, 20),
            latency_ms: 1200,
            error: None,
        });
        telemetry.tool_call(ToolCallRecord {
            parent: second,
            name: "grep".into(),
            duration_ms: 3,
            ok: true,
        });
        telemetry.turn_finished(first, usage(100, 20), Some(StopReason::ToolUse));
        telemetry.turn_finished(second, Usage::default(), None);
    });

    let turns: Vec<u64> = capture.named("turn").iter().map(|(id, _)| *id).collect();
    let (_, llm) = &capture.named("llm_call")[0];
    let (_, tool) = &capture.named("tool_call")[0];
    assert_eq!(llm.parent, Some(turns[0]));
    assert_eq!(tool.parent, Some(turns[1]));
    assert_eq!(llm.fields["input_tokens"], "100");
    assert_eq!(llm.fields["latency_ms"], "1200");
    assert!(!llm.fields.contains_key("error"));
    assert_eq!(tool.fields["tool"], "grep");
    assert_eq!(tool.fields["ok"], "true");
    assert!(capture.spans().iter().all(|(_, s)| s.closed));
    let (_, failed) = &capture.named("turn")[1];
    assert_eq!(failed.fields["failed"], "true");
    assert!(!failed.fields.contains_key("stop_reason"));
}

#[test]
fn llm_call_error_is_recorded_and_truncated() {
    let telemetry = TracingTelemetry::new();
    let long = "é".repeat(MAX_ERROR_LEN);
    let capture = with_capture(|| {
        let span = telemetry.turn_started(turn());
        telemetry.llm_call(LlmCallRecord {
            parent: span,
            model: ModelId::new("claude-sonnet-5-5"),
            usage: Usage::default(),
            latency_ms: 5,
            error: Some(long.clone()),
        });
        telemetry.turn_finished(span, Usage::default(), None);
    });

    let (_, llm) = &capture.named("llm_call")[0];
    assert!(llm.fields["error"].len() <= MAX_ERROR_LEN);
    assert!(long.starts_with(&llm.fields["error"]));
}

#[test]
fn records_for_unknown_turns_become_root_spans() {
    let telemetry = TracingTelemetry::new();
    let capture = with_capture(|| {
        telemetry.tool_call(ToolCallRecord {
            parent: SpanId(42),
            name: "read".into(),
            duration_ms: 1,
            ok: false,
        });
        telemetry.turn_finished(SpanId(42), Usage::default(), None);
    });

    let (_, tool) = &capture.named("tool_call")[0];
    assert_eq!(tool.parent, None);
    assert!(capture.named("turn").is_empty());
}

#[test]
fn spans_never_carry_unexpected_fields() {
    // Guard against adding message content or credentials by accident:
    // every field the adapter can emit is listed here.
    let allowed = [
        "session_id",
        "turn_id",
        "role",
        "model",
        "input_tokens",
        "output_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
        "stop_reason",
        "failed",
        "latency_ms",
        "error",
        "tool",
        "duration_ms",
        "ok",
    ];
    let telemetry = TracingTelemetry::new();
    let capture = with_capture(|| {
        let span = telemetry.turn_started(turn());
        telemetry.llm_call(LlmCallRecord {
            parent: span,
            model: ModelId::new("m"),
            usage: usage(1, 1),
            latency_ms: 1,
            error: Some("boom".into()),
        });
        telemetry.tool_call(ToolCallRecord {
            parent: span,
            name: "t".into(),
            duration_ms: 1,
            ok: true,
        });
        telemetry.turn_finished(span, usage(1, 1), Some(StopReason::EndTurn));
    });
    for (_, span) in capture.spans() {
        for field in span.fields.keys() {
            assert!(
                allowed.contains(&field.as_str()),
                "unexpected field {field}"
            );
        }
    }
}
