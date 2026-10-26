//! A Gantt chart scheduled by the critical path method (`show_gantt`).
//!
//! The engine reads tasks with durations (in working days), dependencies
//! (finish-to-start, start-to-start, finish-to-finish, start-to-finish,
//! each with a lag) and optional "not before" dates, runs the forward and
//! backward passes and fills in every task's early and late start and
//! finish, total and free slack and whether it is critical, with the
//! project duration in a [`Computed`] envelope. Times are working-day
//! offsets from the project start; [`Calendar`] maps them to dates when
//! the project has a start date. [`GanttSpec::timeline`] draws it on the
//! timeline widget (critical bars and slack through [`SpanStatus`]) and [`GanttSpec::table`] lists it on the tree table.

use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::block::BlockError;
use crate::computed::Computed;
use crate::widget::timeline::{
    Marker, Span, SpanStatus, TimeUnit, TimelineRow, TimelineSpec, civil_from_days,
};
use crate::widget::tree::{BadgeTone, ColumnKind, TreeColumn, TreeNode, TreeSpec};

/// Most tasks scheduled.
pub const MAX_TASKS: usize = 5_000;
/// Slack below this (days) counts as none.
pub const SLACK_EPSILON: f64 = 1e-9;
/// Seconds in a day.
const DAY: f64 = 86_400.0;

/// How a task waits on another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DependencyKind {
    /// It starts after the other finishes (`FS`, the default).
    #[default]
    Fs,
    /// It starts after the other starts (`SS`).
    Ss,
    /// It finishes after the other finishes (`FF`).
    Ff,
    /// It finishes after the other starts (`SF`).
    Sf,
}

impl DependencyKind {
    /// `FS`, `SS`, `FF` or `SF`.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Fs => "FS",
            Self::Ss => "SS",
            Self::Ff => "FF",
            Self::Sf => "SF",
        }
    }

    /// The kind a code names, any case.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code.to_ascii_uppercase().as_str() {
            "FS" => Some(Self::Fs),
            "SS" => Some(Self::Ss),
            "FF" => Some(Self::Ff),
            "SF" => Some(Self::Sf),
            _ => None,
        }
    }
}

/// A link to a predecessor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Dependency {
    /// Id of the predecessor.
    pub task: String,
    /// How this task waits on it.
    #[serde(default)]
    pub kind: DependencyKind,
    /// Days added to the link (negative for a lead).
    #[serde(default)]
    pub lag: f64,
}

impl Dependency {
    /// Reads `A`, `A SS`, `A FS+2`, `A FF-1d`, `A:SS+2` or `A+3` (a lag on
    /// finish-to-start).
    ///
    /// # Errors
    ///
    /// Why `text` is not a dependency.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        // The id runs to the first blank, `:` or `+` (ids may hold `-`).
        let at = text.find([' ', ':', '+', '\t']).unwrap_or(text.len());
        let (task, rest) = (&text[..at], text[at..].trim_start_matches([' ', ':', '\t']));
        if task.is_empty() {
            return Err(format!("dependency {text:?} names no task"));
        }
        let (kind, lag) = match rest.get(..2).and_then(DependencyKind::from_code) {
            Some(kind) => (kind, rest[2..].trim()),
            None => (DependencyKind::Fs, rest.trim()),
        };
        let lag = lag.trim_end_matches(['d', 'D']).trim();
        let lag = if lag.is_empty() {
            0.0
        } else {
            lag.trim_start_matches('+')
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| {
                    format!("dependency {text:?}: write it as `A`, `A SS`, `A FS+2` or `A FF-1`")
                })?
        };
        Ok(Self {
            task: task.to_owned(),
            kind,
            lag,
        })
    }

    /// `A`, `A SS`, `A FS+2`, `A FF-1`.
    #[must_use]
    pub fn text(&self) -> String {
        let mut out = self.task.clone();
        if self.kind != DependencyKind::Fs || self.lag != 0.0 {
            let _ = write!(out, " {}", self.kind.code());
        }
        if self.lag != 0.0 {
            let _ = write!(
                out,
                "{}{}",
                if self.lag > 0.0 { "+" } else { "" },
                days(self.lag)
            );
        }
        out
    }
}

/// Which days count as working days.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Calendar {
    /// Every day.
    #[default]
    AllDays,
    /// Monday to Friday.
    Weekdays,
}

impl Calendar {
    /// Its name in the spec.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::AllDays => "all_days",
            Self::Weekdays => "weekdays",
        }
    }

    /// Whether day `day` (days since 1970-01-01) is a working day.
    #[must_use]
    pub fn works(self, day: i64) -> bool {
        // 1970-01-01 was a Thursday: weekday 0 = Monday.
        let weekday = (day + 3).rem_euclid(7);
        self == Self::AllDays || weekday < 5
    }

    /// The first working day at or after `day`.
    #[must_use]
    pub fn next_working(self, day: i64) -> i64 {
        (day..day + 7).find(|d| self.works(*d)).unwrap_or(day)
    }

    /// The calendar day (days since 1970-01-01, with the fraction of the
    /// day) at working-day offset `offset` from `start` (a working day).
    #[must_use]
    pub fn date_at(self, start: i64, offset: f64) -> f64 {
        if self == Self::AllDays {
            #[allow(clippy::cast_precision_loss)]
            return start as f64 + offset;
        }
        let whole = offset.floor();
        let fraction = offset - whole;
        // A whole number of weeks first, then day by day.
        #[allow(clippy::cast_possible_truncation)]
        let mut left = whole as i64;
        let mut day = start + left.div_euclid(5) * 7;
        left = left.rem_euclid(5);
        while left > 0 {
            day += 1;
            if self.works(day) {
                left -= 1;
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let at = day as f64;
        at + fraction
    }

    /// Working days from `start` to `day` (negative before it).
    #[must_use]
    pub fn offset_of(self, start: i64, day: i64) -> f64 {
        let (lo, hi, sign) = if day >= start {
            (start, day, 1.0)
        } else {
            (day, start, -1.0)
        };
        let count = (lo..hi).filter(|d| self.works(*d)).count();
        #[allow(clippy::cast_precision_loss)]
        let n = count as f64;
        sign * n
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
#[must_use]
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The day count of `YYYY-MM-DD` (a time after it, `T…`, is ignored).
#[must_use]
pub fn parse_date(text: &str) -> Option<i64> {
    let date = text.trim().get(..10)?;
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || parts.next().is_some() {
        return None;
    }
    let days = days_from_civil(year, month, day);
    // 31 February and the like do not round-trip.
    (civil_from_days(days) == (year, month, day)).then_some(days)
}

/// `YYYY-MM-DD` of a day count.
#[must_use]
pub fn format_date(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// A day count in days: `3`, `2.5`.
#[must_use]
pub fn days(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_owned();
    if s == "-0" { "0".to_owned() } else { s }
}

/// One task with what the engine computed for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GanttTask {
    /// Unique id, referenced by dependencies.
    pub id: String,
    /// Name shown; the id when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Phase or work package the task belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Duration in working days (0 for a milestone).
    pub duration: f64,
    /// Whether it is a milestone (zero duration).
    #[serde(default)]
    pub milestone: bool,
    /// Its predecessors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<Dependency>,
    /// It may not start before this offset (from a start date).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<f64>,
    /// Early start (forward pass).
    pub early_start: f64,
    /// Early finish.
    pub early_finish: f64,
    /// Late start (backward pass).
    pub late_start: f64,
    /// Late finish.
    pub late_finish: f64,
    /// How long it can slip without moving the project finish.
    pub total_slack: f64,
    /// How long it can slip without moving any successor.
    pub free_slack: f64,
    /// On the critical path (no total slack).
    pub critical: bool,
}

impl GanttTask {
    /// The name, or the id.
    #[must_use]
    pub fn label(&self) -> &str {
        self.name
            .as_deref()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(&self.id)
    }
}

/// A scheduled project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GanttSpec {
    /// Title shown above the chart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The workspace file read, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Project start as `YYYY-MM-DD`; offsets only when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    /// Which days are working days.
    #[serde(default)]
    pub calendar: Calendar,
    /// The tasks in input order.
    pub tasks: Vec<GanttTask>,
    /// Working days from the start to the last finish.
    pub project_duration: f64,
    /// Ids of the critical tasks, by early start.
    #[serde(default)]
    pub critical_path: Vec<String>,
    /// The schedule as an envelope.
    pub schedule: Computed,
    /// Reader and scheduler notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

fn invalid(reason: impl Into<String>) -> BlockError {
    BlockError::Invalid {
        kind: "gantt",
        reason: reason.into(),
    }
}

impl GanttSpec {
    /// Checks sizes, ids, references and that every time is finite.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.tasks.is_empty() {
            return Err(invalid("the project has no tasks"));
        }
        if self.tasks.len() > MAX_TASKS {
            return Err(invalid(format!(
                "{} tasks; at most {MAX_TASKS}",
                self.tasks.len()
            )));
        }
        let mut ids = std::collections::HashSet::new();
        for t in &self.tasks {
            if t.id.trim().is_empty() {
                return Err(invalid("a task id is empty"));
            }
            if !ids.insert(t.id.as_str()) {
                return Err(invalid(format!("task id {:?} appears twice", t.id)));
            }
        }
        for t in &self.tasks {
            let times = [
                t.duration,
                t.early_start,
                t.early_finish,
                t.late_start,
                t.late_finish,
                t.total_slack,
                t.free_slack,
            ];
            if times.iter().any(|v| !v.is_finite()) || t.duration < 0.0 {
                return Err(invalid(format!("task {:?} has a bad time", t.id)));
            }
            if let Some(d) = t.depends_on.iter().find(|d| !ids.contains(d.task.as_str())) {
                return Err(invalid(format!(
                    "task {:?} depends on unknown task {:?}",
                    t.id, d.task
                )));
            }
        }
        if let Some(start) = &self.start
            && parse_date(start).is_none()
        {
            return Err(invalid(format!("start {start:?} is not a YYYY-MM-DD date")));
        }
        self.schedule
            .validate()
            .map_err(|e| invalid(format!("schedule envelope: {e}")))
    }

    /// The project start as a day count, when it has a date.
    #[must_use]
    pub fn start_day(&self) -> Option<i64> {
        self.start.as_deref().and_then(parse_date)
    }

    /// The calendar day (with fraction) of offset `t`, when the project
    /// has a start date.
    #[must_use]
    pub fn day_at(&self, t: f64) -> Option<f64> {
        self.start_day().map(|s| self.calendar.date_at(s, t))
    }

    /// `YYYY-MM-DD` of the day a task ending at offset `t` ends on (the
    /// last working day it uses), or `day N` without a start date.
    #[must_use]
    pub fn finish_text(&self, t: f64) -> String {
        match self.start_day() {
            Some(start) if t > 0.0 => {
                // The task occupies the working day before offset t.
                let day = self.calendar.date_at(start, (t - 1.0).max(0.0));
                #[allow(clippy::cast_possible_truncation)]
                format_date(day.floor() as i64)
            }
            Some(start) => format_date(start),
            None => format!("day {}", days(t)),
        }
    }

    /// `YYYY-MM-DD` of the day offset `t` starts on, or `day N`.
    #[must_use]
    pub fn start_text(&self, t: f64) -> String {
        match self.day_at(t) {
            #[allow(clippy::cast_possible_truncation)]
            Some(day) => format_date(day.floor() as i64),
            None => format!("day {}", days(t)),
        }
    }

    /// The milestones.
    pub fn milestones(&self) -> impl Iterator<Item = &GanttTask> {
        self.tasks.iter().filter(|t| t.milestone)
    }

    /// The display title, the file name or `Project`.
    #[must_use]
    pub fn display_title(&self) -> String {
        if let Some(t) = self.title.as_ref().filter(|t| !t.trim().is_empty()) {
            return t.clone();
        }
        self.path
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .map_or_else(|| "Project".to_owned(), str::to_owned)
    }

    /// The tasks on the timeline widget: a bar from early start to early
    /// finish (critical ones marked), the total slack after it, and
    /// milestones as markers. Calendar dates when the project has a start
    /// date (seconds since the epoch), else days from the start.
    #[must_use]
    pub fn timeline(&self) -> TimelineSpec {
        let dated = self.start_day().is_some();
        let time = |t: f64| match self.day_at(t) {
            Some(day) => day * DAY,
            None => t * DAY,
        };
        let rows = self
            .tasks
            .iter()
            .map(|t| {
                let mut attrs = Map::new();
                attrs.insert("duration (d)".into(), json!(t.duration));
                attrs.insert("early start".into(), json!(self.start_text(t.early_start)));
                attrs.insert(
                    "early finish".into(),
                    json!(self.finish_text(t.early_finish)),
                );
                attrs.insert("late start".into(), json!(self.start_text(t.late_start)));
                attrs.insert("late finish".into(), json!(self.finish_text(t.late_finish)));
                attrs.insert("total slack (d)".into(), json!(t.total_slack));
                attrs.insert("free slack (d)".into(), json!(t.free_slack));
                if !t.depends_on.is_empty() {
                    let deps: Vec<String> = t.depends_on.iter().map(Dependency::text).collect();
                    attrs.insert("after".into(), Value::String(deps.join(", ")));
                }
                let mut spans = Vec::new();
                let mut markers = Vec::new();
                if t.milestone {
                    markers.push(Marker {
                        at: time(t.early_start),
                        label: Some(format!(
                            "◆ {} · {}",
                            t.label(),
                            self.start_text(t.early_start)
                        )),
                        color_key: Some(if t.critical { "critical" } else { "milestone" }.into()),
                    });
                    // A zero-length bar carries the tooltip and selection.
                    spans.push(Span {
                        start: time(t.early_start),
                        end: time(t.early_start),
                        label: Some(format!("◆ {}", t.label())),
                        color_key: None,
                        status: t.critical.then_some(SpanStatus::Critical),
                        attrs: attrs.clone(),
                    });
                } else {
                    spans.push(Span {
                        start: time(t.early_start),
                        end: time(t.early_finish),
                        label: Some(t.label().to_owned()),
                        color_key: t.group.clone(),
                        status: t.critical.then_some(SpanStatus::Critical),
                        attrs: attrs.clone(),
                    });
                }
                if t.total_slack > SLACK_EPSILON {
                    spans.push(Span {
                        start: time(t.early_finish),
                        end: time(t.late_finish),
                        label: Some(format!("slack {} d", days(t.total_slack))),
                        color_key: None,
                        status: Some(SpanStatus::Slack),
                        attrs,
                    });
                }
                TimelineRow {
                    id: t.id.clone(),
                    label: t.label().to_owned(),
                    depth: 0,
                    group: t.group.clone(),
                    spans,
                    markers,
                }
            })
            .collect();
        TimelineSpec {
            title: Some(self.display_title()),
            unit: if dated { TimeUnit::Date } else { TimeUnit::S },
            rows,
            range: None,
        }
    }

    /// The schedule as a table: one row per task with its duration, early
    /// and late start and finish, total and free slack.
    #[must_use]
    pub fn table(&self) -> TreeSpec {
        let col = |name: &str, kind: ColumnKind| TreeColumn {
            name: name.to_owned(),
            kind,
            unit: None,
        };
        let columns = vec![
            col("duration (d)", ColumnKind::Number),
            col("ES", ColumnKind::Text),
            col("EF", ColumnKind::Text),
            col("LS", ColumnKind::Text),
            col("LF", ColumnKind::Text),
            col("total slack (d)", ColumnKind::Number),
            col("free slack (d)", ColumnKind::Number),
        ];
        let nodes = self
            .tasks
            .iter()
            .map(|t| {
                let cells = [
                    json!(t.duration),
                    json!(self.start_text(t.early_start)),
                    json!(self.finish_text(t.early_finish)),
                    json!(self.start_text(t.late_start)),
                    json!(self.finish_text(t.late_finish)),
                    json!(t.total_slack),
                    json!(t.free_slack),
                ];
                TreeNode {
                    label: t.label().to_owned(),
                    badge: if t.critical {
                        Some("critical".into())
                    } else if t.milestone {
                        Some("milestone".into())
                    } else {
                        None
                    },
                    tone: t.critical.then_some(BadgeTone::Error),
                    values: columns.iter().map(|c| c.name.clone()).zip(cells).collect(),
                    children: Vec::new(),
                }
            })
            .collect();
        TreeSpec {
            title: Some("Schedule".into()),
            columns,
            nodes,
            expand_depth: 1,
        }
    }

    /// The schedule as CSV, offsets in working days.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out = String::from(
            "id,name,duration,depends_on,early_start,early_finish,late_start,late_finish,\
             total_slack,free_slack,critical\n",
        );
        for t in &self.tasks {
            let deps: Vec<String> = t.depends_on.iter().map(Dependency::text).collect();
            let _ = writeln!(
                out,
                "{},{},{},{},{},{},{},{},{},{},{}",
                csv_field(&t.id),
                csv_field(t.name.as_deref().unwrap_or("")),
                t.duration,
                csv_field(&deps.join(";")),
                t.early_start,
                t.early_finish,
                t.late_start,
                t.late_finish,
                t.total_slack,
                t.free_slack,
                t.critical
            );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computed::Engine;

    fn task(id: &str, d: f64, es: f64, ts: f64) -> GanttTask {
        GanttTask {
            id: id.into(),
            name: None,
            group: None,
            duration: d,
            milestone: d == 0.0,
            depends_on: Vec::new(),
            not_before: None,
            early_start: es,
            early_finish: es + d,
            late_start: es + ts,
            late_finish: es + d + ts,
            total_slack: ts,
            free_slack: ts,
            critical: ts == 0.0,
        }
    }

    fn spec() -> GanttSpec {
        let mut b = task("B", 2.0, 3.0, 1.0);
        b.depends_on.push(Dependency {
            task: "A".into(),
            kind: DependencyKind::Fs,
            lag: 0.0,
        });
        GanttSpec {
            title: None,
            path: None,
            start: Some("2026-11-02".into()),
            calendar: Calendar::Weekdays,
            tasks: vec![task("A", 3.0, 0.0, 0.0), b, task("M", 0.0, 6.0, 0.0)],
            project_duration: 6.0,
            critical_path: vec!["A".into(), "M".into()],
            schedule: Computed::new(
                "pl_1",
                "plan.cpm",
                "CPM",
                Engine {
                    name: "crest-plan".into(),
                    version: "0".into(),
                    backend: None,
                    platform: None,
                },
            ),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn dates_and_weekday_calendars() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2024, 2, 29), 19_782);
        assert_eq!(parse_date("2026-11-02"), Some(days_from_civil(2026, 11, 2)));
        assert_eq!(parse_date("2026-02-31"), None);
        assert_eq!(parse_date("2026-11-02T09:00:00Z"), parse_date("2026-11-02"));
        let monday = parse_date("2026-11-02").unwrap();
        let cal = Calendar::Weekdays;
        assert!(cal.works(monday) && !cal.works(monday + 5));
        assert_eq!(cal.next_working(monday + 5), monday + 7);
        // Five working days from a Monday is the next Monday.
        assert_eq!(cal.date_at(monday, 5.0), (monday + 7) as f64);
        assert_eq!(cal.date_at(monday, 7.5), (monday + 9) as f64 + 0.5);
        assert_eq!(cal.offset_of(monday, monday + 9), 7.0);
        assert_eq!(Calendar::AllDays.date_at(monday, 3.0), (monday + 3) as f64);
        assert_eq!(format_date(monday), "2026-11-02");
    }

    #[test]
    fn timeline_table_and_csv() {
        let s = spec();
        s.validate().unwrap();
        // A (3 days from Monday) ends on Wednesday; B starts Thursday.
        assert_eq!(s.finish_text(3.0), "2026-11-04");
        assert_eq!(s.start_text(3.0), "2026-11-05");
        let t = s.timeline();
        assert_eq!(t.unit, TimeUnit::Date);
        assert_eq!(t.rows[0].spans[0].status, Some(SpanStatus::Critical));
        assert_eq!(t.rows[1].spans.len(), 2, "B has a slack bar");
        assert_eq!(t.rows[1].spans[1].label.as_deref(), Some("slack 1 d"));
        assert_eq!(t.rows[2].markers.len(), 1);
        t.validate().unwrap();
        let table = s.table();
        assert_eq!(table.nodes[0].badge.as_deref(), Some("critical"));
        assert_eq!(table.columns.len(), 7);
        assert!(
            s.to_csv().contains("\nB,,2,A,3,5,4,6,1,1,false\n"),
            "{}",
            s.to_csv()
        );
        let mut undated = spec();
        undated.start = None;
        assert_eq!(undated.timeline().unit, TimeUnit::S);
        assert_eq!(undated.start_text(3.0), "day 3");
        let lagged = Dependency {
            task: "A".into(),
            kind: DependencyKind::Ss,
            lag: 2.0,
        };
        assert_eq!(lagged.text(), "A SS+2");
        for (text, kind, lag) in [
            ("A", DependencyKind::Fs, 0.0),
            ("A SS", DependencyKind::Ss, 0.0),
            ("A FS+2", DependencyKind::Fs, 2.0),
            ("A ff -1d", DependencyKind::Ff, -1.0),
            ("A:SF+0.5", DependencyKind::Sf, 0.5),
            ("A+3", DependencyKind::Fs, 3.0),
            ("design-1 SS+2", DependencyKind::Ss, 2.0),
        ] {
            let d = Dependency::parse(text).unwrap();
            assert_eq!(
                (d.task.as_str(), d.kind, d.lag),
                (text.split([' ', ':', '+']).next().unwrap(), kind, lag),
                "{text}"
            );
        }
        assert!(Dependency::parse("A XX").is_err());
        assert!(Dependency::parse("").is_err());
        let mut bad = spec();
        bad.tasks[1].depends_on[0].task = "Z".into();
        assert!(
            bad.validate()
                .unwrap_err()
                .to_string()
                .contains("unknown task")
        );
    }
}
