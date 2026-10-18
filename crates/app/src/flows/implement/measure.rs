//! Expected against obtained: parses the `expect` of a hypothesis test
//! (`p95 < 20ms`, `>= 1000 req/s`) and finds the matching measurement in
//! the output of its command, best effort.
//!
//! With a metric name (`p95`), the measurement is the first number after
//! its last mention in the output; without one, the last number with the
//! expected unit (or the last number at all). Times convert between `ns`,
//! `us`/`µs`, `ms`, `s` and `min`.

/// A comparison of an expectation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    /// `<`
    Less,
    /// `<=` or `≤`
    LessOrEqual,
    /// `>`
    Greater,
    /// `>=` or `≥`
    GreaterOrEqual,
    /// `=` or `==`
    Equal,
}

impl Comparison {
    /// Whether `value <op> threshold` holds.
    #[must_use]
    pub fn holds(self, value: f64, threshold: f64) -> bool {
        match self {
            Self::Less => value < threshold,
            Self::LessOrEqual => value <= threshold,
            Self::Greater => value > threshold,
            Self::GreaterOrEqual => value >= threshold,
            Self::Equal => (value - threshold).abs() <= f64::EPSILON * threshold.abs().max(1.0),
        }
    }

    /// The operator as written.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Less => "<",
            Self::LessOrEqual => "<=",
            Self::Greater => ">",
            Self::GreaterOrEqual => ">=",
            Self::Equal => "=",
        }
    }
}

/// A numeric expectation, e.g. `p95 < 20ms`.
#[derive(Debug, Clone, PartialEq)]
pub struct Expectation {
    /// What is measured (`p95`), if named.
    pub metric: Option<String>,
    /// How the measurement compares.
    pub comparison: Comparison,
    /// The threshold.
    pub threshold: f64,
    /// Its unit (`ms`), if any.
    pub unit: Option<String>,
}

const OPERATORS: [(&str, Comparison); 7] = [
    ("<=", Comparison::LessOrEqual),
    (">=", Comparison::GreaterOrEqual),
    ("==", Comparison::Equal),
    ("≤", Comparison::LessOrEqual),
    ("≥", Comparison::GreaterOrEqual),
    ("<", Comparison::Less),
    (">", Comparison::Greater),
];

/// A number at the start of `text` and the unit right after it.
fn leading_number(text: &str) -> Option<(f64, Option<String>, usize)> {
    let negative = text.starts_with('-');
    let body = if negative { &text[1..] } else { text };
    let len = body
        .bytes()
        .take_while(|b| b.is_ascii_digit() || *b == b'.')
        .count();
    let digits = &body[..len];
    let value: f64 = digits.trim_end_matches('.').parse().ok()?;
    let value = if negative { -value } else { value };
    let digits = &text[..len + usize::from(negative)];
    let rest = &text[digits.len()..];
    let rest_trimmed = rest.trim_start_matches(' ');
    let unit: String = rest_trimmed
        .chars()
        .take_while(|c| c.is_alphabetic() || *c == '/' || *c == '%' || *c == 'µ')
        .collect();
    let used = digits.len() + (rest.len() - rest_trimmed.len()) + unit.len();
    let unit = Some(unit).filter(|u| !u.is_empty());
    Some((value, unit, used))
}

/// Parses a numeric expectation; `None` for prose (`pasa`, `sin errores`).
#[must_use]
pub fn parse_expectation(text: &str) -> Option<Expectation> {
    let (at, symbol, comparison) = OPERATORS
        .iter()
        .filter_map(|(sym, cmp)| text.find(sym).map(|at| (at, *sym, *cmp)))
        .min_by_key(|(at, sym, _)| (*at, std::cmp::Reverse(sym.len())))
        .or_else(|| {
            // A lone `=` (not part of `<=`, `>=` or `==`).
            text.find('=').map(|at| (at, "=", Comparison::Equal))
        })?;
    let metric = text[..at].trim();
    let (threshold, unit, _) = leading_number(text[at + symbol.len()..].trim_start())?;
    Some(Expectation {
        metric: Some(metric.to_owned()).filter(|m| !m.is_empty()),
        comparison,
        threshold,
        unit,
    })
}

/// Factor to seconds of a time unit.
fn seconds_per(unit: &str) -> Option<f64> {
    Some(match unit {
        "ns" => 1e-9,
        "us" | "µs" | "μs" => 1e-6,
        "ms" => 1e-3,
        "s" | "sec" | "secs" => 1.0,
        "min" => 60.0,
        _ => return None,
    })
}

/// `value` in `from` expressed in `to`, when both are the same unit or
/// both are times.
#[must_use]
pub fn convert(value: f64, from: Option<&str>, to: Option<&str>) -> Option<f64> {
    match (from, to) {
        (_, None) => Some(value),
        (None, Some(_)) => Some(value),
        (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => Some(value),
        (Some(a), Some(b)) => Some(value * seconds_per(a)? / seconds_per(b)?),
    }
}

/// Every number in `text` with its unit, in order.
fn numbers(text: &str) -> Vec<(usize, f64, Option<String>)> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let starts = rest.starts_with(|c: char| c.is_ascii_digit());
        let after_word = text[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if starts
            && !after_word
            && let Some((value, unit, used)) = leading_number(rest)
        {
            found.push((i, value, unit));
            i += used.max(1);
            continue;
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    found
}

/// The measurement for `expect` in `output`, in the unit of the
/// expectation.
#[must_use]
pub fn find_measurement(expect: &Expectation, output: &str) -> Option<f64> {
    let wanted = expect.unit.as_deref();
    let all = numbers(output);
    if let Some(metric) = &expect.metric {
        let lower = output.to_lowercase();
        let at = lower.rfind(&metric.to_lowercase())? + metric.len();
        return all
            .iter()
            .find(|(pos, _, _)| *pos >= at)
            .and_then(|(_, v, u)| convert(*v, u.as_deref(), wanted));
    }
    let same_unit = all.iter().rev().find_map(|(_, v, u)| match (u, wanted) {
        (Some(u), Some(_)) => convert(*v, Some(u), wanted).filter(|_| seconds_or_same(u, wanted)),
        _ => None,
    });
    same_unit.or_else(|| {
        all.last()
            .and_then(|(_, v, u)| convert(*v, u.as_deref(), wanted))
    })
}

fn seconds_or_same(unit: &str, wanted: Option<&str>) -> bool {
    wanted.is_some_and(|w| {
        w.eq_ignore_ascii_case(unit) || (seconds_per(w).is_some() && seconds_per(unit).is_some())
    })
}

/// A number for tables: no trailing zeros.
#[must_use]
pub fn format_number(value: f64) -> String {
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numeric_expectations() {
        let e = parse_expectation("p95 < 20ms").unwrap();
        assert_eq!(e.metric.as_deref(), Some("p95"));
        assert_eq!((e.comparison, e.threshold), (Comparison::Less, 20.0));
        assert_eq!(e.unit.as_deref(), Some("ms"));
        let e = parse_expectation(">= 1000 req/s").unwrap();
        assert_eq!(e.metric, None);
        assert_eq!(e.comparison, Comparison::GreaterOrEqual);
        assert_eq!(e.unit.as_deref(), Some("req/s"));
        assert_eq!(
            parse_expectation("errors = 0").unwrap().comparison,
            Comparison::Equal
        );
        assert_eq!(parse_expectation("pasa"), None);
        assert_eq!(parse_expectation("a < b"), None);
    }

    #[test]
    fn finds_the_measurement_of_the_metric() {
        let e = parse_expectation("p95 < 20ms").unwrap();
        let out = "bench sessions\np50: 4.1ms\np95: 12.5 ms\np99: 30ms\n";
        assert_eq!(find_measurement(&e, out), Some(12.5));
        assert_eq!(find_measurement(&e, "p95=0.018s"), Some(18.0));
        assert_eq!(find_measurement(&e, "nada"), None);

        let e = parse_expectation("< 2 s").unwrap();
        assert_eq!(
            find_measurement(&e, "run 3 of 3: 1500ms total, 12 items"),
            Some(1.5)
        );
        let e = parse_expectation(">= 1000").unwrap();
        assert_eq!(find_measurement(&e, "v2 throughput 1234 ops"), Some(1234.0));
    }

    #[test]
    fn comparisons_hold() {
        assert!(Comparison::Less.holds(12.5, 20.0));
        assert!(!Comparison::Greater.holds(1.0, 1.0));
        assert!(Comparison::Equal.holds(0.0, 0.0));
        assert_eq!(format_number(12.5), "12.5");
        assert_eq!(format_number(20.0), "20");
    }
}
