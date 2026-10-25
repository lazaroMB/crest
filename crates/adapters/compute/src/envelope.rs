//! Filling the parts of a [`Computed`] envelope every computation shares:
//! value formatting, input hashes, engine, ids, status and provenance.

use std::time::{SystemTime, UNIX_EPOCH};

use application::ports::compute::{ComputeRequest, DataColumn, DataSet, REQUEST_PARAM};
use domain::computed::{
    Computed, ComputedStatus, ComputedValue, ComputedWarning, Engine, InputRef, Interval,
    ValueFormat, WarningSeverity, apa,
};

/// Engine name in every envelope.
pub const ENGINE_NAME: &str = "crest-stats";
/// The library underneath.
pub const BACKEND: &str = "statrs 0.19.1";

/// Engine name of calc sheets and conversions.
pub const CALC_ENGINE_NAME: &str = "crest-calc";
/// The units library underneath.
pub const CALC_BACKEND: &str = "numbat 1.24.0";

/// Engine name of the solver cards (LP, equations, Monte Carlo, cash
/// flow, decision matrix).
pub const SOLVE_ENGINE_NAME: &str = "crest-solve";

/// The engine record: crate version and target platform.
pub fn engine() -> Engine {
    Engine {
        name: ENGINE_NAME.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        backend: Some(BACKEND.into()),
        platform: Some(platform()),
    }
}

/// The engine record of calc sheets and conversions.
pub fn calc_engine() -> Engine {
    Engine {
        name: CALC_ENGINE_NAME.into(),
        backend: Some(CALC_BACKEND.into()),
        ..engine()
    }
}

/// The engine record of the solver cards; each sets its own backend.
pub fn solve_engine() -> Engine {
    Engine {
        name: SOLVE_ENGINE_NAME.into(),
        backend: None,
        ..engine()
    }
}

fn platform() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// `blake3:<hex>` of `bytes`.
pub fn hash(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

/// A deterministic id for a request: `st_` (statistics), `ca_` (calc
/// sheets and conversions) or `so_` (solver cards) and 12 hex digits of
/// its hash.
pub fn derive_id(request: &ComputeRequest) -> String {
    let bytes = serde_json::to_vec(request).unwrap_or_default();
    let hex = blake3::hash(&bytes).to_hex();
    let prefix = match request {
        ComputeRequest::Calc(_) | ComputeRequest::Convert(_) => "ca",
        ComputeRequest::Optimize(_) | ComputeRequest::Solve(_) | ComputeRequest::Simulate(_) => {
            "so"
        }
        _ => "st",
    };
    format!("{prefix}_{}", &hex.as_str()[..12])
}

/// The input record of `data`: its source with the hash of the canonical
/// JSON of the columns read (in request order) plus any `extra` input such
/// as a count table, the row count and the column names.
pub fn input(data: &DataSet, used: &[DataColumn], extra: Option<&serde_json::Value>) -> InputRef {
    let mut bytes = serde_json::to_vec(used).unwrap_or_default();
    if let Some(extra) = extra {
        bytes.extend(serde_json::to_vec(extra).unwrap_or_default());
    }
    let rows = used.first().map(DataColumn::len).unwrap_or_default() as u64;
    InputRef {
        hash: Some(hash(&bytes)),
        rows: Some(rows),
        columns: used.iter().map(|c| c.name.clone()).collect(),
        role: data.source.role.clone().or_else(|| Some("data".into())),
        filter: data.source.filter.clone().or_else(|| Some("none".into())),
        ..data.source.clone()
    }
}

/// Decimals that show `scale` (a standard deviation, a typical magnitude)
/// with about three significant digits.
pub fn auto_decimals(scale: f64) -> u32 {
    if !(scale.is_finite() && scale > 0.0) {
        return 2;
    }
    // log10 of a finite positive number fits in i32.
    #[allow(clippy::cast_possible_truncation)]
    let exponent = scale.log10().floor() as i32;
    (2 - exponent).clamp(0, 8).unsigned_abs()
}

/// A number shown with `decimals`.
pub fn number(v: f64, decimals: u32) -> ComputedValue {
    ComputedValue {
        display: Some(apa::number(v, decimals)),
        decimals: Some(decimals),
        ..ComputedValue::new(v)
    }
}

/// A count.
pub fn count(n: u64) -> ComputedValue {
    #[allow(clippy::cast_precision_loss)]
    number(n as f64, 0)
}

/// Degrees of freedom.
pub fn df(v: f64) -> ComputedValue {
    ComputedValue {
        display: Some(apa::df_text(v)),
        ..ComputedValue::new(v)
    }
}

/// A p-value in APA style.
pub fn p_value(v: f64) -> ComputedValue {
    ComputedValue {
        display: Some(apa::p_value_text(v)),
        fmt: Some(ValueFormat::Apa),
        ..ComputedValue::new(v)
    }
}

/// A statistic bounded by 1 (r, R², V), without the leading zero.
pub fn bounded(v: f64, decimals: u32) -> ComputedValue {
    ComputedValue {
        display: Some(apa::bounded(v, decimals)),
        decimals: Some(decimals),
        fmt: Some(ValueFormat::Bounded),
        ..ComputedValue::new(v)
    }
}

/// A value shown with `sig` significant digits (AIC, condition numbers).
pub fn significant(v: f64, sig: u32) -> ComputedValue {
    ComputedValue {
        display: Some(apa::significant(v, sig)),
        sig: Some(sig),
        ..ComputedValue::new(v)
    }
}

/// `value` with a two-sided interval, when the bounds are finite.
pub fn with_ci(
    mut value: ComputedValue,
    level: f64,
    lo: f64,
    hi: f64,
    method: &str,
) -> ComputedValue {
    if lo.is_finite() && hi.is_finite() && lo <= hi {
        value.ci = Some(Interval {
            level,
            lo,
            hi,
            method: Some(method.into()),
        });
    }
    value
}

/// `value` with a verbal label and its rule.
pub fn labelled(mut value: ComputedValue, label: &str, rule: &str) -> ComputedValue {
    value.label = Some(label.into());
    value.label_rule = Some(rule.into());
    value
}

/// A warning.
pub fn warning(code: &str, severity: WarningSeverity, text: impl Into<String>) -> ComputedWarning {
    ComputedWarning {
        code: code.into(),
        severity,
        text: text.into(),
    }
}

/// Marks `computed` as failed with `error`, dropping its values.
pub fn fail(computed: &mut Computed, error: impl Into<String>) {
    computed.status = ComputedStatus::Error;
    computed.error = Some(error.into());
    computed.values.clear();
}

/// The common last step: stores the request, drops values that could not
/// be computed (with a warning), sets the status from the warnings and
/// fills the environment hash and provenance.
pub fn finish(
    computed: &mut Computed,
    request: &ComputeRequest,
    started: SystemTime,
    ended: SystemTime,
) {
    computed.method.params.insert(
        REQUEST_PARAM.into(),
        serde_json::to_value(request).unwrap_or_default(),
    );
    let bad: Vec<String> = computed
        .values
        .iter()
        .filter(|(_, v)| !v.v.is_finite())
        .map(|(k, _)| k.clone())
        .collect();
    for key in &bad {
        computed.values.remove(key);
    }
    for value in computed.values.values_mut() {
        let ok = value
            .ci
            .as_ref()
            .is_some_and(|ci| ci.lo.is_finite() && ci.hi.is_finite() && ci.lo <= ci.hi);
        if !ok {
            value.ci = None;
        }
    }
    if !bad.is_empty() {
        computed.warnings.push(warning(
            "not_computable",
            WarningSeverity::Info,
            format!("could not be computed from these data: {}", bad.join(", ")),
        ));
    }
    if computed.status != ComputedStatus::Error
        && computed
            .warnings
            .iter()
            .any(|w| w.severity != WarningSeverity::Info)
    {
        computed.status = ComputedStatus::Warning;
    }
    let env = format!(
        "{} {} {} {}",
        computed.engine.name,
        computed.engine.version,
        computed.engine.backend.as_deref().unwrap_or(BACKEND),
        platform()
    );
    computed.environment_hash = Some(hash(env.as_bytes()));
    computed.provenance.activity_started = Some(rfc3339(started));
    computed.provenance.duration_ms = Some(
        ended
            .duration_since(started)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or_default(),
    );
}

/// `time` as RFC 3339 in UTC, to the second: `2026-10-24T16:40:00Z`.
pub fn rfc3339(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let days = i64::try_from(secs / 86_400).unwrap_or_default();
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to (year, month, day), Howard Hinnant's
/// `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        u32::try_from(m).unwrap_or_default(),
        u32::try_from(d).unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn rfc3339_formats_utc() {
        let t = UNIX_EPOCH + Duration::from_secs(1_792_860_000);
        assert_eq!(rfc3339(t), "2026-10-24T16:40:00Z");
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn auto_decimals_keeps_three_digits_of_the_scale() {
        assert_eq!(auto_decimals(79.0), 1);
        assert_eq!(auto_decimals(1.8), 2);
        assert_eq!(auto_decimals(0.079), 4);
        assert_eq!(auto_decimals(0.0), 2);
        assert_eq!(auto_decimals(25_000.0), 0);
    }
}
