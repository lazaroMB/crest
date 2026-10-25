//! Trace files to a waterfall: OTLP JSON, Jaeger JSON and Zipkin v2 JSON,
//! read with our own serde structs (no `opentelemetry-proto`: its JSON
//! support needs the generated types and features we do not otherwise
//! use).
//!
//! The format is told by shape: an object with `resourceSpans` (or a
//! Jaeger v3 `result` holding one) is OTLP, one with `data` or
//! `spans` is Jaeger, an array is Zipkin (or a list of Jaeger traces).
//! A file that is not one JSON value is read as OTLP JSON lines, which is
//! what the collector's file exporter writes. Every format becomes the
//! same [`RawSpan`]s, then [`build`] orders them into a tree (children
//! under their parent, by start time; a span whose parent is missing is
//! a root) and lays them out as timeline rows.

use std::collections::{HashMap, HashSet};

use domain::devops::trace::{
    KIND, MAX_SPANS, SERVICE, SPAN_ID, STATUS_MESSAGE, TRACE_ID, TraceFormat, TraceSpec,
    critical_path,
};
use domain::widget::timeline::{
    MAX_DEPTH, Marker, Span, SpanStatus, TimeUnit, TimelineRow, TimelineSpec,
};
use serde::Deserialize;
use serde_json::{Map, Value};

/// A span of any format, times in nanoseconds since the epoch.
#[derive(Debug, Clone, PartialEq)]
pub struct RawSpan {
    /// Trace id as written.
    pub trace_id: String,
    /// Span id as written.
    pub span_id: String,
    /// Parent span id; `None` (or empty) for a root.
    pub parent_id: Option<String>,
    /// Operation name.
    pub name: String,
    /// Service that emitted it.
    pub service: Option<String>,
    /// Start, ns since the epoch.
    pub start_ns: i128,
    /// End, ns since the epoch.
    pub end_ns: i128,
    /// `server`, `client`...
    pub kind: Option<String>,
    /// Outcome, when the span states one.
    pub status: Option<SpanStatus>,
    /// Why it failed.
    pub status_message: Option<String>,
    /// The span's attributes (tags).
    pub attrs: Map<String, Value>,
    /// Events (logs, annotations): ns since the epoch and a name.
    pub events: Vec<(i128, String)>,
}

/// Reads a trace file of any supported format.
///
/// # Errors
///
/// A message naming `path` for malformed JSON, an unknown shape or no
/// spans.
pub fn parse(bytes: &[u8], path: &str) -> Result<TraceSpec, String> {
    let (format, spans) = read_spans(bytes).map_err(|e| format!("`{path}`: {e}"))?;
    if spans.is_empty() {
        return Err(format!("`{path}` has no spans"));
    }
    let mut spec = build(format, spans);
    spec.path = Some(path.to_owned());
    Ok(spec)
}

fn read_spans(bytes: &[u8]) -> Result<(TraceFormat, Vec<RawSpan>), String> {
    let first = bytes.iter().find(|b| !b.is_ascii_whitespace()).copied();
    match first {
        Some(b'[') => {
            if let Ok(spans) = serde_json::from_slice::<Vec<ZipkinSpan>>(bytes) {
                return Ok((TraceFormat::Zipkin, zipkin(spans)));
            }
            if let Ok(traces) = serde_json::from_slice::<Vec<Vec<ZipkinSpan>>>(bytes) {
                return Ok((
                    TraceFormat::Zipkin,
                    zipkin(traces.into_iter().flatten().collect()),
                ));
            }
            match serde_json::from_slice::<Vec<JaegerTrace>>(bytes) {
                Ok(traces) => Ok((TraceFormat::Jaeger, jaeger(traces))),
                Err(e) => Err(format!(
                    "a JSON array that is neither Zipkin v2 spans nor Jaeger traces ({e})"
                )),
            }
        }
        Some(b'{') => match serde_json::from_slice::<Envelope>(bytes) {
            Ok(envelope) => envelope.spans(),
            Err(whole) => {
                // The collector's file exporter: one request per line.
                let mut spans = Vec::new();
                let mut lines = 0;
                for (ix, line) in bytes.split(|b| *b == b'\n').enumerate() {
                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    let request: OtlpRequest = serde_json::from_slice(line).map_err(|e| {
                        if lines == 0 {
                            format!("not trace JSON ({whole})")
                        } else {
                            format!("line {}: not OTLP JSON ({e})", ix + 1)
                        }
                    })?;
                    lines += 1;
                    spans.extend(otlp(request));
                }
                Ok((TraceFormat::Otlp, spans))
            }
        },
        _ => Err("not JSON: expected OTLP, Jaeger or Zipkin JSON".into()),
    }
}

/// Every top-level shape of an object file, read in one pass.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    #[serde(default, alias = "resource_spans")]
    resource_spans: Option<Vec<ResourceSpans>>,
    /// Jaeger's v3 API wraps OTLP in `result`.
    #[serde(default)]
    result: Option<OtlpRequest>,
    /// Jaeger's v1 API and UI download.
    #[serde(default)]
    data: Option<Vec<JaegerTrace>>,
    /// A single Jaeger trace.
    #[serde(default)]
    spans: Option<Vec<JaegerSpan>>,
    #[serde(default)]
    processes: HashMap<String, JaegerProcess>,
}

impl Envelope {
    fn spans(self) -> Result<(TraceFormat, Vec<RawSpan>), String> {
        if let Some(resource_spans) = self.resource_spans {
            return Ok((TraceFormat::Otlp, otlp(OtlpRequest { resource_spans })));
        }
        if let Some(request) = self.result {
            return Ok((TraceFormat::Otlp, otlp(request)));
        }
        if let Some(data) = self.data {
            return Ok((TraceFormat::Jaeger, jaeger(data)));
        }
        if let Some(spans) = self.spans {
            let trace = JaegerTrace {
                spans,
                processes: self.processes,
            };
            return Ok((TraceFormat::Jaeger, jaeger(vec![trace])));
        }
        Err("a JSON object without `resourceSpans` (OTLP) or `data`/`spans` (Jaeger)".into())
    }
}

// OTLP JSON (opentelemetry-proto's JSON mapping: camelCase keys, ids in
// hex, 64-bit integers as strings or numbers, enums as numbers or names).

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OtlpRequest {
    #[serde(default, alias = "resource_spans")]
    resource_spans: Vec<ResourceSpans>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceSpans {
    #[serde(default)]
    resource: Option<Resource>,
    #[serde(default, alias = "scope_spans", alias = "instrumentationLibrarySpans")]
    scope_spans: Vec<ScopeSpans>,
}

#[derive(Deserialize)]
struct Resource {
    #[serde(default)]
    attributes: Vec<KeyValue>,
}

#[derive(Deserialize)]
struct ScopeSpans {
    #[serde(default)]
    spans: Vec<OtlpSpan>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OtlpSpan {
    #[serde(default, alias = "trace_id")]
    trace_id: String,
    #[serde(default, alias = "span_id")]
    span_id: String,
    #[serde(default, alias = "parent_span_id")]
    parent_span_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: Value,
    #[serde(default, alias = "start_time_unix_nano")]
    start_time_unix_nano: Value,
    #[serde(default, alias = "end_time_unix_nano")]
    end_time_unix_nano: Value,
    #[serde(default)]
    attributes: Vec<KeyValue>,
    #[serde(default)]
    events: Vec<OtlpEvent>,
    #[serde(default)]
    status: Option<OtlpStatus>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OtlpEvent {
    #[serde(default, alias = "time_unix_nano")]
    time_unix_nano: Value,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct OtlpStatus {
    #[serde(default)]
    code: Value,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct KeyValue {
    key: String,
    #[serde(default)]
    value: Value,
}

/// An integer written as a JSON number or a string.
fn integer(value: &Value) -> Option<i128> {
    match value {
        Value::Number(n) => n
            .as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from))
            .or_else(|| n.as_f64().map(|f| f as i128)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// An OTLP `AnyValue` (`{"stringValue": "x"}`) as plain JSON.
fn any_value(value: &Value) -> Value {
    let Value::Object(map) = value else {
        return value.clone();
    };
    let Some((key, inner)) = map.iter().next() else {
        return Value::Null;
    };
    match key.as_str() {
        "intValue" | "int_value" => integer(inner).map_or(Value::Null, |n| {
            i64::try_from(n).map_or_else(|_| Value::String(n.to_string()), Value::from)
        }),
        "arrayValue" | "array_value" => Value::Array(
            inner
                .get("values")
                .and_then(Value::as_array)
                .map(|values| values.iter().map(any_value).collect())
                .unwrap_or_default(),
        ),
        "kvlistValue" | "kvlist_value" => Value::Object(
            inner
                .get("values")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|kv| {
                            let key = kv.get("key")?.as_str()?.to_owned();
                            Some((key, kv.get("value").map(any_value).unwrap_or_default()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        ),
        _ => inner.clone(),
    }
}

fn otlp_kind(kind: &Value) -> Option<String> {
    let name = match kind {
        Value::Number(n) => match n.as_u64()? {
            1 => "internal",
            2 => "server",
            3 => "client",
            4 => "producer",
            5 => "consumer",
            _ => return None,
        },
        Value::String(s) => {
            let lower = s.trim_start_matches("SPAN_KIND_").to_ascii_lowercase();
            return (!lower.is_empty() && lower != "unspecified").then_some(lower);
        }
        _ => return None,
    };
    Some(name.to_owned())
}

fn otlp_status(status: Option<&OtlpStatus>) -> (Option<SpanStatus>, Option<String>) {
    let Some(status) = status else {
        return (None, None);
    };
    let code = match &status.code {
        Value::Number(n) => n.as_u64().unwrap_or(0),
        Value::String(s) => match s.as_str() {
            "STATUS_CODE_OK" | "OK" | "Ok" => 1,
            "STATUS_CODE_ERROR" | "ERROR" | "Error" => 2,
            _ => 0,
        },
        _ => 0,
    };
    let message = (!status.message.is_empty()).then(|| status.message.clone());
    match code {
        1 => (Some(SpanStatus::Ok), message),
        2 => (Some(SpanStatus::Error), message),
        _ => (None, message),
    }
}

fn otlp(request: OtlpRequest) -> Vec<RawSpan> {
    let mut out = Vec::new();
    for resource in request.resource_spans {
        let resource_attrs = resource.resource.map(|r| r.attributes).unwrap_or_default();
        let service = resource_attrs
            .iter()
            .find(|kv| kv.key == "service.name")
            .map(|kv| text(&any_value(&kv.value)));
        for scope in resource.scope_spans {
            for span in scope.spans {
                let (status, status_message) = otlp_status(span.status.as_ref());
                let start = integer(&span.start_time_unix_nano).unwrap_or(0);
                let end = integer(&span.end_time_unix_nano).unwrap_or(start);
                out.push(RawSpan {
                    trace_id: span.trace_id,
                    span_id: span.span_id,
                    parent_id: Some(span.parent_span_id).filter(|p| !p.is_empty()),
                    name: span.name,
                    service: service.clone(),
                    start_ns: start,
                    end_ns: end.max(start),
                    kind: otlp_kind(&span.kind),
                    status,
                    status_message,
                    attrs: span
                        .attributes
                        .iter()
                        .map(|kv| (kv.key.clone(), any_value(&kv.value)))
                        .collect(),
                    events: span
                        .events
                        .into_iter()
                        .map(|e| (integer(&e.time_unix_nano).unwrap_or(start), e.name))
                        .collect(),
                });
            }
        }
    }
    out
}

// Jaeger JSON (v1 query API): times in microseconds.

#[derive(Deserialize)]
struct JaegerTrace {
    #[serde(default)]
    spans: Vec<JaegerSpan>,
    #[serde(default)]
    processes: HashMap<String, JaegerProcess>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JaegerSpan {
    #[serde(rename = "traceID", default)]
    trace_id: String,
    #[serde(rename = "spanID", default)]
    span_id: String,
    #[serde(default)]
    operation_name: String,
    #[serde(default)]
    references: Vec<JaegerReference>,
    /// Older exports give the parent directly.
    #[serde(rename = "parentSpanID", default)]
    parent_span_id: Option<String>,
    #[serde(default)]
    start_time: Value,
    #[serde(default)]
    duration: Value,
    #[serde(default)]
    tags: Vec<JaegerTag>,
    #[serde(default)]
    logs: Vec<JaegerLog>,
    #[serde(rename = "processID", default)]
    process_id: Option<String>,
    #[serde(default)]
    process: Option<JaegerProcess>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JaegerReference {
    #[serde(default)]
    ref_type: String,
    #[serde(rename = "spanID", default)]
    span_id: String,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JaegerProcess {
    #[serde(default)]
    service_name: String,
}

#[derive(Deserialize)]
struct JaegerTag {
    key: String,
    #[serde(default)]
    value: Value,
}

#[derive(Deserialize)]
struct JaegerLog {
    #[serde(default)]
    timestamp: Value,
    #[serde(default)]
    fields: Vec<JaegerTag>,
}

/// A tag that means true (`true`, `"true"`).
fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::String(s) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn jaeger(traces: Vec<JaegerTrace>) -> Vec<RawSpan> {
    let mut out = Vec::new();
    for trace in traces {
        for span in trace.spans {
            let parent = span
                .references
                .iter()
                .find(|r| r.ref_type.eq_ignore_ascii_case("CHILD_OF"))
                .or_else(|| span.references.first())
                .map(|r| r.span_id.clone())
                .or(span.parent_span_id.clone())
                .filter(|p| !p.is_empty() && *p != "0");
            let service = span
                .process
                .as_ref()
                .or_else(|| {
                    span.process_id
                        .as_ref()
                        .and_then(|id| trace.processes.get(id))
                })
                .map(|p| p.service_name.clone())
                .filter(|s| !s.is_empty());
            let start = integer(&span.start_time).unwrap_or(0) * 1_000;
            let end = start + integer(&span.duration).unwrap_or(0).max(0) * 1_000;
            let mut attrs = Map::new();
            let mut kind = None;
            let mut status = None;
            let mut status_message = None;
            for tag in span.tags {
                match tag.key.as_str() {
                    "span.kind" => kind = Some(text(&tag.value)),
                    "error" if truthy(&tag.value) => status = Some(SpanStatus::Error),
                    "otel.status_code" => match text(&tag.value).as_str() {
                        "ERROR" => status = Some(SpanStatus::Error),
                        "OK" if status.is_none() => status = Some(SpanStatus::Ok),
                        _ => {}
                    },
                    "otel.status_description" => status_message = Some(text(&tag.value)),
                    _ => {
                        attrs.insert(tag.key, tag.value);
                    }
                }
            }
            let events = span
                .logs
                .into_iter()
                .map(|log| {
                    let at = integer(&log.timestamp).unwrap_or(start / 1_000) * 1_000;
                    let name = log
                        .fields
                        .iter()
                        .find(|f| f.key == "event" || f.key == "message")
                        .or_else(|| log.fields.first())
                        .map(|f| text(&f.value))
                        .unwrap_or_else(|| "log".into());
                    (at, name)
                })
                .collect();
            out.push(RawSpan {
                trace_id: span.trace_id,
                span_id: span.span_id,
                parent_id: parent,
                name: span.operation_name,
                service,
                start_ns: start,
                end_ns: end,
                kind,
                status,
                status_message,
                attrs,
                events,
            });
        }
    }
    out
}

// Zipkin v2 JSON: times in microseconds, tags as strings.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ZipkinSpan {
    trace_id: String,
    id: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    timestamp: Option<Value>,
    #[serde(default)]
    duration: Option<Value>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    local_endpoint: Option<ZipkinEndpoint>,
    #[serde(default)]
    tags: Map<String, Value>,
    #[serde(default)]
    annotations: Vec<ZipkinAnnotation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ZipkinEndpoint {
    #[serde(default)]
    service_name: Option<String>,
}

#[derive(Deserialize)]
struct ZipkinAnnotation {
    #[serde(default)]
    timestamp: Value,
    #[serde(default)]
    value: String,
}

fn zipkin(spans: Vec<ZipkinSpan>) -> Vec<RawSpan> {
    spans
        .into_iter()
        .map(|span| {
            let start = span.timestamp.as_ref().and_then(integer).unwrap_or(0) * 1_000;
            let end = start + span.duration.as_ref().and_then(integer).unwrap_or(0).max(0) * 1_000;
            let mut attrs = span.tags;
            let mut status = None;
            let mut status_message = None;
            if let Some(error) = attrs.shift_remove("error") {
                status = Some(SpanStatus::Error);
                let message = text(&error);
                if !message.is_empty() && message != "true" {
                    status_message = Some(message);
                }
            }
            RawSpan {
                trace_id: span.trace_id,
                span_id: span.id,
                parent_id: span.parent_id.filter(|p| !p.is_empty()),
                name: span.name.unwrap_or_default(),
                service: span
                    .local_endpoint
                    .and_then(|e| e.service_name)
                    .filter(|s| !s.is_empty()),
                start_ns: start,
                end_ns: end,
                kind: span.kind.map(|k| k.to_ascii_lowercase()),
                status,
                status_message,
                attrs,
                events: span
                    .annotations
                    .into_iter()
                    .map(|a| (integer(&a.timestamp).unwrap_or(0) * 1_000, a.value))
                    .collect(),
            }
        })
        .collect()
}

/// A JSON value as text: strings without quotes.
pub(crate) fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `id` shortened for a label: the first 8 characters.
fn short(id: &str) -> &str {
    id.char_indices().nth(8).map_or(id, |(ix, _)| &id[..ix])
}

/// Orders `spans` into a tree and lays them out as a [`TraceSpec`]: one
/// row per span, times in ms from the earliest start, the critical path
/// of the longest root.
#[must_use]
pub fn build(format: TraceFormat, spans: Vec<RawSpan>) -> TraceSpec {
    let total_spans = spans.len();
    let t0 = spans.iter().map(|s| s.start_ns).min().unwrap_or(0);
    // Parents are looked up within the trace; a span id seen twice (a
    // Zipkin shared span) keeps its first occurrence as the parent.
    let mut by_id: HashMap<(&str, &str), usize> = HashMap::new();
    for (ix, span) in spans.iter().enumerate() {
        by_id
            .entry((span.trace_id.as_str(), span.span_id.as_str()))
            .or_insert(ix);
    }
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); spans.len()];
    let mut roots = Vec::new();
    for (ix, span) in spans.iter().enumerate() {
        let parent = span
            .parent_id
            .as_deref()
            .and_then(|p| by_id.get(&(span.trace_id.as_str(), p)))
            .copied()
            .filter(|&p| p != ix);
        match parent {
            Some(p) => children[p].push(ix),
            None => roots.push(ix),
        }
    }
    let by_start = |list: &mut Vec<usize>| {
        list.sort_by(|&a, &b| {
            spans[a]
                .start_ns
                .cmp(&spans[b].start_ns)
                .then(spans[a].end_ns.cmp(&spans[b].end_ns).reverse())
                .then(a.cmp(&b))
        });
    };
    by_start(&mut roots);
    for list in &mut children {
        by_start(list);
    }
    // Preorder, with an explicit stack; spans caught in a parent cycle
    // are not reachable from a root and are added as roots after.
    let mut order: Vec<(usize, u32)> = Vec::with_capacity(spans.len());
    let mut seen = vec![false; spans.len()];
    let walk = |root: usize, order: &mut Vec<(usize, u32)>, seen: &mut Vec<bool>| {
        let mut stack = vec![(root, 0u32)];
        while let Some((ix, depth)) = stack.pop() {
            if seen[ix] {
                continue;
            }
            seen[ix] = true;
            order.push((ix, depth));
            for &kid in children[ix].iter().rev() {
                stack.push((kid, depth + 1));
            }
        }
    };
    for &root in &roots {
        walk(root, &mut order, &mut seen);
    }
    for ix in 0..spans.len() {
        if !seen[ix] {
            walk(ix, &mut order, &mut seen);
        }
    }
    order.truncate(MAX_SPANS);

    let trace_ids: HashSet<&str> = spans.iter().map(|s| s.trace_id.as_str()).collect();
    let grouped = trace_ids.len() > 1;
    let mut services: Vec<String> = Vec::new();
    let mut row_ids: HashSet<String> = HashSet::new();
    let ms = |ns: i128| (ns - t0) as f64 / 1e6;
    let mut rows = Vec::with_capacity(order.len());
    for (ix, depth) in order {
        let span = &spans[ix];
        if let Some(service) = &span.service
            && !services.contains(service)
        {
            services.push(service.clone());
        }
        let mut id = if span.span_id.is_empty() {
            format!("span-{ix}")
        } else {
            span.span_id.clone()
        };
        if row_ids.contains(&id) {
            id = format!("{}:{}", short(&span.trace_id), id);
        }
        let mut n = 2;
        let base = id.clone();
        while row_ids.contains(&id) {
            id = format!("{base}~{n}");
            n += 1;
        }
        row_ids.insert(id.clone());
        let mut attrs = Map::new();
        if let Some(service) = &span.service {
            attrs.insert(SERVICE.into(), service.clone().into());
        }
        if let Some(kind) = &span.kind {
            attrs.insert(KIND.into(), kind.clone().into());
        }
        attrs.insert(SPAN_ID.into(), span.span_id.clone().into());
        attrs.insert(TRACE_ID.into(), span.trace_id.clone().into());
        if let Some(message) = &span.status_message {
            attrs.insert(STATUS_MESSAGE.into(), message.clone().into());
        }
        for (key, value) in &span.attrs {
            attrs.entry(key.clone()).or_insert_with(|| value.clone());
        }
        let name = if span.name.is_empty() {
            "(unnamed)".to_owned()
        } else {
            span.name.clone()
        };
        rows.push(TimelineRow {
            id,
            label: name,
            depth: depth.min(MAX_DEPTH),
            group: grouped.then(|| short(&span.trace_id).to_owned()),
            spans: vec![Span {
                start: ms(span.start_ns),
                end: ms(span.end_ns.max(span.start_ns)),
                label: None,
                color_key: span.service.clone(),
                status: span.status,
                attrs,
            }],
            markers: span
                .events
                .iter()
                .map(|(at, name)| Marker {
                    at: ms(*at),
                    label: Some(name.clone()),
                    color_key: (name == "exception").then(|| "exception".to_owned()),
                })
                .collect(),
        });
    }
    let timeline = TimelineSpec {
        title: None,
        unit: TimeUnit::Ms,
        rows,
        range: None,
    };
    TraceSpec {
        title: None,
        path: None,
        format,
        traces: trace_ids.len(),
        services,
        started_at: (total_spans > 0).then(|| t0 as f64 / 1e9),
        total_spans,
        critical_path: critical_path(&timeline),
        timeline,
    }
}
