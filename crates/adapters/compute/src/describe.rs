//! Descriptive statistics: per-variable summaries, by group when asked.
//!
//! Conventions: the sample SD has n − 1 in the denominator; quartiles are
//! Hyndman-Fan type 7 (R's default, numpy's `linear`); skewness is the
//! adjusted Fisher-Pearson G1 and kurtosis the excess G2 (SPSS, Excel,
//! pandas); histogram bins follow Freedman-Diaconis, falling back to
//! Sturges when the IQR is zero.

use application::ports::compute::{ColumnValues, ComputeError, DescribeRequest};
use domain::computed::{
    Computed, ComputedBody, DescriptiveBody, Histogram, LevelCount, Narrative, VariableKind,
    VariableSummary,
};
use serde_json::Value;

use crate::data::{self, key_part};
use crate::envelope::{self, auto_decimals, count, number};
use crate::reproduce::{py_frame, py_str, r_col, r_frame, r_str};

/// Location and spread of a numeric sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// Values.
    pub n: usize,
    /// Mean.
    pub mean: f64,
    /// Sample standard deviation (NaN below 2 values).
    pub sd: f64,
    /// Median.
    pub median: f64,
    /// First quartile, type 7.
    pub q1: f64,
    /// Third quartile, type 7.
    pub q3: f64,
    /// Minimum.
    pub min: f64,
    /// Maximum.
    pub max: f64,
    /// G1 skewness (NaN below 3 values or when constant).
    pub skew: f64,
    /// G2 excess kurtosis (NaN below 4 values or when constant).
    pub kurtosis: f64,
}

/// The mean, by a corrected two-pass sum.
pub fn mean(xs: &[f64]) -> f64 {
    let n = xs.len() as f64;
    let m = xs.iter().sum::<f64>() / n;
    m + xs.iter().map(|x| x - m).sum::<f64>() / n
}

/// The sample variance (n − 1), by the corrected two-pass algorithm.
pub fn variance(xs: &[f64]) -> f64 {
    let n = xs.len() as f64;
    if xs.len() < 2 {
        return f64::NAN;
    }
    let m = mean(xs);
    let (ss, s) = xs.iter().fold((0.0, 0.0), |(ss, s), x| {
        let d = x - m;
        (ss + d * d, s + d)
    });
    (ss - s * s / n) / (n - 1.0)
}

/// The type-7 quantile of sorted values.
pub fn quantile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let h = (sorted.len() - 1) as f64 * p;
    let lo = h.floor();
    // `lo` is within 0..len by construction.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let i = lo as usize;
    match sorted.get(i + 1) {
        Some(next) => sorted[i] + (h - lo) * (next - sorted[i]),
        None => sorted[i],
    }
}

/// Summary of a non-empty sample.
pub fn summarize(xs: &[f64]) -> Option<Summary> {
    if xs.is_empty() {
        return None;
    }
    let mut sorted = xs.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = xs.len();
    let nf = n as f64;
    let m = mean(xs);
    let sd = variance(xs).sqrt();
    let central = |k: i32| xs.iter().map(|x| (x - m).powi(k)).sum::<f64>() / nf;
    let (m2, m3, m4) = (central(2), central(3), central(4));
    let skew = if n >= 3 && m2 > 0.0 {
        (nf * (nf - 1.0)).sqrt() / (nf - 2.0) * m3 / m2.powf(1.5)
    } else {
        f64::NAN
    };
    let kurtosis = if n >= 4 && m2 > 0.0 {
        let g2 = m4 / (m2 * m2) - 3.0;
        (nf - 1.0) / ((nf - 2.0) * (nf - 3.0)) * ((nf + 1.0) * g2 + 6.0)
    } else {
        f64::NAN
    };
    Some(Summary {
        n,
        mean: m,
        sd,
        median: quantile(&sorted, 0.5),
        q1: quantile(&sorted, 0.25),
        q3: quantile(&sorted, 0.75),
        min: sorted[0],
        max: sorted[n - 1],
        skew,
        kurtosis,
    })
}

/// Freedman-Diaconis histogram of a non-empty sample (Sturges when the IQR
/// is zero), at most 100 bins.
pub fn histogram(xs: &[f64]) -> Option<Histogram> {
    let summary = summarize(xs)?;
    let (min, max) = (summary.min, summary.max);
    if max == min {
        return Some(Histogram {
            edges: vec![min - 0.5, max + 0.5],
            counts: vec![xs.len() as u64],
        });
    }
    let n = xs.len() as f64;
    let width = 2.0 * (summary.q3 - summary.q1) / n.cbrt();
    let bins = if width > 0.0 {
        ((max - min) / width).ceil()
    } else {
        n.log2().ceil() + 1.0
    };
    // Clamped to 1..=100 before the cast.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bins = bins.clamp(1.0, 100.0) as usize;
    let step = (max - min) / bins as f64;
    let mut edges: Vec<f64> = (0..=bins).map(|i| min + step * i as f64).collect();
    edges[bins] = max;
    let mut counts = vec![0u64; bins];
    for x in xs {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = (((x - min) / step).floor() as usize).min(bins - 1);
        counts[i] += 1;
    }
    Some(Histogram { edges, counts })
}

/// One summary row and its values.
fn numeric_row(
    computed: &mut Computed,
    name: &str,
    group: Option<&str>,
    unit: Option<&String>,
    values: &[Option<f64>],
) -> VariableSummary {
    let xs = data::present(values);
    let prefix = match group {
        Some(g) => format!("{}.{}", key_part(name), key_part(g)),
        None => key_part(name),
    };
    let mut row = VariableSummary {
        name: name.to_owned(),
        group: group.map(str::to_owned),
        kind: VariableKind::Numeric,
        unit: unit.cloned(),
        n: xs.len() as u64,
        missing: data::missing(values),
        mean: None,
        sd: None,
        median: None,
        q1: None,
        q3: None,
        min: None,
        max: None,
        skew: None,
        kurtosis: None,
        histogram: histogram(&xs),
        levels: Vec::new(),
    };
    computed.values.insert(format!("{prefix}.n"), count(row.n));
    computed
        .values
        .insert(format!("{prefix}.missing"), count(row.missing));
    let Some(s) = summarize(&xs) else {
        return row;
    };
    let scale = if s.sd.is_finite() && s.sd > 0.0 {
        s.sd
    } else {
        s.mean.abs()
    };
    let decimals = auto_decimals(scale);
    let finite = |x: f64| x.is_finite().then_some(x);
    row.mean = Some(s.mean);
    row.sd = finite(s.sd);
    row.median = Some(s.median);
    row.q1 = Some(s.q1);
    row.q3 = Some(s.q3);
    row.min = Some(s.min);
    row.max = Some(s.max);
    row.skew = finite(s.skew);
    row.kurtosis = finite(s.kurtosis);
    let fields = [
        ("mean", s.mean, decimals),
        ("sd", s.sd, decimals),
        ("median", s.median, decimals),
        ("q1", s.q1, decimals),
        ("q3", s.q3, decimals),
        ("min", s.min, decimals),
        ("max", s.max, decimals),
        ("skew", s.skew, 2),
        ("kurtosis", s.kurtosis, 2),
    ];
    for (key, v, d) in fields {
        if v.is_finite() {
            let mut value = number(v, d);
            if key != "skew" && key != "kurtosis" {
                value.unit = unit.cloned();
            }
            computed.values.insert(format!("{prefix}.{key}"), value);
        }
    }
    row
}

fn categorical_row(
    computed: &mut Computed,
    name: &str,
    group: Option<&str>,
    values: &[Option<String>],
) -> VariableSummary {
    let mut levels: Vec<LevelCount> = Vec::new();
    for v in values.iter().flatten() {
        match levels.iter_mut().find(|l| &l.level == v) {
            Some(level) => level.count += 1,
            None => levels.push(LevelCount {
                level: v.clone(),
                count: 1,
                percent: 0.0,
            }),
        }
    }
    let n: u64 = levels.iter().map(|l| l.count).sum();
    for level in &mut levels {
        #[allow(clippy::cast_precision_loss)]
        let share = level.count as f64 / n as f64;
        level.percent = 100.0 * share;
    }
    levels.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.level.cmp(&b.level)));
    let prefix = match group {
        Some(g) => format!("{}.{}", key_part(name), key_part(g)),
        None => key_part(name),
    };
    computed.values.insert(format!("{prefix}.n"), count(n));
    let missing = data::missing(values);
    computed
        .values
        .insert(format!("{prefix}.missing"), count(missing));
    for level in &levels {
        let key = format!("{prefix}.{}", key_part(&level.level));
        computed
            .values
            .insert(format!("{key}.count"), count(level.count));
        let mut percent = number(level.percent, 1);
        percent.unit = Some("%".into());
        computed.values.insert(format!("{key}.percent"), percent);
    }
    VariableSummary {
        name: name.to_owned(),
        group: group.map(str::to_owned),
        kind: VariableKind::Categorical,
        unit: None,
        n,
        missing,
        mean: None,
        sd: None,
        median: None,
        q1: None,
        q3: None,
        min: None,
        max: None,
        skew: None,
        kurtosis: None,
        histogram: None,
        levels,
    }
}

/// Runs a describe request.
pub fn run(
    request: &DescribeRequest,
    id: String,
) -> Result<(Computed, ComputedBody), ComputeError> {
    let data = &request.data;
    let names: Vec<String> = if request.variables.is_empty() {
        data.columns
            .iter()
            .map(|c| c.name.clone())
            .filter(|n| Some(n) != request.by.as_ref())
            .collect()
    } else {
        request.variables.clone()
    };
    if names.is_empty() {
        return Err(ComputeError::Invalid("no variables to describe".into()));
    }
    let groups = match &request.by {
        Some(by) => Some(data::labels(data, by)?),
        None => None,
    };
    let mut computed = Computed::new(
        id,
        "stats.describe",
        "Descriptive statistics",
        envelope::engine(),
    );
    computed
        .method
        .params
        .insert("quantiles".into(), Value::from("type 7"));
    computed.method.params.insert(
        "skewness".into(),
        Value::from("G1 (adjusted Fisher-Pearson)"),
    );
    computed
        .method
        .params
        .insert("kurtosis".into(), Value::from("G2 (excess)"));
    computed
        .method
        .params
        .insert("histogram".into(), Value::from("Freedman-Diaconis"));
    let mut rows = Vec::new();
    for name in &names {
        let column = data::column(data, name)?;
        // One pass per group (or a single pass with every row).
        let passes: Vec<(Option<&str>, Vec<bool>)> = match &groups {
            Some((labels, levels)) => levels
                .iter()
                .map(|level| {
                    let mask = labels.iter().map(|l| l.as_deref() == Some(level)).collect();
                    (Some(level.as_str()), mask)
                })
                .collect(),
            None => vec![(None, vec![true; column.len()])],
        };
        for (group, mask) in passes {
            let row = match &column.values {
                ColumnValues::Numeric(values) => {
                    let values: Vec<Option<f64>> = values
                        .iter()
                        .zip(&mask)
                        .filter(|(_, keep)| **keep)
                        .map(|(v, _)| v.filter(|x| x.is_finite()))
                        .collect();
                    numeric_row(&mut computed, name, group, column.unit.as_ref(), &values)
                }
                ColumnValues::Categorical(values) => {
                    let values: Vec<Option<String>> = values
                        .iter()
                        .zip(&mask)
                        .filter(|(_, keep)| **keep)
                        .map(|(v, _)| v.clone())
                        .collect();
                    categorical_row(&mut computed, name, group, &values)
                }
            };
            if row.kind == VariableKind::Numeric && row.n > 1 && row.sd == Some(0.0) {
                computed.warnings.push(envelope::warning(
                    "constant_column",
                    domain::computed::WarningSeverity::Info,
                    format!(
                        "{name} is constant{}",
                        group.map(|g| format!(" in group {g}")).unwrap_or_default()
                    ),
                ));
            }
            rows.push(row);
        }
    }
    let mut used: Vec<&str> = names.iter().map(String::as_str).collect();
    if let Some(by) = &request.by {
        used.push(by);
    }
    let used = data::used_columns(data, &used);
    computed.inputs.push(envelope::input(data, &used, None));
    let first = rows.first().map(|r| r.name.clone()).unwrap_or_default();
    let first_key = key_part(&first);
    computed.narrative = Some(Narrative {
        template: "describe".into(),
        text_slots: ["n", "mean", "sd"]
            .iter()
            .map(|k| format!("{first_key}.{k}"))
            .filter(|k| computed.values.contains_key(k))
            .collect(),
    });
    let quoted: Vec<String> = names.iter().map(|n| py_str(n)).collect();
    let py_cols = format!("df[[{}]]", quoted.join(", "));
    computed.reproduce.python = Some(match &request.by {
        Some(by) => format!(
            "{}; df.groupby({})[[{}]].describe()",
            py_frame(data, &used),
            py_str(by),
            quoted.join(", ")
        ),
        None => format!(
            "{}; {py_cols}.describe(), {py_cols}.skew(), {py_cols}.kurt()",
            py_frame(data, &used)
        ),
    });
    let r_names: Vec<String> = names.iter().map(|n| r_str(n)).collect();
    let r_cols = format!("df[c({})]", r_names.join(", "));
    computed.reproduce.r = Some(match &request.by {
        Some(by) => format!(
            "{}; by({r_cols}, {}, summary)",
            r_frame(data, &used),
            r_col(by)
        ),
        None => format!(
            "{}; summary({r_cols}); sapply({r_cols}, sd, na.rm = TRUE)",
            r_frame(data, &used)
        ),
    });
    let body = ComputedBody::Descriptive(DescriptiveBody {
        by: request.by.clone(),
        variables: rows,
    });
    Ok((computed, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_are_type_7() {
        let sorted = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(quantile(&sorted, 0.25), 1.75);
        assert_eq!(quantile(&sorted, 0.5), 2.5);
        assert_eq!(quantile(&sorted, 1.0), 4.0);
    }

    #[test]
    fn summary_moments_match_hand_values() {
        let s = summarize(&[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]).unwrap_or_else(|| panic!());
        assert_eq!(s.mean, 5.0);
        assert!((s.sd - (32.0f64 / 7.0).sqrt()).abs() < 1e-12);
        // pandas: Series([2,4,4,4,5,5,7,9]).skew() = 0.8184875533567997,
        // .kurt() = 0.940625.
        assert!((s.skew - 0.818_487_553_356_799_7).abs() < 1e-12);
        assert!((s.kurtosis - 0.940_625).abs() < 1e-12);
    }

    #[test]
    fn histogram_counts_every_value() {
        let xs: Vec<f64> = (0..100).map(f64::from).collect();
        let h = histogram(&xs).unwrap_or_else(|| panic!());
        assert_eq!(h.validate(), Ok(()));
        assert_eq!(h.counts.iter().sum::<u64>(), 100);
        let constant = histogram(&[3.0, 3.0]).unwrap_or_else(|| panic!());
        assert_eq!(constant.counts, vec![2]);
    }
}
