//! Log files to records: JSON lines, logfmt and plain text.
//!
//! The format is told from the first lines (most of them JSON objects,
//! most of them `key=value` pairs, or else text) unless the caller names
//! it. Each record gets its message, a level normalised to `fatal`,
//! `error`, `warn`, `info`, `debug` or `trace` (pino/bunyan numbers
//! included), the timestamp as written and the other keys as fields; a
//! tracing-style nested `fields` object is flattened. Plain text lines
//! give a leading timestamp and the first level word; indented lines
//! (stack traces) are folded into the record above as `more.N` fields.
//! ANSI colour codes are stripped.
//!
//! Facets are the fields that make good chips (few distinct short
//! values, present on enough lines), the well-known names first; the
//! first service-like one is also a column. When most timestamps parse
//! (RFC 3339 / ISO 8601, `2026-10-24 13:05:07,123`, epoch seconds,
//! milliseconds, microseconds or nanoseconds), a histogram counts the
//! lines per bucket and level class.

use std::collections::{HashMap, HashSet};

use application::ports::formats::LogRequest;
use domain::devops::logs::{
    BucketCounts, LogFormat, LogHistogram, LogSpec, MAX_BUCKETS, MAX_LINES,
};
use domain::widget::list::{LevelClass, ListItem, ListSpec};
use serde_json::{Map, Value};

/// Lines looked at to tell the format.
const SNIFF_LINES: usize = 50;
/// Facets offered at most.
const MAX_FACETS: usize = 4;
/// Most distinct values of a facet field.
const MAX_FACET_VALUES: usize = 20;
/// Continuation lines kept per record.
const MAX_MORE: usize = 50;

/// Keys of the message, level and time, in order of preference.
const MESSAGE_KEYS: [&str; 7] = ["msg", "message", "@message", "log", "text", "event", "body"];
const LEVEL_KEYS: [&str; 8] = [
    "level",
    "lvl",
    "severity",
    "loglevel",
    "log.level",
    "@level",
    "levelname",
    "severity_text",
];
const TIME_KEYS: [&str; 8] = [
    "ts",
    "time",
    "timestamp",
    "@timestamp",
    "t",
    "date",
    "datetime",
    "asctime",
];
/// Fields preferred as facets, then as the column.
const KNOWN_FACETS: [&str; 12] = [
    "service",
    "service.name",
    "app",
    "component",
    "logger",
    "logger_name",
    "name",
    "target",
    "module",
    "host",
    "hostname",
    "env",
];

/// Reads a log file.
///
/// # Errors
///
/// A message naming the path for bytes that are not text or hold no
/// line.
pub fn parse(bytes: &[u8], request: &LogRequest) -> Result<LogSpec, String> {
    let path = &request.path;
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        // A head cut inside a character: drop the partial one.
        Err(e) if request.truncated && e.error_len().is_none() => {
            std::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or_default()
        }
        Err(_) => return Err(format!("`{path}` is not UTF-8 text")),
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines: Vec<String> = text
        .lines()
        .map(|l| strip_ansi(l.trim_end_matches('\r')))
        .collect();
    if request.truncated && !text.ends_with('\n') {
        // The last line may be cut.
        lines.pop();
    }
    if lines.iter().all(|l| l.trim().is_empty()) {
        return Err(format!("`{path}` has no log lines"));
    }
    let format = request.format.unwrap_or_else(|| sniff(&lines));
    let mut items: Vec<ListItem> = Vec::new();
    let mut total = 0usize;
    let mut times: Vec<Option<f64>> = Vec::new();
    // Whether the previous record is kept (so its continuation lines are).
    let mut kept = false;
    // Whether the previous record had a timestamp.
    let mut last_timed = false;
    for line in &lines {
        if line.trim().is_empty() {
            continue;
        }
        let record = match format {
            LogFormat::Json => json_line(line),
            LogFormat::Logfmt => logfmt_line(line),
            LogFormat::Text => None,
        };
        let record = match record {
            Some(record) => record,
            None => {
                let item = text_line(line);
                // In a log whose lines start with a time, a line without
                // one (an exception message) continues the record above.
                let untimed = format == LogFormat::Text && item.ts.is_none() && last_timed;
                if total > 0 && (is_continuation(line, format) || untimed) {
                    if kept && let Some(last) = items.last_mut() {
                        add_more(last, line);
                    }
                    continue;
                }
                item
            }
        };
        last_timed = record.ts.is_some();
        total += 1;
        kept = items.len() < MAX_LINES;
        if kept {
            times.push(record.ts.as_deref().and_then(parse_time));
            items.push(record);
        }
    }
    let (facets, columns) = facets(&items);
    let histogram = histogram(&items, &times);
    Ok(LogSpec {
        path: Some(path.clone()),
        format,
        total_lines: total,
        truncated: request.truncated,
        list: ListSpec {
            title: None,
            columns,
            items,
            facets,
            follow: false,
        },
        histogram,
    })
}

/// Removes ANSI escape sequences (colours, cursor moves).
fn strip_ansi(line: &str) -> String {
    if !line.contains('\u{1b}') {
        return line.to_owned();
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // Parameters, then one final byte in `@`..=`~`.
                for d in chars.by_ref() {
                    if ('@'..='~').contains(&d) {
                        break;
                    }
                }
            }
            Some(']') => {
                // OSC: up to BEL or ST.
                chars.next();
                while let Some(d) = chars.next() {
                    if d == '\u{7}' {
                        break;
                    }
                    if d == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {
                chars.next();
            }
        }
    }
    out
}

/// The format most of the first lines are in.
fn sniff(lines: &[String]) -> LogFormat {
    // Indented lines (stack traces) do not count.
    let sample: Vec<&str> = lines
        .iter()
        .filter(|l| !l.starts_with([' ', '\t']))
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .take(SNIFF_LINES)
        .collect();
    let n = sample.len().max(1);
    let json = sample.iter().filter(|l| json_line(l).is_some()).count();
    if json * 2 > n {
        return LogFormat::Json;
    }
    let logfmt = sample
        .iter()
        .filter(|l| pairs(l).iter().filter(|(_, v)| v.is_some()).count() >= 2)
        .count();
    if logfmt * 2 > n {
        LogFormat::Logfmt
    } else {
        LogFormat::Text
    }
}

/// A line that belongs to the record above: indented, or a stack frame
/// or cause marker.
fn is_continuation(line: &str, format: LogFormat) -> bool {
    let starts_blank = line.starts_with([' ', '\t']);
    let trimmed = line.trim_start();
    starts_blank
        || trimmed.starts_with("at ")
        || trimmed.starts_with("Caused by")
        || trimmed.starts_with("... ")
        || (format != LogFormat::Text && trimmed.starts_with('}'))
}

fn add_more(item: &mut ListItem, line: &str) {
    let n = item
        .fields
        .keys()
        .filter(|k| k.starts_with("more."))
        .count();
    if n < MAX_MORE {
        item.fields.insert(
            format!("more.{}", n + 1),
            Value::String(line.trim_end().to_owned()),
        );
    } else if n == MAX_MORE {
        item.fields.insert(
            "more.…".to_owned(),
            Value::String("more lines cut".to_owned()),
        );
    }
}

/// The canonical level name of `raw` (a name or a pino/bunyan number).
fn level_name(raw: &Value) -> Option<String> {
    let name = match raw {
        Value::Number(n) => match n.as_u64()? {
            0..=10 => "trace",
            11..=20 => "debug",
            21..=30 => "info",
            31..=40 => "warn",
            41..=50 => "error",
            _ => "fatal",
        }
        .to_owned(),
        Value::String(s) => canonical_level(s)?,
        _ => return None,
    };
    Some(name)
}

fn canonical_level(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(
        match LevelClass::of(text) {
            LevelClass::Fatal => "fatal",
            LevelClass::Error => "error",
            LevelClass::Warn => "warn",
            LevelClass::Info => "info",
            LevelClass::Debug => "debug",
            LevelClass::Trace => "trace",
            LevelClass::Other => return Some(text.to_ascii_lowercase()),
        }
        .to_owned(),
    )
}

fn take_first(map: &mut Map<String, Value>, keys: &[&str]) -> Option<Value> {
    keys.iter().find_map(|key| map.shift_remove(*key))
}

/// A JSON object line as a record.
fn json_line(line: &str) -> Option<ListItem> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(trimmed) else {
        return None;
    };
    // tracing-subscriber's JSON: the message sits in `fields`.
    if let Some(Value::Object(inner)) = map.get("fields").cloned()
        && !MESSAGE_KEYS.iter().any(|k| map.contains_key(*k))
    {
        map.shift_remove("fields");
        for (key, value) in inner {
            map.entry(key).or_insert(value);
        }
    }
    let message = take_first(&mut map, &MESSAGE_KEYS);
    let level = take_first(&mut map, &LEVEL_KEYS).and_then(|l| level_name(&l));
    let ts = take_first(&mut map, &TIME_KEYS).map(|t| match t {
        Value::String(s) => s,
        other => other.to_string(),
    });
    let text = match message {
        Some(Value::String(s)) => s,
        Some(other) => other.to_string(),
        None => summary(&map),
    };
    Some(ListItem {
        level,
        ts,
        text,
        fields: map,
    })
}

/// The first fields as `k=v`, for a record without a message.
fn summary(fields: &Map<String, Value>) -> String {
    fields
        .iter()
        .take(4)
        .map(|(k, v)| match v {
            Value::String(s) => format!("{k}={s}"),
            other => format!("{k}={other}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `key=value` pairs of a logfmt line; a bare key has no value.
fn pairs(line: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            key.push(c);
            chars.next();
        }
        if chars.peek() != Some(&'=') {
            out.push((key, None));
            continue;
        }
        chars.next();
        let mut value = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(next) = chars.next() {
                            value.push(match next {
                                'n' => '\n',
                                't' => '\t',
                                other => other,
                            });
                        }
                    }
                    '"' => break,
                    other => value.push(other),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                value.push(c);
                chars.next();
            }
        }
        out.push((key, Some(value)));
    }
    out
}

/// A logfmt line as a record, when it has at least two pairs.
fn logfmt_line(line: &str) -> Option<ListItem> {
    let pairs = pairs(line);
    if pairs.iter().filter(|(_, v)| v.is_some()).count() < 2 {
        return None;
    }
    let mut map = Map::new();
    let mut bare = Vec::new();
    for (key, value) in pairs {
        match value {
            Some(value) => {
                map.insert(key, Value::String(value));
            }
            None => bare.push(key),
        }
    }
    let message = take_first(&mut map, &MESSAGE_KEYS);
    let level = take_first(&mut map, &LEVEL_KEYS).and_then(|l| level_name(&l));
    let ts = take_first(&mut map, &TIME_KEYS).map(|t| match t {
        Value::String(s) => s,
        other => other.to_string(),
    });
    let text = match message {
        Some(Value::String(s)) => s,
        _ if !bare.is_empty() => bare.join(" "),
        _ => summary(&map),
    };
    Some(ListItem {
        level,
        ts,
        text,
        fields: map,
    })
}

/// A plain text line: a leading timestamp (maybe in brackets), then the
/// first level word among the next few.
fn text_line(line: &str) -> ListItem {
    let mut rest = line.trim();
    let mut ts = None;
    let unbracketed = rest.strip_prefix('[').unwrap_or(rest);
    if let Some((_, len)) = parse_time_prefix(unbracketed) {
        ts = Some(unbracketed[..len].to_owned());
        rest = unbracketed[len..].trim_start();
        rest = rest.strip_prefix(']').unwrap_or(rest).trim_start();
    }
    let mut level = None;
    let words: Vec<&str> = rest.split_whitespace().take(4).collect();
    for (ix, word) in words.iter().enumerate() {
        let bare =
            word.trim_matches(|c: char| matches!(c, '[' | ']' | '(' | ')' | ':' | '<' | '>'));
        let upper = !bare.is_empty() && bare.chars().all(|c| c.is_ascii_uppercase());
        if (upper || ix == 0) && is_level_word(bare) {
            level = canonical_level(bare);
            if ix == 0 {
                rest = rest[word.len()..].trim_start();
            }
            break;
        }
    }
    ListItem {
        level,
        ts,
        text: rest.to_owned(),
        fields: Map::new(),
    }
}

fn is_level_word(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "fatal"
            | "critical"
            | "crit"
            | "panic"
            | "emerg"
            | "alert"
            | "error"
            | "err"
            | "warn"
            | "warning"
            | "info"
            | "notice"
            | "debug"
            | "trace"
            | "verbose"
    )
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard
/// Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `n` ASCII digits at the start of `s`, as a number.
fn digits(s: &[u8], n: usize) -> Option<u32> {
    let part = s.get(..n)?;
    if !part.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(part).ok()?.parse().ok()
}

/// A calendar timestamp at the start of `s`: seconds since the epoch and
/// the bytes it took. `YYYY-MM-DD[T ]HH:MM[:SS][.,fraction][Z|±HH[:]MM]`;
/// without a zone it is UTC.
fn parse_time_prefix(s: &str) -> Option<(f64, usize)> {
    let b = s.as_bytes();
    let year = digits(b, 4)?;
    if b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return None;
    }
    let month = digits(&b[5..], 2)?;
    let day = digits(&b[8..], 2)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut at = 10;
    let mut secs = 0.0;
    if matches!(b.get(at), Some(b'T' | b't' | b' ')) && b.get(at + 3) == Some(&b':') {
        let hour = digits(&b[at + 1..], 2)?;
        let minute = digits(&b[at + 4..], 2)?;
        at += 6;
        let mut second = 0;
        if b.get(at) == Some(&b':') {
            second = digits(&b[at + 1..], 2)?;
            at += 3;
        }
        let mut fraction = 0.0;
        if matches!(b.get(at), Some(b'.' | b',')) {
            let start = at + 1;
            let mut end = start;
            while b.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            if end > start {
                fraction = format!("0.{}", &s[start..end]).parse().unwrap_or(0.0);
                at = end;
            }
        }
        secs = f64::from(hour * 3_600 + minute * 60 + second) + fraction;
        match b.get(at) {
            Some(b'Z' | b'z') => at += 1,
            Some(sign @ (b'+' | b'-')) => {
                let hours = digits(&b[at + 1..], 2);
                let (minutes, len) = if b.get(at + 3) == Some(&b':') {
                    (digits(&b[at + 4..], 2), 6)
                } else {
                    (digits(&b[at + 3..], 2), 5)
                };
                if let (Some(h), Some(m)) = (hours, minutes) {
                    let offset = f64::from(h * 3_600 + m * 60);
                    secs -= if *sign == b'+' { offset } else { -offset };
                    at += len;
                } else if let Some(h) = hours {
                    secs -= f64::from(h * 3_600) * if *sign == b'+' { 1.0 } else { -1.0 };
                    at += 3;
                }
            }
            _ => {}
        }
    }
    let days = days_from_civil(i64::from(year), month, day);
    Some((days as f64 * 86_400.0 + secs, at))
}

/// A timestamp as seconds since the epoch: a calendar time, or an epoch
/// number in seconds, milliseconds, microseconds or nanoseconds (told by
/// its size).
#[must_use]
pub fn parse_time(text: &str) -> Option<f64> {
    let text = text.trim();
    if let Ok(n) = text.parse::<f64>() {
        if !n.is_finite() || n <= 0.0 {
            return None;
        }
        let secs = if n > 1e17 {
            n / 1e9
        } else if n > 1e14 {
            n / 1e6
        } else if n > 1e11 {
            n / 1e3
        } else {
            n
        };
        return Some(secs);
    }
    parse_time_prefix(text).map(|(secs, _)| secs)
}

/// Facet fields and the column, from the items' fields.
fn facets(items: &[ListItem]) -> (Vec<String>, Vec<String>) {
    struct Stat {
        present: usize,
        values: HashSet<String>,
        too_many: bool,
        first_seen: usize,
    }
    let mut stats: HashMap<&str, Stat> = HashMap::new();
    for item in items {
        for (key, value) in &item.fields {
            if key.starts_with("more.") {
                continue;
            }
            let order = stats.len();
            let stat = stats.entry(key.as_str()).or_insert_with(|| Stat {
                present: 0,
                values: HashSet::new(),
                too_many: false,
                first_seen: order,
            });
            stat.present += 1;
            if stat.too_many {
                continue;
            }
            let text = match value {
                Value::String(s) => s.clone(),
                Value::Bool(_) => value.to_string(),
                // Numbers (durations, counts) make poor chips.
                _ => {
                    stat.too_many = true;
                    continue;
                }
            };
            if text.chars().count() > 40 {
                stat.too_many = true;
            } else {
                stat.values.insert(text);
                stat.too_many = stat.values.len() > MAX_FACET_VALUES;
            }
        }
    }
    let n = items.len().max(1);
    let good = |stat: &Stat, known: bool| {
        !stat.too_many
            && stat.present * 10 >= n
            && (stat.values.len() >= 2 || (known && !stat.values.is_empty()))
            && (stat.values.len() < n || n < 4)
    };
    let mut chosen: Vec<String> = KNOWN_FACETS
        .iter()
        .filter(|k| stats.get(**k).is_some_and(|s| good(s, true)))
        .map(|k| (*k).to_owned())
        .collect();
    let mut others: Vec<(&str, &Stat)> = stats
        .iter()
        .filter(|(k, s)| !KNOWN_FACETS.contains(k) && good(s, false))
        .map(|(k, s)| (*k, s))
        .collect();
    others.sort_by(|a, b| {
        b.1.present
            .cmp(&a.1.present)
            .then(a.1.first_seen.cmp(&b.1.first_seen))
    });
    chosen.extend(others.into_iter().map(|(k, _)| k.to_owned()));
    chosen.truncate(MAX_FACETS);
    let column = chosen
        .iter()
        .find(|f| KNOWN_FACETS.contains(&f.as_str()))
        .or_else(|| chosen.first())
        .cloned();
    (chosen, column.into_iter().collect())
}

/// Bucket widths tried, in seconds.
const WIDTHS: [f64; 22] = [
    0.001,
    0.01,
    0.1,
    1.0,
    2.0,
    5.0,
    10.0,
    15.0,
    30.0,
    60.0,
    120.0,
    300.0,
    600.0,
    900.0,
    1_800.0,
    3_600.0,
    7_200.0,
    21_600.0,
    43_200.0,
    86_400.0,
    604_800.0,
    2_592_000.0,
];
/// Buckets the histogram aims for at most.
const TARGET_BUCKETS: f64 = 60.0;

/// Lines per bucket and class, when at least half the items have a time
/// and they span more than an instant.
fn histogram(items: &[ListItem], times: &[Option<f64>]) -> Option<LogHistogram> {
    let known: Vec<f64> = times.iter().flatten().copied().collect();
    if known.len() < 2 || known.len() * 2 < items.len() {
        return None;
    }
    let lo = known.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = known.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = hi - lo;
    if span <= 0.0 {
        return None;
    }
    let width = WIDTHS
        .iter()
        .copied()
        .find(|w| span / w <= TARGET_BUCKETS)
        .unwrap_or_else(|| span / TARGET_BUCKETS);
    let start = (lo / width).floor() * width;
    let count = (((hi - start) / width).floor() as usize + 1).min(MAX_BUCKETS);
    let mut buckets = vec![BucketCounts::default(); count];
    for (item, at) in items.iter().zip(times) {
        let Some(at) = at else { continue };
        let ix = (((at - start) / width).floor().max(0.0) as usize).min(count - 1);
        let bucket = &mut buckets[ix];
        match item.level.as_deref().map(LevelClass::of) {
            Some(LevelClass::Fatal | LevelClass::Error) => bucket.errors += 1,
            Some(LevelClass::Warn) => bucket.warnings += 1,
            _ => bucket.other += 1,
        }
    }
    Some(LogHistogram {
        start,
        width,
        buckets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(path: &str) -> LogRequest {
        LogRequest {
            path: path.into(),
            format: None,
            truncated: false,
        }
    }

    #[test]
    fn times_parse_in_every_shape() {
        assert_eq!(parse_time("1970-01-02T00:00:00Z"), Some(86_400.0));
        assert_eq!(parse_time("1970-01-01T01:00:00+01:00"), Some(0.0));
        assert_eq!(parse_time("1970-01-01 00:00:01,500"), Some(1.5));
        assert_eq!(parse_time("2026-10-24T13:05:07Z"), Some(1_792_847_107.0));
        assert_eq!(parse_time("1792847107123"), Some(1_792_847_107.123));
        assert_eq!(parse_time("1792847107"), Some(1_792_847_107.0));
        assert_eq!(parse_time("2026-13-01"), None);
        assert_eq!(parse_time("noon"), None);
        assert_eq!(
            parse_time_prefix("2026-10-24T13:05:07.5Z INFO x").map(|p| p.1),
            Some(22)
        );
    }

    #[test]
    fn ansi_is_stripped() {
        assert_eq!(strip_ansi("\u{1b}[31mERROR\u{1b}[0m boom"), "ERROR boom");
        assert_eq!(
            strip_ansi("\u{1b}]8;;http://x\u{7}link\u{1b}]8;;\u{7}"),
            "link"
        );
    }

    #[test]
    fn logfmt_pairs_handle_quotes_and_bare_keys() {
        assert_eq!(
            pairs(r#"a=1 msg="hi \"there\"" flag b="#),
            [
                ("a".into(), Some("1".into())),
                ("msg".into(), Some("hi \"there\"".into())),
                ("flag".into(), None),
                ("b".into(), Some(String::new())),
            ]
        );
    }

    #[test]
    fn text_lines_give_time_and_level() {
        let item = text_line("[2026-10-24T13:05:07Z ERROR crest::run] boom");
        assert_eq!(item.ts.as_deref(), Some("2026-10-24T13:05:07Z"));
        assert_eq!(item.level.as_deref(), Some("error"));
        assert_eq!(item.text, "crest::run] boom");
        let python = text_line("2026-10-24 13:05:07,123 WARNING [main] disk low");
        assert_eq!(python.level.as_deref(), Some("warn"));
        assert_eq!(python.text, "[main] disk low");
        let prose = text_line("the info desk is open");
        assert_eq!(prose.level, None);
    }

    #[test]
    fn json_lines_with_pino_levels_and_tracing_fields() {
        let bytes = concat!(
            r#"{"level":30,"time":1792847107000,"msg":"listening","service":"api"}"#,
            "\n",
            r#"{"timestamp":"2026-10-24T13:05:08Z","level":"ERROR","fields":{"message":"db down","port":5432},"target":"crest::db"}"#,
            "\n",
            "    at Pool.connect (pool.js:12)\n",
            "not json at all\n",
        );
        let spec = parse(bytes.as_bytes(), &request("app.log")).unwrap();
        assert_eq!(spec.format, LogFormat::Json);
        assert_eq!(spec.total_lines, 3);
        let [a, b, c] = &spec.list.items[..] else {
            panic!("{:?}", spec.list.items);
        };
        assert_eq!(
            (a.level.as_deref(), a.text.as_str()),
            (Some("info"), "listening")
        );
        assert_eq!(
            (b.level.as_deref(), b.text.as_str()),
            (Some("error"), "db down")
        );
        assert_eq!(b.fields["port"], 5432);
        assert_eq!(b.fields["more.1"], "    at Pool.connect (pool.js:12)");
        assert_eq!(c.text, "not json at all");
        let histogram = spec.histogram.unwrap();
        assert_eq!(histogram.width, 0.1);
        assert_eq!(histogram.buckets.first().map(|b| b.other), Some(1));
        assert_eq!(histogram.buckets.last().map(|b| b.errors), Some(1));
    }

    #[test]
    fn logfmt_gets_facets_and_a_column() {
        let mut text = String::new();
        for ix in 0..20 {
            let service = if ix % 2 == 0 { "api" } else { "db" };
            let level = if ix % 5 == 0 { "warn" } else { "info" };
            text.push_str(&format!(
                "ts=2026-10-24T13:05:{:02}Z level={level} service={service} req={ix} msg=\"request {ix}\"\n",
                ix
            ));
        }
        let spec = parse(text.as_bytes(), &request("x.log")).unwrap();
        assert_eq!(spec.format, LogFormat::Logfmt);
        assert_eq!(spec.list.facets, ["service"]);
        assert_eq!(spec.list.columns, ["service"]);
        assert_eq!(spec.list.items[3].text, "request 3");
        spec.validate().unwrap();
        assert_eq!(
            spec.histogram
                .map(|h| h.buckets.iter().map(BucketCounts::total).sum::<u32>()),
            Some(20)
        );
    }

    #[test]
    fn a_cut_head_drops_the_partial_line() {
        let spec = parse(
            b"INFO one\nINFO two\nINFO thr",
            &LogRequest {
                truncated: true,
                ..request("x.log")
            },
        )
        .unwrap();
        assert_eq!(spec.total_lines, 2);
        assert!(spec.truncated);
        assert!(parse(b"\n\n", &request("e.log")).is_err());
        assert!(parse(&[0xff, 0xfe, 0x00], &request("b.log")).is_err());
    }
}
