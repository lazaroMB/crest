//! The value axis of a chart: its domain, "nice" ticks (steps of 1, 2 or 5
//! times a power of ten) and how numbers are written on it (compact on the
//! axis and bar tips, full in tooltips).

use domain::block::ChartKind;

/// Ticks the value axis aims for; the nice step may give a few more.
pub const TARGET_TICKS: usize = 5;

/// The range of the values a chart draws. Bars and areas grow from zero,
/// so their domain always includes it; lines fit the data. An empty or
/// non-finite input gives `(0, 1)` and a flat one is widened around itself.
pub fn value_domain(kind: ChartKind, values: impl IntoIterator<Item = f64>) -> (f64, f64) {
    let mut range: Option<(f64, f64)> = None;
    for v in values.into_iter().filter(|v| v.is_finite()) {
        range = Some(match range {
            Some((lo, hi)) => (lo.min(v), hi.max(v)),
            None => (v, v),
        });
    }
    let (mut lo, mut hi) = range.unwrap_or((0.0, 1.0));
    if matches!(kind, ChartKind::Bar | ChartKind::Area) {
        lo = lo.min(0.0);
        hi = hi.max(0.0);
    }
    if lo == hi {
        let pad = if lo == 0.0 { 1.0 } else { lo.abs() * 0.1 };
        if lo >= 0.0 && matches!(kind, ChartKind::Bar | ChartKind::Area) {
            hi += pad;
        } else {
            lo -= pad;
            hi += pad;
        }
    }
    (lo, hi)
}

/// The step of about `target` intervals over `span`: 1, 2 or 5 times a power
/// of ten.
pub fn nice_step(span: f64, target: usize) -> f64 {
    let span = if span.is_finite() && span > 0.0 {
        span
    } else {
        1.0
    };
    let raw = span / target.max(1) as f64;
    let magnitude = 10f64.powf(raw.log10().floor());
    let norm = raw / magnitude;
    let nice = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * magnitude
}

/// Ticks covering `[min, max]` with a nice step, from the last multiple of
/// the step at or below `min` to the first at or above `max`. The first and
/// last tick are the drawn axis range.
pub fn nice_ticks(min: f64, max: f64) -> Vec<f64> {
    nice_ticks_n(min, max, TARGET_TICKS)
}

/// [`nice_ticks`] aiming for `target` intervals.
pub fn nice_ticks_n(min: f64, max: f64, target: usize) -> Vec<f64> {
    let (min, max) = if min <= max { (min, max) } else { (max, min) };
    let step = nice_step(max - min, target);
    let first = (min / step).floor() as i64;
    let last = (max / step).ceil() as i64;
    // `+ 0.0` turns a negative zero into zero.
    (first..=last.max(first + 1))
        .map(|i| i as f64 * step + 0.0)
        .collect()
}

/// `n` without trailing zeros after `decimals` decimals, never `-0`.
fn trimmed(n: f64, decimals: usize) -> String {
    let s = format!("{n:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    };
    if s == "-0" { "0".to_owned() } else { s }
}

/// A short number for axes and bar tips: `812`, `1.2K`, `3M`, `0.25`.
pub fn format_compact(n: f64) -> String {
    if !n.is_finite() {
        return n.to_string();
    }
    const UNITS: [(f64, &str); 4] = [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K")];
    let abs = n.abs();
    for (ix, (unit, suffix)) in UNITS.iter().enumerate() {
        if abs >= *unit {
            let scaled = trimmed(n / unit, 1);
            // 999_950 rounds to "1000K": write it with the next unit.
            if scaled.trim_start_matches('-') == "1000"
                && let Some(&(bigger, suffix)) = ix.checked_sub(1).and_then(|i| UNITS.get(i))
            {
                return format!("{}{suffix}", trimmed(n / bigger, 1));
            }
            return format!("{scaled}{suffix}");
        }
    }
    if abs >= 999.5 {
        // Rounds up to a thousand.
        return format!("{}K", trimmed(n / 1e3, 1));
    }
    let decimals = if abs >= 100.0 {
        0
    } else if abs >= 1.0 {
        2
    } else {
        3
    };
    trimmed(n, decimals)
}

/// The whole number for tooltips: thousands separated with commas and up
/// to four decimals, `1,234,567.5`.
pub fn format_full(n: f64) -> String {
    if !n.is_finite() {
        return n.to_string();
    }
    let s = trimmed(n, 4);
    let (sign, digits) = match s.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", s.as_str()),
    };
    let (int, frac) = match digits.split_once('.') {
        Some((int, frac)) => (int, Some(frac)),
        None => (digits, None),
    };
    let mut grouped = String::with_capacity(int.len() + int.len() / 3);
    for (ix, ch) in int.chars().enumerate() {
        if ix > 0 && (int.len() - ix) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    match frac {
        Some(frac) => format!("{sign}{grouped}.{frac}"),
        None => format!("{sign}{grouped}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_use_steps_of_one_two_or_five() {
        let ticks = nice_ticks(0.0, 812.0);
        assert_eq!(ticks, [0.0, 200.0, 400.0, 600.0, 800.0, 1000.0]);
        assert_eq!(nice_step(1.0, 5), 0.2);
        assert_eq!(nice_step(40.0, 5), 10.0);
        assert_eq!(nice_step(0.07, 5), 0.02);
        let around_zero = nice_ticks(-35.0, 60.0);
        assert_eq!(around_zero.first(), Some(&-40.0));
        assert_eq!(around_zero.last(), Some(&60.0));
        assert!(around_zero.contains(&0.0));
        assert!(
            around_zero
                .iter()
                .all(|t| !(t.is_sign_negative() && *t == 0.0))
        );
    }

    #[test]
    fn flat_and_empty_data_still_get_two_ticks() {
        assert!(nice_ticks(5.0, 5.0).len() >= 2);
        let (lo, hi) = value_domain(ChartKind::Line, []);
        assert!(lo < hi);
        let (lo, hi) = value_domain(ChartKind::Line, [3.0, 3.0]);
        assert!(lo < 3.0 && hi > 3.0);
    }

    #[test]
    fn bars_and_areas_include_zero_lines_fit_the_data() {
        assert_eq!(value_domain(ChartKind::Bar, [5.0, 9.0]), (0.0, 9.0));
        assert_eq!(value_domain(ChartKind::Area, [-5.0, -2.0]), (-5.0, 0.0));
        assert_eq!(value_domain(ChartKind::Line, [5.0, 9.0]), (5.0, 9.0));
        assert_eq!(
            value_domain(ChartKind::Bar, [f64::NAN, 2.0, f64::INFINITY]),
            (0.0, 2.0)
        );
    }

    #[test]
    fn compact_numbers() {
        assert_eq!(format_compact(812.0), "812");
        assert_eq!(format_compact(1200.0), "1.2K");
        assert_eq!(format_compact(3_000_000.0), "3M");
        assert_eq!(format_compact(-1500.0), "-1.5K");
        assert_eq!(format_compact(2.5e9), "2.5B");
        assert_eq!(format_compact(999_950.0), "1M");
        assert_eq!(format_compact(999.7), "1K");
        assert_eq!(format_compact(12.5), "12.5");
        assert_eq!(format_compact(0.25), "0.25");
        assert_eq!(format_compact(0.0), "0");
        assert_eq!(format_compact(-0.0), "0");
        assert_eq!(format_compact(-0.0001), "0");
        assert_eq!(format_compact(0.1 + 0.2), "0.3");
    }

    #[test]
    fn full_numbers() {
        assert_eq!(format_full(1_234_567.5), "1,234,567.5");
        assert_eq!(format_full(-1234.0), "-1,234");
        assert_eq!(format_full(999.0), "999");
        assert_eq!(format_full(0.12345), "0.1235");
        assert_eq!(format_full(-0.00001), "0");
    }
}
