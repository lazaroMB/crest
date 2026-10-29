//! PostgreSQL `EXPLAIN (FORMAT JSON)` into a [`QueryPlanSpec`].
//!
//! Accepted shapes: the array `EXPLAIN` prints (`[{"Plan": ...,
//! "Execution Time": ...}]`), one such object, the result row some
//! clients export (`[{"QUERY PLAN": [...]}]`), and the plan copied out of
//! `psql` with its header, `+` continuation marks and row count. Text
//! `EXPLAIN` output is refused with a hint to re-run it with
//! `FORMAT JSON`.
//!
//! The JSON is walked as a [`Value`]: PostgreSQL writes one key per
//! property with spaces in the names (`"Actual Total Time"`), and which
//! keys appear depends on the node and the options. The known ones fill
//! [`PlanNode`]'s fields; other scalars and lists of strings go to its
//! `attrs`.

use domain::devops::query_plan::{MAX_DEPTH, MAX_NODES, PlanBuffers, PlanNode, QueryPlanSpec};
use serde_json::{Map, Value};

/// Keys read into [`PlanNode`] fields (not repeated in `attrs`).
const KNOWN: &[&str] = &[
    "Node Type",
    "Plans",
    "Relation Name",
    "Schema",
    "Alias",
    "Index Name",
    "Join Type",
    "Parent Relationship",
    "Subplan Name",
    "Startup Cost",
    "Total Cost",
    "Plan Rows",
    "Actual Total Time",
    "Actual Rows",
    "Actual Loops",
    "Rows Removed by Filter",
    "Rows Removed by Join Filter",
    "Rows Removed by Index Recheck",
    "Workers Launched",
    "Shared Hit Blocks",
    "Shared Read Blocks",
    "Shared Dirtied Blocks",
    "Shared Written Blocks",
    "Local Hit Blocks",
    "Local Read Blocks",
    "Local Dirtied Blocks",
    "Local Written Blocks",
    "Temp Read Blocks",
    "Temp Written Blocks",
    "Sort Space Type",
    "Sort Space Used",
    "Hash Batches",
    "Workers",
];

/// The condition shown for a node: the first of these keys present.
const CONDITIONS: &[&str] = &[
    "Index Cond",
    "Hash Cond",
    "Merge Cond",
    "Join Filter",
    "Recheck Cond",
    "Filter",
    "Sort Key",
    "Group Key",
    "One-Time Filter",
];

/// Parses the plan in `bytes`.
///
/// # Errors
///
/// A message for text `EXPLAIN` output, malformed JSON, JSON without a
/// `Plan`, or a plan nested too deep or too large.
pub fn parse(bytes: &[u8], path: Option<&str>) -> Result<QueryPlanSpec, String> {
    let name = path.unwrap_or("the plan");
    let text = std::str::from_utf8(bytes).map_err(|_| format!("`{name}` is not UTF-8 text"))?;
    let json = json_text(text).ok_or_else(|| {
        if looks_like_text_plan(text) {
            format!(
                "`{name}` is a text EXPLAIN; only JSON plans are read. Run it again as \
                 EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) ..."
            )
        } else {
            format!("`{name}` is not an EXPLAIN (FORMAT JSON) plan")
        }
    })?;
    let value: Value =
        serde_json::from_str(&json).map_err(|e| format!("`{name}` is not valid JSON: {e}"))?;
    let top = top_object(value).ok_or_else(|| format!("`{name}` has no \"Plan\" object"))?;
    let plan = top
        .get("Plan")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("`{name}` has no \"Plan\" object"))?;
    let mut nodes = Vec::new();
    walk(plan, 0, &mut nodes)?;
    let analyzed = nodes
        .iter()
        .any(|n| n.actual_time.is_some() || n.loops.is_some());
    let buffers = nodes.iter().any(|n| n.buffers.is_some());
    let trigger_time = top.get("Triggers").and_then(Value::as_array).map(|t| {
        t.iter()
            .filter_map(|t| t.get("Time").and_then(Value::as_f64))
            .sum::<f64>()
    });
    let jit_time = top
        .get("JIT")
        .and_then(|j| j.get("Timing"))
        .and_then(|t| t.get("Total"))
        .and_then(number);
    let mut spec = QueryPlanSpec {
        title: None,
        path: path.map(str::to_owned),
        engine: domain::devops::query_plan::PlanEngine::Postgres,
        analyzed,
        buffers,
        planning_time: top.get("Planning Time").and_then(number),
        execution_time: top
            .get("Execution Time")
            .or_else(|| top.get("Total Runtime"))
            .and_then(number),
        trigger_time: trigger_time.filter(|t| *t > 0.0),
        jit_time,
        nodes,
    };
    spec.analyse();
    Ok(spec)
}

/// The JSON part of `text`: as is when it starts with `[` or `{`, else
/// the plan inside `psql` output (header, `+` marks, `(1 row)`).
fn json_text(text: &str) -> Option<String> {
    let trimmed = text.trim_start_matches('\u{feff}').trim();
    if trimmed.starts_with(['[', '{']) {
        return Some(trimmed.to_owned());
    }
    let start = trimmed.find(['[', '{'])?;
    let mut out = String::new();
    for line in trimmed[start..].lines() {
        let line = line.trim_end();
        if line.starts_with('(') && line.ends_with("row)") || line.ends_with("rows)") {
            break;
        }
        let line = line.strip_suffix('+').unwrap_or(line).trim_end();
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}

fn looks_like_text_plan(text: &str) -> bool {
    text.contains("cost=") || text.contains("actual time=") || text.contains("->")
}

/// The object holding `Plan`, unwrapping arrays and `QUERY PLAN`
/// result rows (whose value may be the plan as a JSON string).
fn top_object(value: Value) -> Option<Map<String, Value>> {
    match value {
        Value::Array(items) => items.into_iter().find_map(top_object),
        Value::Object(map) if map.contains_key("Plan") => Some(map),
        Value::Object(mut map) => {
            let inner = map
                .remove("QUERY PLAN")
                .or_else(|| map.remove("query plan"))?;
            match inner {
                Value::String(s) => top_object(serde_json::from_str(&s).ok()?),
                other => top_object(other),
            }
        }
        _ => None,
    }
}

/// `c` without one pair of parentheses around all of it.
fn unwrap_parens(c: &str) -> &str {
    let Some(inner) = c.strip_prefix('(').and_then(|c| c.strip_suffix(')')) else {
        return c;
    };
    let mut depth = 0_i32;
    for ch in inner.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return c;
        }
    }
    inner
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite())
}

fn text(map: &Map<String, Value>, key: &str) -> Option<String> {
    match map.get(key)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Array(items) => {
            let parts: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
            (!parts.is_empty()).then(|| parts.join(", "))
        }
        _ => None,
    }
}

fn count(map: &Map<String, Value>, key: &str) -> Option<u64> {
    map.get(key).and_then(Value::as_u64)
}

fn walk(map: &Map<String, Value>, depth: u32, out: &mut Vec<PlanNode>) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("the plan is nested deeper than {MAX_DEPTH} levels"));
    }
    if out.len() >= MAX_NODES {
        return Err(format!("the plan has more than {MAX_NODES} nodes"));
    }
    let node_type = text(map, "Node Type").unwrap_or_else(|| "?".to_owned());
    let relation = text(map, "Relation Name")
        .or_else(|| text(map, "Function Name"))
        .or_else(|| text(map, "CTE Name"));
    let relation = match (relation, text(map, "Schema")) {
        (Some(r), Some(schema)) if schema != "public" => Some(format!("{schema}.{r}")),
        (r, _) => r,
    };
    let alias = text(map, "Alias").filter(|a| {
        relation
            .as_deref()
            .is_none_or(|r| r.rsplit('.').next() != Some(a.as_str()))
    });
    let removed: Vec<f64> = [
        "Rows Removed by Filter",
        "Rows Removed by Join Filter",
        "Rows Removed by Index Recheck",
    ]
    .iter()
    .filter_map(|k| map.get(*k).and_then(number))
    .collect();
    let block_keys = [
        "Shared Hit Blocks",
        "Shared Read Blocks",
        "Shared Dirtied Blocks",
        "Shared Written Blocks",
        "Local Hit Blocks",
        "Local Read Blocks",
        "Temp Read Blocks",
        "Temp Written Blocks",
    ];
    let buffers = block_keys
        .iter()
        .any(|k| map.contains_key(*k))
        .then(|| PlanBuffers {
            shared_hit: count(map, "Shared Hit Blocks").unwrap_or(0),
            shared_read: count(map, "Shared Read Blocks").unwrap_or(0),
            shared_dirtied: count(map, "Shared Dirtied Blocks").unwrap_or(0),
            shared_written: count(map, "Shared Written Blocks").unwrap_or(0),
            local: count(map, "Local Hit Blocks").unwrap_or(0)
                + count(map, "Local Read Blocks").unwrap_or(0),
            temp_read: count(map, "Temp Read Blocks").unwrap_or(0),
            temp_written: count(map, "Temp Written Blocks").unwrap_or(0),
        });
    let condition = CONDITIONS.iter().find_map(|k| {
        text(map, k).map(|c| format!("{}: {}", k.to_ascii_lowercase(), unwrap_parens(&c)))
    });
    let mut attrs = Map::new();
    for (key, value) in map {
        if KNOWN.contains(&key.as_str()) {
            continue;
        }
        match value {
            Value::String(_) | Value::Number(_) | Value::Bool(_) => {
                attrs.insert(key.clone(), value.clone());
            }
            Value::Array(items) if items.iter().all(Value::is_string) && !items.is_empty() => {
                if let Some(joined) = text(map, key) {
                    attrs.insert(key.clone(), joined.into());
                }
            }
            _ => {}
        }
    }
    out.push(PlanNode {
        node_type,
        depth,
        relation,
        alias,
        index: text(map, "Index Name"),
        join_type: text(map, "Join Type"),
        parent_relationship: text(map, "Parent Relationship"),
        subplan_name: text(map, "Subplan Name"),
        condition,
        startup_cost: map.get("Startup Cost").and_then(number),
        total_cost: map.get("Total Cost").and_then(number),
        plan_rows: map.get("Plan Rows").and_then(number),
        actual_time: map.get("Actual Total Time").and_then(number),
        actual_rows: map.get("Actual Rows").and_then(number),
        loops: map.get("Actual Loops").and_then(number),
        rows_removed: (!removed.is_empty()).then(|| removed.iter().sum()),
        workers: count(map, "Workers Launched").and_then(|w| u32::try_from(w).ok()),
        buffers,
        sort_space_type: text(map, "Sort Space Type"),
        sort_space_kb: map.get("Sort Space Used").and_then(number),
        hash_batches: count(map, "Hash Batches"),
        inclusive_time: None,
        self_time: None,
        warnings: Vec::new(),
        attrs,
    });
    if let Some(children) = map.get("Plans").and_then(Value::as_array) {
        for child in children.iter().filter_map(Value::as_object) {
            walk(child, depth + 1, out)?;
        }
    }
    Ok(())
}
