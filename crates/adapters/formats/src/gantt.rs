//! Gantt charts for `show_gantt`: tasks read from CSV, JSON or the call,
//! scheduled by the critical path method.
//!
//! Durations and lags are working days. The forward pass gives each task
//! its early start (the latest its predecessors allow: FS = their early
//! finish + lag, SS = their early start + lag, FF = their early finish +
//! lag - duration, SF = their early start + lag - duration, and at least
//! its "not before" offset and the project start) and early finish; the
//! project finishes at the latest early finish. The backward pass gives
//! the late finish (the earliest its successors allow, mirrored) and late
//! start. Total slack is late start - early start, free slack the room
//! before the nearest successor constraint (or the project finish), and a
//! task with no total slack is critical. A dependency cycle is an error
//! naming the tasks on it. Offsets become dates through the calendar
//! (every day, or Monday to Friday) when the project has a start date.

use std::collections::{BTreeMap, HashMap};

use application::csv::{delimiter_for, records};
use application::ports::formats::{GanttRequest, GanttSource, GanttTaskInput};
use domain::computed::{Computed, ComputedValue};
use domain::gantt::{
    Dependency, DependencyKind, GanttSpec, GanttTask, MAX_TASKS, SLACK_EPSILON, format_date,
    parse_date,
};
use domain::science::molecule_grid::value_key;
use serde_json::{Map, Value};

use crate::{chem, sci};

/// Envelope kind of a schedule.
pub const KIND: &str = "plan.cpm";
/// Engine name of the scheduler.
pub const PLAN_ENGINE_NAME: &str = "crest-plan";
/// What computes it.
pub const BACKEND: &str = "crest CPM (forward and backward pass)";
/// Tasks whose own times go into the envelope.
pub const ENVELOPE_TASKS: usize = 200;

/// The column names (lowercase) each field is read from.
const ID: &[&str] = &["id", "key", "wbs", "task_id"];
const NAME: &[&str] = &["name", "task", "title", "task_name", "summary"];
const GROUP: &[&str] = &["group", "phase", "section", "package", "category"];
const DURATION: &[&str] = &["duration", "days", "duration_days", "dur", "length"];
const START: &[&str] = &["start", "start_date", "begin", "not_before"];
const FINISH: &[&str] = &["finish", "end", "end_date", "finish_date", "due"];
const DEPENDS: &[&str] = &[
    "depends_on",
    "depends",
    "dependencies",
    "predecessors",
    "deps",
    "after",
];
const MILESTONE: &[&str] = &["milestone", "is_milestone"];

fn field<'a>(row: &'a Map<String, Value>, names: &[&str]) -> Option<&'a Value> {
    row.iter()
        .find(|(k, v)| names.contains(&k.trim().to_ascii_lowercase().as_str()) && !v.is_null())
        .map(|(_, v)| v)
}

fn text_of(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A task from a row of a file (CSV cells as strings, or a JSON object).
fn task_from_row(row: &Map<String, Value>, index: usize) -> Result<GanttTaskInput, String> {
    let name = text_of(field(row, NAME));
    let id = text_of(field(row, ID))
        .or_else(|| name.clone())
        .ok_or_else(|| format!("task {} has no id or name", index + 1))?;
    let duration = match field(row, DURATION) {
        None => None,
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) if s.trim().is_empty() => None,
        Some(Value::String(s)) => Some(
            s.trim()
                .trim_end_matches(['d', 'D'])
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("task {id:?}: duration {s:?} is not a number of days"))?,
        ),
        Some(other) => return Err(format!("task {id:?}: duration {other} is not a number")),
    };
    let depends_on = match field(row, DEPENDS) {
        None => Vec::new(),
        Some(Value::Array(items)) => items.iter().filter_map(|v| text_of(Some(v))).collect(),
        Some(v) => text_of(Some(v))
            .map(|s| {
                s.split([';', ',', '|'])
                    .map(|d| d.trim().to_owned())
                    .filter(|d| !d.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
    };
    let milestone = match field(row, MILESTONE) {
        Some(Value::Bool(b)) => *b,
        Some(v) => text_of(Some(v)).is_some_and(|s| {
            matches!(
                s.to_ascii_lowercase().as_str(),
                "true" | "yes" | "y" | "x" | "1"
            )
        }),
        None => false,
    };
    Ok(GanttTaskInput {
        id,
        name,
        group: text_of(field(row, GROUP)),
        duration,
        start: text_of(field(row, START)),
        finish: text_of(field(row, FINISH)),
        depends_on,
        milestone,
    })
}

/// The tasks of a file: JSON when it starts with `[` or `{`, else CSV or
/// TSV (by the path's extension).
fn read_tasks(bytes: &[u8], path: Option<&str>) -> Result<Vec<GanttTaskInput>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the task file is not UTF-8".to_owned())?;
    let text = text.trim_start_matches('\u{feff}');
    let trimmed = text.trim_start();
    let rows: Vec<Map<String, Value>> = if trimmed.starts_with(['[', '{']) {
        let value: Value = serde_json::from_str(trimmed).map_err(|e| format!("not JSON: {e}"))?;
        let list = match value {
            Value::Array(items) => items,
            Value::Object(mut obj) => match obj.remove("tasks") {
                Some(Value::Array(items)) => items,
                _ => {
                    return Err(
                        "a JSON task file is an array of tasks or {\"tasks\": [...]}".into(),
                    );
                }
            },
            _ => return Err("a JSON task file is an array of tasks".into()),
        };
        list.into_iter()
            .map(|v| match v {
                Value::Object(obj) => Ok(obj),
                other => Err(format!("a task is not an object: {other}")),
            })
            .collect::<Result<_, _>>()?
    } else {
        let delimiter = path.map_or(',', delimiter_for);
        let mut records = records(text, delimiter)?.into_iter();
        let header = records.next().ok_or("the task file is empty")?;
        records
            .map(|record| {
                header
                    .iter()
                    .zip(record)
                    .map(|(k, v)| (k.trim().to_owned(), Value::String(v)))
                    .collect()
            })
            .collect()
    };
    rows.iter()
        .enumerate()
        .map(|(ix, row)| task_from_row(row, ix))
        .collect()
}

/// A task ready to schedule.
struct Planned {
    input: GanttTaskInput,
    duration: f64,
    milestone: bool,
    not_before: Option<f64>,
    deps: Vec<(usize, Dependency)>,
}

/// The order of `tasks` such that every task comes after its
/// predecessors (Kahn), or the tasks of a cycle.
fn order(tasks: &[Planned]) -> Result<Vec<usize>, Vec<usize>> {
    let n = tasks.len();
    let mut indegree = vec![0usize; n];
    let mut successors = vec![Vec::new(); n];
    for (j, t) in tasks.iter().enumerate() {
        for (i, _) in &t.deps {
            indegree[j] += 1;
            successors[*i].push(j);
        }
    }
    let mut ready: Vec<usize> = (0..n).filter(|j| indegree[*j] == 0).rev().collect();
    let mut out = Vec::with_capacity(n);
    while let Some(i) = ready.pop() {
        out.push(i);
        for &j in successors[i].iter().rev() {
            indegree[j] -= 1;
            if indegree[j] == 0 {
                ready.push(j);
            }
        }
    }
    if out.len() == n {
        return Ok(out);
    }
    // Walk predecessors among the unscheduled until one repeats.
    let left: Vec<bool> = (0..n).map(|j| indegree[j] > 0).collect();
    let mut at = (0..n).find(|j| left[*j]).unwrap_or(0);
    let mut seen = vec![usize::MAX; n];
    let mut path = Vec::new();
    while seen[at] == usize::MAX {
        seen[at] = path.len();
        path.push(at);
        at = tasks[at]
            .deps
            .iter()
            .map(|(i, _)| *i)
            .find(|i| left[*i])
            .unwrap_or(at);
    }
    // Predecessor order reversed is flow order; start at the first task
    // given, so the message does not depend on where the walk began.
    let mut cycle = path[seen[at]..].to_vec();
    cycle.reverse();
    if let Some(first) = cycle
        .iter()
        .enumerate()
        .min_by_key(|(_, i)| **i)
        .map(|(k, _)| k)
    {
        cycle.rotate_left(first);
    }
    Err(cycle)
}

/// Schedules `request`.
///
/// # Errors
///
/// A message for unreadable input, bad tasks or a dependency cycle.
pub fn schedule(request: &GanttRequest) -> Result<GanttSpec, String> {
    let (inputs, bytes) = match &request.source {
        GanttSource::File(bytes) => (read_tasks(bytes, request.path.as_deref())?, bytes.clone()),
        GanttSource::Tasks(tasks) => (
            tasks.clone(),
            serde_json::to_vec(tasks).map_err(|e| e.to_string())?,
        ),
    };
    if inputs.is_empty() {
        return Err("there are no tasks".into());
    }
    if inputs.len() > MAX_TASKS {
        return Err(format!("{} tasks; at most {MAX_TASKS}", inputs.len()));
    }
    let calendar = request.calendar;
    let mut warnings = Vec::new();
    let date = |text: &str, what: &str| {
        parse_date(text).ok_or_else(|| format!("{what} {text:?} is not a YYYY-MM-DD date"))
    };
    // The project start: given, else the earliest task start.
    let mut start_day = match request.start.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(s) => Some(date(s, "start")?),
        None => {
            let mut earliest: Option<i64> = None;
            for t in &inputs {
                if let Some(s) = &t.start {
                    let d = date(s, &format!("task {:?} start", t.id))?;
                    earliest = Some(earliest.map_or(d, |e| e.min(d)));
                }
            }
            earliest
        }
    };
    if let Some(day) = start_day
        && !calendar.works(day)
    {
        let next = calendar.next_working(day);
        warnings.push(format!(
            "the project start {} is not a working day; it starts on {}",
            format_date(day),
            format_date(next)
        ));
        start_day = Some(next);
    }
    let index: HashMap<&str, usize> = inputs
        .iter()
        .enumerate()
        .map(|(ix, t)| (t.id.as_str(), ix))
        .collect();
    if index.len() != inputs.len() {
        let mut seen = std::collections::HashSet::new();
        let twice = inputs
            .iter()
            .find(|t| !seen.insert(t.id.as_str()))
            .map_or("", |t| t.id.as_str());
        return Err(format!("task id {twice:?} appears twice"));
    }
    let mut planned = Vec::with_capacity(inputs.len());
    for t in &inputs {
        let starts = t
            .start
            .as_deref()
            .map(|s| date(s, &format!("task {:?} start", t.id)))
            .transpose()?;
        let duration = match (t.duration, starts, &t.finish) {
            (Some(d), ..) => d,
            (None, Some(s), Some(f)) => {
                let f = date(f, &format!("task {:?} finish", t.id))?;
                if f < s {
                    return Err(format!("task {:?} finishes before it starts", t.id));
                }
                // Both days count: start to finish inclusive.
                calendar.offset_of(s, f + 1)
            }
            (None, ..) if t.milestone => 0.0,
            (None, ..) => {
                return Err(format!(
                    "task {:?} has no duration (give `duration`, or `start` and `finish`)",
                    t.id
                ));
            }
        };
        if !(duration.is_finite() && duration >= 0.0) {
            return Err(format!("task {:?} has a negative or bad duration", t.id));
        }
        let milestone = t.milestone || duration == 0.0;
        if t.milestone && duration > 0.0 {
            warnings.push(format!(
                "milestone {:?} had a duration of {duration} d; milestones take none",
                t.id
            ));
        }
        let not_before = match (starts, start_day) {
            (Some(s), Some(p)) => Some(calendar.offset_of(p, calendar.next_working(s))),
            _ => None,
        };
        let mut deps = Vec::new();
        for text in &t.depends_on {
            let dep = Dependency::parse(text).map_err(|e| format!("task {:?}: {e}", t.id))?;
            let Some(&i) = index.get(dep.task.as_str()) else {
                return Err(format!(
                    "task {:?} depends on unknown task {:?}",
                    t.id, dep.task
                ));
            };
            if dep.task == t.id {
                return Err(format!("task {:?} depends on itself", t.id));
            }
            deps.push((i, dep));
        }
        planned.push(Planned {
            input: t.clone(),
            duration: if milestone { 0.0 } else { duration },
            milestone,
            not_before,
            deps,
        });
    }
    let order = order(&planned).map_err(|cycle| {
        let mut names: Vec<&str> = cycle
            .iter()
            .map(|i| planned[*i].input.id.as_str())
            .collect();
        if let Some(first) = names.first().copied() {
            names.push(first);
        }
        format!("the dependencies form a cycle: {}", names.join(" → "))
    })?;
    let n = planned.len();
    let (mut es, mut ef) = (vec![0.0f64; n], vec![0.0f64; n]);
    for &j in &order {
        let t = &planned[j];
        let d = t.duration;
        let mut start = t.not_before.unwrap_or(0.0).max(0.0);
        for (i, dep) in &t.deps {
            let bound = match dep.kind {
                DependencyKind::Fs => ef[*i] + dep.lag,
                DependencyKind::Ss => es[*i] + dep.lag,
                DependencyKind::Ff => ef[*i] + dep.lag - d,
                DependencyKind::Sf => es[*i] + dep.lag - d,
            };
            start = start.max(bound);
        }
        es[j] = start;
        ef[j] = start + d;
    }
    let finish = ef.iter().copied().fold(0.0, f64::max);
    let mut successors: Vec<Vec<(usize, &Dependency)>> = vec![Vec::new(); n];
    for (j, t) in planned.iter().enumerate() {
        for (i, dep) in &t.deps {
            successors[*i].push((j, dep));
        }
    }
    let (mut ls, mut lf) = (vec![0.0f64; n], vec![0.0f64; n]);
    for &i in order.iter().rev() {
        let d = planned[i].duration;
        let mut late = finish;
        for (j, dep) in &successors[i] {
            let bound = match dep.kind {
                DependencyKind::Fs => ls[*j] - dep.lag,
                DependencyKind::Ss => ls[*j] - dep.lag + d,
                DependencyKind::Ff => lf[*j] - dep.lag,
                DependencyKind::Sf => lf[*j] - dep.lag + d,
            };
            late = late.min(bound);
        }
        lf[i] = late;
        ls[i] = late - d;
    }
    let mut tasks = Vec::with_capacity(n);
    for (i, t) in planned.iter().enumerate() {
        let total = clean(ls[i] - es[i]);
        let free = successors[i]
            .iter()
            .map(|(j, dep)| match dep.kind {
                DependencyKind::Fs => es[*j] - dep.lag - ef[i],
                DependencyKind::Ss => es[*j] - dep.lag - es[i],
                DependencyKind::Ff => ef[*j] - dep.lag - ef[i],
                DependencyKind::Sf => ef[*j] - dep.lag - es[i],
            })
            .fold(finish - ef[i], f64::min);
        tasks.push(GanttTask {
            id: t.input.id.clone(),
            name: t.input.name.clone().filter(|n| *n != t.input.id),
            group: t.input.group.clone(),
            duration: t.duration,
            milestone: t.milestone,
            depends_on: t.deps.iter().map(|(_, d)| d.clone()).collect(),
            not_before: t.not_before,
            early_start: clean(es[i]),
            early_finish: clean(ef[i]),
            late_start: clean(ls[i]),
            late_finish: clean(lf[i]),
            total_slack: total,
            free_slack: clean(free).max(0.0),
            critical: total.abs() <= SLACK_EPSILON,
        });
        if total < -SLACK_EPSILON {
            warnings.push(format!(
                "task {:?} has negative slack ({} d): its constraints cannot all be met",
                t.input.id, total
            ));
        }
    }
    let mut critical: Vec<&GanttTask> = tasks.iter().filter(|t| t.critical).collect();
    critical.sort_by(|a, b| {
        a.early_start
            .total_cmp(&b.early_start)
            .then(a.early_finish.total_cmp(&b.early_finish))
    });
    let critical_path: Vec<String> = critical.iter().map(|t| t.id.clone()).collect();
    let start = start_day.map(format_date);
    let mut spec = GanttSpec {
        title: None,
        path: request.path.clone(),
        start,
        calendar,
        tasks,
        project_duration: clean(finish),
        critical_path,
        schedule: Computed::new(
            "pl_pending",
            KIND,
            "Critical path method",
            sci::engine(PLAN_ENGINE_NAME, BACKEND),
        ),
        warnings,
    };
    spec.schedule = envelope(&bytes, request, &spec);
    Ok(spec)
}

/// `v` rounded to 1e-9 days, so sums of fractions print cleanly.
fn clean(v: f64) -> f64 {
    let r = (v * 1e9).round() / 1e9;
    if r == 0.0 { 0.0 } else { r }
}

fn days(v: f64) -> ComputedValue {
    let mut value = ComputedValue::new(v);
    value.unit = Some("d".into());
    value
}

fn envelope(bytes: &[u8], request: &GanttRequest, spec: &GanttSpec) -> Computed {
    let calendar = spec.calendar.name().as_bytes();
    let start = spec.start.as_deref().unwrap_or("").as_bytes();
    let mut computed = Computed::new(
        sci::derive_id("pl", &[KIND.as_bytes(), calendar, start, bytes]),
        KIND,
        "Critical path method",
        sci::engine(PLAN_ENGINE_NAME, BACKEND),
    );
    computed.method.why = Some(
        "Forward pass for early start/finish (FS, SS, FF and SF links with lags, not-before \
         dates), backward pass from the latest early finish for late start/finish; total slack \
         = LS − ES, free slack = room before the nearest successor constraint; critical = no \
         total slack. Durations and lags in working days."
            .into(),
    );
    let mut params = BTreeMap::new();
    params.insert(
        "calendar".into(),
        Value::String(spec.calendar.name().into()),
    );
    if let Some(start) = &spec.start {
        params.insert("start".into(), Value::String(start.clone()));
        params.insert(
            "finish_date".into(),
            Value::String(spec.finish_text(spec.project_duration)),
        );
    }
    params.insert(
        "critical_path".into(),
        Value::String(spec.critical_path.join(" → ")),
    );
    computed.method.params = params;
    computed.inputs = vec![chem::input("tasks", "data", bytes, request.path.as_deref())];
    let v = &mut computed.values;
    v.insert("project_duration".into(), days(spec.project_duration));
    #[allow(clippy::cast_precision_loss)]
    {
        v.insert("tasks".into(), ComputedValue::new(spec.tasks.len() as f64));
        v.insert(
            "critical_tasks".into(),
            ComputedValue::new(spec.critical_path.len() as f64),
        );
        v.insert(
            "milestones".into(),
            ComputedValue::new(spec.milestones().count() as f64),
        );
    }
    let mut used = std::collections::HashSet::new();
    for (ix, t) in spec.tasks.iter().enumerate().take(ENVELOPE_TASKS) {
        let mut key = value_key(&t.id);
        if !used.insert(key.clone()) {
            key = format!("{key}_{}", ix + 1);
            used.insert(key.clone());
        }
        for (name, x) in [
            ("es", t.early_start),
            ("ef", t.early_finish),
            ("ls", t.late_start),
            ("lf", t.late_finish),
            ("total_slack", t.total_slack),
            ("free_slack", t.free_slack),
        ] {
            v.insert(format!("{key}.{name}"), days(x));
        }
    }
    computed
}
