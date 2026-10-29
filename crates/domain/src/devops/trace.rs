//! A distributed trace as a waterfall (`show_trace`).
//!
//! The adapter turns the spans of an OTLP, Jaeger or Zipkin file into one
//! [`TimelineSpec`] row per span, in tree order: each span follows its
//! parent, one level deeper, siblings by start time. Times are
//! milliseconds from the start of the first span; [`TraceSpec::started_at`]
//! keeps the wall clock. Each row has exactly one span, whose attributes
//! start with the crest keys ([`SERVICE`], [`KIND`], [`SPAN_ID`],
//! [`TRACE_ID`], [`STATUS_MESSAGE`]) followed by the span's own.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::invalid;
use crate::block::BlockError;
use crate::widget::list::value_text;
use crate::widget::timeline::{SpanStatus, TimelineRow, TimelineSpec};

/// Spans a trace block keeps; the rest of a larger file is dropped.
pub const MAX_SPANS: usize = 20_000;

/// Attribute of a row's span: the service that emitted it.
pub const SERVICE: &str = "service";
/// Attribute: the span kind (`server`, `client`, `internal`...).
pub const KIND: &str = "kind";
/// Attribute: the span id as the file wrote it.
pub const SPAN_ID: &str = "span_id";
/// Attribute: the trace id as the file wrote it.
pub const TRACE_ID: &str = "trace_id";
/// Attribute: the status description of a failed span.
pub const STATUS_MESSAGE: &str = "status_message";

const BLOCK: &str = "trace";

/// The file format a trace came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceFormat {
    /// OTLP JSON (`ExportTraceServiceRequest`, or one per line).
    Otlp,
    /// Jaeger's JSON (`/api/traces` or the UI's download).
    Jaeger,
    /// Zipkin v2 JSON.
    Zipkin,
}

impl TraceFormat {
    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Otlp => "OTLP",
            Self::Jaeger => "Jaeger",
            Self::Zipkin => "Zipkin",
        }
    }
}

/// One or more traces, drawn as a waterfall.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TraceSpec {
    /// Caption; the file name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Workspace path of the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// What the file was.
    pub format: TraceFormat,
    /// Distinct trace ids.
    pub traces: usize,
    /// Services, in order of first appearance.
    #[serde(default)]
    pub services: Vec<String>,
    /// Wall clock of the first span start, in seconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<f64>,
    /// Spans in the file; more than the rows when it was cut at
    /// [`MAX_SPANS`].
    pub total_spans: usize,
    /// One row per span in tree order, times in ms from the first start.
    pub timeline: TimelineSpec,
    /// Row ids on the critical path of the longest trace, root first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub critical_path: Vec<String>,
}

/// The time a span spent outside its children, and its place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpanTime {
    /// Row index.
    pub row: usize,
    /// End minus start, in ms.
    pub duration: f64,
    /// Duration minus the time covered by its children, in ms.
    pub self_time: f64,
}

impl TraceSpec {
    /// Checks the timeline, one span per row, the span count and that the
    /// critical path names rows.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        self.timeline.validate()?;
        if self.timeline.rows.len() > MAX_SPANS {
            return Err(invalid(
                BLOCK,
                format!(
                    "{} spans; at most {MAX_SPANS} are kept",
                    self.timeline.rows.len()
                ),
            ));
        }
        if let Some(row) = self.timeline.rows.iter().find(|r| r.spans.len() != 1) {
            return Err(invalid(
                BLOCK,
                format!("row {:?} must have exactly one span", row.id),
            ));
        }
        if self.total_spans < self.timeline.rows.len() {
            return Err(invalid(BLOCK, "total_spans is less than the rows"));
        }
        if let Some(id) = self
            .critical_path
            .iter()
            .find(|id| !self.timeline.rows.iter().any(|r| &r.id == *id))
        {
            return Err(invalid(
                BLOCK,
                format!("critical path names unknown span {id:?}"),
            ));
        }
        Ok(())
    }

    /// Spans shown.
    #[must_use]
    pub fn span_count(&self) -> usize {
        self.timeline.rows.len()
    }

    /// End of the last span minus start of the first, in ms.
    #[must_use]
    pub fn duration(&self) -> f64 {
        self.timeline
            .data_extent()
            .map_or(0.0, |(lo, hi)| (hi - lo).max(0.0))
    }

    /// Rows whose span failed.
    #[must_use]
    pub fn errors(&self) -> Vec<usize> {
        self.timeline
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                row.spans
                    .first()
                    .is_some_and(|s| s.status == Some(SpanStatus::Error))
            })
            .map(|(ix, _)| ix)
            .collect()
    }

    /// Attribute `key` of row `ix`'s span as text.
    #[must_use]
    pub fn attr(&self, ix: usize, key: &str) -> Option<String> {
        let span = self.timeline.rows.get(ix)?.spans.first()?;
        span.attrs.get(key).map(value_text)
    }

    /// Duration and self time of every span; the `n` with the most self
    /// time first when `n` is given.
    #[must_use]
    pub fn times(&self, n: Option<usize>) -> Vec<SpanTime> {
        let mut times = span_times(&self.timeline);
        if let Some(n) = n {
            times.sort_by(|a, b| b.self_time.total_cmp(&a.self_time).then(a.row.cmp(&b.row)));
            times.truncate(n);
        }
        times
    }

    /// `service: name` of row `ix`.
    #[must_use]
    pub fn span_name(&self, ix: usize) -> String {
        let Some(row) = self.timeline.rows.get(ix) else {
            return String::new();
        };
        match self.attr(ix, SERVICE) {
            Some(service) if !service.is_empty() => format!("{service}: {}", row.label),
            _ => row.label.clone(),
        }
    }

    /// The timeline with only the rows of the critical path (depths kept).
    #[must_use]
    pub fn critical_timeline(&self) -> TimelineSpec {
        let rows = self
            .timeline
            .rows
            .iter()
            .filter(|row| self.critical_path.contains(&row.id))
            .cloned()
            .collect();
        TimelineSpec {
            title: self.timeline.title.clone(),
            unit: self.timeline.unit,
            rows,
            range: self.timeline.range,
        }
    }
}

/// The parent row of every row: the nearest row above it with a smaller
/// depth (rows are in tree order).
#[must_use]
pub fn parents(rows: &[TimelineRow]) -> Vec<Option<usize>> {
    let mut stack: Vec<usize> = Vec::new();
    rows.iter()
        .enumerate()
        .map(|(ix, row)| {
            while stack
                .last()
                .is_some_and(|&top| rows[top].depth >= row.depth)
            {
                stack.pop();
            }
            let parent = stack.last().copied();
            stack.push(ix);
            parent
        })
        .collect()
}

fn bounds(row: &TimelineRow) -> (f64, f64) {
    row.spans.first().map_or((0.0, 0.0), |s| (s.start, s.end))
}

/// Duration and self time of each row (in row order): the self time is
/// the duration minus the union of the children's intervals clipped to
/// the span.
#[must_use]
pub fn span_times(timeline: &TimelineSpec) -> Vec<SpanTime> {
    let rows = &timeline.rows;
    let parents = parents(rows);
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
    for (ix, parent) in parents.iter().enumerate() {
        if let Some(p) = parent {
            children[*p].push(ix);
        }
    }
    rows.iter()
        .enumerate()
        .map(|(ix, row)| {
            let (start, end) = bounds(row);
            let mut kids: Vec<(f64, f64)> = children[ix]
                .iter()
                .map(|&c| {
                    let (s, e) = bounds(&rows[c]);
                    (s.max(start), e.min(end))
                })
                .filter(|(s, e)| e > s)
                .collect();
            kids.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut covered = 0.0;
            let mut reach = f64::NEG_INFINITY;
            for (s, e) in kids {
                let s = s.max(reach);
                if e > s {
                    covered += e - s;
                }
                reach = reach.max(e);
            }
            let duration = end - start;
            SpanTime {
                row: ix,
                duration,
                self_time: (duration - covered).max(0.0),
            }
        })
        .collect()
}

/// The critical path of the longest root span, as row ids from the root
/// down: from the end of a span, the child that finished last before the
/// current point is on the path, then the point moves to that child's
/// start and the next child that finished before it follows.
#[must_use]
pub fn critical_path(timeline: &TimelineSpec) -> Vec<String> {
    let rows = &timeline.rows;
    let parents = parents(rows);
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
    for (ix, parent) in parents.iter().enumerate() {
        if let Some(p) = parent {
            children[*p].push(ix);
        }
    }
    for kids in &mut children {
        kids.sort_by(|&a, &b| bounds(&rows[b]).1.total_cmp(&bounds(&rows[a]).1));
    }
    let root = (0..rows.len())
        .filter(|&ix| parents[ix].is_none())
        .max_by(|&a, &b| {
            let (sa, ea) = bounds(&rows[a]);
            let (sb, eb) = bounds(&rows[b]);
            (ea - sa).total_cmp(&(eb - sb)).then(b.cmp(&a))
        });
    let Some(root) = root else {
        return Vec::new();
    };
    let mut path = Vec::new();
    // (row, until): an explicit stack, so a deep trace cannot overflow.
    let mut work = vec![(root, f64::INFINITY)];
    while let Some((ix, until)) = work.pop() {
        path.push(ix);
        let (start, end) = bounds(&rows[ix]);
        let mut cursor = end.min(until);
        let mut picked = Vec::new();
        for &kid in &children[ix] {
            let (ks, _) = bounds(&rows[kid]);
            if ks < cursor && ks >= start {
                picked.push((kid, cursor));
                cursor = ks;
            }
        }
        // Later children first on the stack's top, so the path reads in
        // time order once sorted below.
        work.extend(picked.into_iter().rev());
    }
    // Tree order is time order within each parent.
    path.sort_unstable();
    path.into_iter().map(|ix| rows[ix].id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;
    use crate::widget::timeline::{Span, TimeUnit};

    fn row(id: &str, depth: u32, start: f64, end: f64) -> TimelineRow {
        TimelineRow {
            id: id.into(),
            label: id.into(),
            depth,
            group: None,
            spans: vec![Span {
                start,
                end,
                label: None,
                color_key: None,
                status: None,
                attrs: Map::new(),
            }],
            markers: Vec::new(),
        }
    }

    fn spec(rows: Vec<TimelineRow>) -> TraceSpec {
        let timeline = TimelineSpec {
            title: None,
            unit: TimeUnit::Ms,
            rows,
            range: None,
        };
        TraceSpec {
            title: None,
            path: None,
            format: TraceFormat::Otlp,
            traces: 1,
            services: Vec::new(),
            started_at: None,
            total_spans: timeline.rows.len(),
            critical_path: critical_path(&timeline),
            timeline,
        }
    }

    /// root 0..100: a 0..30, b 20..90 (b1 25..40, b2 50..85), c 92..95.
    fn sample() -> TraceSpec {
        spec(vec![
            row("root", 0, 0.0, 100.0),
            row("a", 1, 0.0, 30.0),
            row("b", 1, 20.0, 90.0),
            row("b1", 2, 25.0, 40.0),
            row("b2", 2, 50.0, 85.0),
            row("c", 1, 92.0, 95.0),
        ])
    }

    #[test]
    fn parents_follow_depth() {
        let trace = sample();
        assert_eq!(
            parents(&trace.timeline.rows),
            [None, Some(0), Some(0), Some(2), Some(2), Some(0)]
        );
    }

    #[test]
    fn self_time_excludes_overlapping_children() {
        let trace = sample();
        let times = trace.times(None);
        // root: children cover 0..90 and 92..95 = 93.
        assert_eq!(times[0].self_time, 7.0);
        // b: 70 minus 15 and 35.
        assert_eq!(times[2].self_time, 20.0);
        assert_eq!(times[4].self_time, 35.0);
        let top = trace.times(Some(2));
        assert_eq!(top.iter().map(|t| t.row).collect::<Vec<_>>(), [4, 1]);
    }

    #[test]
    fn critical_path_walks_back_from_the_end() {
        let trace = sample();
        // From 100: c (92..95), then b (ends 90 < 92), within b: b2, b1;
        // then a starts at 0 but ends at 30 > b's start 20: still before
        // the cursor 20 → a.
        assert_eq!(trace.critical_path, ["root", "a", "b", "b1", "b2", "c"]);
        let lone = spec(vec![row("r", 0, 0.0, 10.0), row("x", 1, 1.0, 9.0)]);
        assert_eq!(lone.critical_path, ["r", "x"]);
        let parallel = spec(vec![
            row("r", 0, 0.0, 10.0),
            row("fast", 1, 0.0, 3.0),
            row("slow", 1, 0.0, 9.0),
        ]);
        assert_eq!(parallel.critical_path, ["r", "slow"]);
        assert_eq!(parallel.critical_timeline().rows.len(), 2);
        assert!(critical_path(&spec(vec![]).timeline).is_empty());
    }

    #[test]
    fn validates_rows_and_path() {
        let mut trace = sample();
        trace.validate().unwrap();
        assert_eq!(trace.duration(), 100.0);
        trace.critical_path.push("nope".into());
        assert!(trace.validate().unwrap_err().to_string().contains("nope"));
        let mut two = sample();
        two.timeline.rows[1].spans.clear();
        assert!(
            two.validate()
                .unwrap_err()
                .to_string()
                .contains("exactly one")
        );
        let mut count = sample();
        count.total_spans = 1;
        assert!(count.validate().is_err());
    }

    #[test]
    fn names_carry_the_service() {
        let mut trace = sample();
        trace.timeline.rows[2].spans[0]
            .attrs
            .insert(SERVICE.into(), "db".into());
        trace.timeline.rows[2].spans[0].status = Some(SpanStatus::Error);
        assert_eq!(trace.span_name(2), "db: b");
        assert_eq!(trace.span_name(1), "a");
        assert_eq!(trace.errors(), [2]);
    }
}
