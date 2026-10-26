//! A timeline: rows of spans (and point markers) on one time axis.
//!
//! Traces, Gantt charts, HAR waterfalls and waveforms are all rows of
//! `[start, end]` intervals. Times are numbers in the spec's [`TimeUnit`];
//! with [`TimeUnit::Date`] they are seconds since the Unix epoch (UTC). A
//! row's `depth` indents it under the row above (a trace tree), and
//! consecutive rows with the same `group` are drawn as one band.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{invalid, unique};
use crate::block::BlockError;

/// Rows a timeline may have.
pub const MAX_ROWS: usize = 100_000;
/// Spans (all rows together) a timeline may have.
pub const MAX_SPANS: usize = 1_000_000;
/// Deepest indentation of a row.
pub const MAX_DEPTH: u32 = 64;

const KIND: &str = "timeline";

/// Unit of every time in a [`TimelineSpec`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimeUnit {
    /// Nanoseconds.
    Ns,
    /// Microseconds.
    Us,
    /// Milliseconds.
    #[default]
    Ms,
    /// Seconds.
    S,
    /// Calendar time: seconds since 1970-01-01T00:00:00Z.
    Date,
}

impl TimeUnit {
    /// Seconds in one unit (1 for [`TimeUnit::Date`]).
    #[must_use]
    pub fn seconds(self) -> f64 {
        match self {
            Self::Ns => 1e-9,
            Self::Us => 1e-6,
            Self::Ms => 1e-3,
            Self::S | Self::Date => 1.0,
        }
    }
}

/// How a span ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    /// Finished fine.
    Ok,
    /// Finished with a warning (slow, retried).
    Warning,
    /// Failed.
    Error,
    /// On a critical path (a Gantt chart's critical tasks).
    Critical,
    /// Room a task may slip (a Gantt chart's slack), drawn faint.
    Slack,
}

impl SpanStatus {
    /// Lowercase name, as in the spec.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Critical => "critical",
            Self::Slack => "slack",
        }
    }
}

/// One interval of a row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Span {
    /// Start time.
    pub start: f64,
    /// End time; at least `start` (equal for an instant). `stop` is
    /// accepted too, since `end` is a keyword in Lua.
    #[serde(alias = "stop")]
    pub end: f64,
    /// Text drawn inside the bar when it fits, and in the tooltip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Spans with the same key share a palette color (a service, a kind).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_key: Option<String>,
    /// Outcome; errors and warnings are drawn in their theme colors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SpanStatus>,
    /// Extra fields shown when the span is hovered or selected.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    #[schemars(with = "std::collections::BTreeMap<String, Value>")]
    pub attrs: Map<String, Value>,
}

impl Span {
    /// `end - start`.
    #[must_use]
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }
}

/// A point in time drawn as a tick across its row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Marker {
    /// When.
    pub at: f64,
    /// What happened (shown in the tooltip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Palette key, as for spans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_key: Option<String>,
}

/// One lane of the timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TimelineRow {
    /// Unique id of the row (a span id, a signal path).
    pub id: String,
    /// Text in the label column.
    pub label: String,
    /// Indentation level (0 = top level), for trees of spans.
    #[serde(default)]
    pub depth: u32,
    /// Consecutive rows of the same group are banded together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Intervals of the row.
    #[serde(default)]
    pub spans: Vec<Span>,
    /// Instants of the row.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
}

/// A closed time interval.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TimeRange {
    /// First time shown.
    pub start: f64,
    /// Last time shown; greater than `start`.
    pub end: f64,
}

/// Rows of spans on a shared time axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TimelineSpec {
    /// Caption above the timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Unit of every time.
    #[serde(default)]
    pub unit: TimeUnit,
    /// Lanes, top to bottom.
    pub rows: Vec<TimelineRow>,
    /// Axis range at first; the extent of the data when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<TimeRange>,
}

impl TimelineSpec {
    /// Checks row ids, sizes, depths and that every time is finite with
    /// `start <= end`.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.rows.len() > MAX_ROWS {
            return Err(invalid(
                KIND,
                format!("{} rows; at most {MAX_ROWS} can be drawn", self.rows.len()),
            ));
        }
        let spans = self.span_count();
        if spans > MAX_SPANS {
            return Err(invalid(
                KIND,
                format!("{spans} spans; at most {MAX_SPANS} can be drawn"),
            ));
        }
        unique(KIND, "row id", self.rows.iter().map(|r| r.id.as_str()))?;
        for row in &self.rows {
            if row.depth > MAX_DEPTH {
                return Err(invalid(
                    KIND,
                    format!(
                        "row {:?} is nested {} deep (max {MAX_DEPTH})",
                        row.id, row.depth
                    ),
                ));
            }
            for (ix, span) in row.spans.iter().enumerate() {
                if !span.start.is_finite() || !span.end.is_finite() {
                    return Err(invalid(
                        KIND,
                        format!("span {} of row {:?} has a non-finite time", ix + 1, row.id),
                    ));
                }
                if span.end < span.start {
                    return Err(invalid(
                        KIND,
                        format!("span {} of row {:?} ends before it starts", ix + 1, row.id),
                    ));
                }
            }
            if let Some(ix) = row.markers.iter().position(|m| !m.at.is_finite()) {
                return Err(invalid(
                    KIND,
                    format!(
                        "marker {} of row {:?} has a non-finite time",
                        ix + 1,
                        row.id
                    ),
                ));
            }
        }
        if let Some(range) = self.range
            && !(range.start.is_finite() && range.end.is_finite() && range.start < range.end)
        {
            return Err(invalid(KIND, "range must be finite with start < end"));
        }
        Ok(())
    }

    /// Spans of all rows.
    #[must_use]
    pub fn span_count(&self) -> usize {
        self.rows.iter().map(|r| r.spans.len()).sum()
    }

    /// The first and last time of the data (spans and markers), or `None`
    /// when there is none.
    #[must_use]
    pub fn data_extent(&self) -> Option<(f64, f64)> {
        let times = self.rows.iter().flat_map(|row| {
            row.spans
                .iter()
                .flat_map(|s| [s.start, s.end])
                .chain(row.markers.iter().map(|m| m.at))
        });
        times
            .filter(|t| t.is_finite())
            .fold(None, |acc, t| match acc {
                None => Some((t, t)),
                Some((lo, hi)) => Some((f64::min(lo, t), f64::max(hi, t))),
            })
    }

    /// The range the axis starts at: `range`, or the data extent widened
    /// when it is a single instant, or `0..1`.
    #[must_use]
    pub fn extent(&self) -> (f64, f64) {
        if let Some(range) = self.range {
            return (range.start, range.end);
        }
        match self.data_extent() {
            Some((lo, hi)) if hi > lo => (lo, hi),
            Some((t, _)) => (t - 0.5, t + 0.5),
            None => (0.0, 1.0),
        }
    }

    /// The spans as CSV: `row,label,start,end,duration,status`, times in
    /// the spec's unit.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out = String::from("row,label,start,end,duration,status\n");
        for row in &self.rows {
            for span in &row.spans {
                let label = span.label.as_deref().unwrap_or(&row.label);
                let status = span.status.map(SpanStatus::name).unwrap_or_default();
                out.push_str(&format!(
                    "{},{},{},{},{},{status}\n",
                    csv_field(&row.id),
                    csv_field(label),
                    span.start,
                    span.end,
                    span.duration(),
                ));
            }
        }
        out
    }
}

fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// `n` with at most `decimals` decimals, without trailing zeros.
fn trimmed(n: f64, decimals: usize) -> String {
    let s = format!("{n:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    };
    if s == "-0" { "0".to_owned() } else { s }
}

/// A duration given in `unit`, written in the largest unit that keeps it
/// at least 1: `850 µs`, `1.25 ms`, `3.4 s`, `2 min 5 s`, `1 h 3 min`,
/// `2 d 4 h`.
#[must_use]
pub fn format_duration(value: f64, unit: TimeUnit) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    let seconds = value * unit.seconds();
    let abs = seconds.abs();
    let sign = if seconds < 0.0 { "-" } else { "" };
    if abs == 0.0 {
        return "0".to_owned();
    }
    let small = [(1.0, "s"), (1e-3, "ms"), (1e-6, "µs"), (1e-9, "ns")];
    if abs < 60.0 {
        for (scale, suffix) in small {
            if abs >= scale || suffix == "ns" {
                let scaled = abs / scale;
                let decimals = if scaled >= 100.0 {
                    0
                } else if scaled >= 10.0 {
                    1
                } else {
                    2
                };
                return format!("{sign}{} {suffix}", trimmed(scaled, decimals));
            }
        }
    }
    let total = abs.round() as u64;
    let (d, h, m, s) = (
        total / 86_400,
        total % 86_400 / 3_600,
        total % 3_600 / 60,
        total % 60,
    );
    let text = if d > 0 {
        if h > 0 {
            format!("{d} d {h} h")
        } else {
            format!("{d} d")
        }
    } else if h > 0 {
        if m > 0 {
            format!("{h} h {m} min")
        } else {
            format!("{h} h")
        }
    } else if s > 0 {
        format!("{m} min {s} s")
    } else {
        format!("{m} min")
    };
    format!("{sign}{text}")
}

/// Year, month (1–12) and day (1–31).
pub type CivilDate = (i64, u32, u32);
/// Hour, minute, second and the fraction of the second.
pub type ClockTime = (u32, u32, u32, f64);

/// Year, month (1–12) and day (1–31) of a day count since 1970-01-01
/// (proleptic Gregorian; Howard Hinnant's `civil_from_days`).
#[must_use]
pub fn civil_from_days(days: i64) -> CivilDate {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Calendar parts of a Unix time in seconds (UTC): date, then hour,
/// minute, second and the fraction of the second.
#[must_use]
pub fn date_parts(epoch_seconds: f64) -> (CivilDate, ClockTime) {
    let whole = epoch_seconds.floor();
    let fraction = epoch_seconds - whole;
    let whole = whole as i64;
    let days = whole.div_euclid(86_400);
    let secs = whole.rem_euclid(86_400) as u32;
    (
        civil_from_days(days),
        (secs / 3_600, secs % 3_600 / 60, secs % 60, fraction),
    )
}

/// A Unix time as `2026-10-24 13:05:07` (UTC), with milliseconds when it
/// has a fraction.
#[must_use]
pub fn format_date_time(epoch_seconds: f64) -> String {
    if !epoch_seconds.is_finite() {
        return epoch_seconds.to_string();
    }
    let ((y, mo, d), (h, mi, s, fraction)) = date_parts(epoch_seconds);
    let millis = (fraction * 1_000.0).round() as u32;
    if millis == 0 || millis == 1_000 {
        format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
    } else {
        format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{millis:03}")
    }
}

/// A time of `unit` for tooltips: a date and time for [`TimeUnit::Date`],
/// a duration from zero otherwise.
#[must_use]
pub fn format_time(value: f64, unit: TimeUnit) -> String {
    match unit {
        TimeUnit::Date => format_date_time(value),
        _ => format_duration(value, unit),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn span(start: f64, end: f64) -> Span {
        Span {
            start,
            end,
            label: None,
            color_key: None,
            status: None,
            attrs: Map::new(),
        }
    }

    fn row(id: &str, spans: Vec<Span>) -> TimelineRow {
        TimelineRow {
            id: id.into(),
            label: id.into(),
            depth: 0,
            group: None,
            spans,
            markers: Vec::new(),
        }
    }

    fn spec(rows: Vec<TimelineRow>) -> TimelineSpec {
        TimelineSpec {
            title: None,
            unit: TimeUnit::Ms,
            rows,
            range: None,
        }
    }

    #[test]
    fn parses_with_defaults() {
        let spec: TimelineSpec = serde_json::from_value(json!({
            "rows": [{ "id": "a", "label": "GET /", "spans": [
                { "start": 0, "end": 12.5, "status": "error", "attrs": { "http.status": 500 } }
            ] }]
        }))
        .unwrap();
        assert_eq!(spec.unit, TimeUnit::Ms);
        assert_eq!(spec.rows[0].depth, 0);
        assert_eq!(spec.rows[0].spans[0].status, Some(SpanStatus::Error));
        assert_eq!(spec.rows[0].spans[0].attrs["http.status"], 500);
        spec.validate().unwrap();
    }

    #[test]
    fn rejects_bad_times_and_duplicate_ids() {
        let reversed = spec(vec![row("a", vec![span(5.0, 1.0)])]);
        assert!(
            reversed
                .validate()
                .unwrap_err()
                .to_string()
                .contains("ends before")
        );
        let nan = spec(vec![row("a", vec![span(f64::NAN, 1.0)])]);
        assert!(
            nan.validate()
                .unwrap_err()
                .to_string()
                .contains("non-finite")
        );
        let twice = spec(vec![row("a", vec![]), row("a", vec![])]);
        assert!(
            twice
                .validate()
                .unwrap_err()
                .to_string()
                .contains("appears twice")
        );
        let mut deep = spec(vec![row("a", vec![])]);
        deep.rows[0].depth = MAX_DEPTH + 1;
        assert!(deep.validate().is_err());
        let mut range = spec(vec![]);
        range.range = Some(TimeRange {
            start: 2.0,
            end: 2.0,
        });
        assert!(range.validate().is_err());
    }

    #[test]
    fn extent_covers_spans_and_markers() {
        let mut timeline = spec(vec![
            row("a", vec![span(2.0, 5.0)]),
            row("b", vec![span(1.0, 3.0)]),
        ]);
        timeline.rows[1].markers.push(Marker {
            at: 9.0,
            label: None,
            color_key: None,
        });
        assert_eq!(timeline.extent(), (1.0, 9.0));
        assert_eq!(spec(vec![]).extent(), (0.0, 1.0));
        let instant = spec(vec![row("a", vec![span(4.0, 4.0)])]);
        assert_eq!(instant.extent(), (3.5, 4.5));
        timeline.range = Some(TimeRange {
            start: 0.0,
            end: 20.0,
        });
        assert_eq!(timeline.extent(), (0.0, 20.0));
    }

    #[test]
    fn csv_lists_every_span() {
        let mut timeline = spec(vec![row("a,b", vec![span(0.0, 1.5), span(2.0, 4.0)])]);
        timeline.rows[0].spans[1].label = Some("db".into());
        timeline.rows[0].spans[1].status = Some(SpanStatus::Error);
        assert_eq!(
            timeline.to_csv(),
            "row,label,start,end,duration,status\n\"a,b\",\"a,b\",0,1.5,1.5,\n\"a,b\",db,2,4,2,error\n"
        );
    }

    #[test]
    fn durations_pick_a_readable_unit() {
        assert_eq!(format_duration(0.85, TimeUnit::Ms), "850 µs");
        assert_eq!(format_duration(1.25, TimeUnit::Ms), "1.25 ms");
        assert_eq!(format_duration(3_400.0, TimeUnit::Ms), "3.4 s");
        assert_eq!(format_duration(125.0, TimeUnit::S), "2 min 5 s");
        assert_eq!(format_duration(3_780.0, TimeUnit::S), "1 h 3 min");
        assert_eq!(format_duration(187_200.0, TimeUnit::Date), "2 d 4 h");
        assert_eq!(format_duration(12.0, TimeUnit::Ns), "12 ns");
        assert_eq!(format_duration(0.0, TimeUnit::Ms), "0");
        assert_eq!(format_duration(-2.0, TimeUnit::S), "-2 s");
    }

    #[test]
    fn dates_are_utc_calendar_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(format_date_time(1_761_311_107.0), "2025-10-24 13:05:07");
        assert_eq!(format_date_time(1.5), "1970-01-01 00:00:01.500");
        assert_eq!(format_time(1.5, TimeUnit::S), "1.5 s");
    }
}
