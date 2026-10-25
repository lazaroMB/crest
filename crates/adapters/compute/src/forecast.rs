//! The forecast card, on `augurs`: MSTL (Bandara, Bergmeir and Hewamalage
//! 2021, through `stlrs`) splits the series into trend, one seasonal
//! component per period and a remainder; AutoETS (the ETS model with the
//! lowest AICc, as statsforecast) forecasts the deseasonalised series and
//! the seasonal components are carried forward. Without periods AutoETS
//! runs on the series itself. Intervals are the ETS ones at 80% and 95%.
//!
//! Accuracy is measured honestly: the last `holdout` observations are
//! left out, the model is refitted without them and their forecast is
//! compared with what happened (MAPE, sMAPE, MAE). The final forecast
//! uses the whole series. The decomposition runs in single precision
//! (`stlrs`), so components carry about seven significant digits.

use application::ports::compute::{ComputeError, DataColumn, ForecastRequest};
use augurs_core::{Fit as _, Forecast, Predict as _};
use augurs_ets::AutoETS;
use augurs_ets::trend::AutoETSTrendModel;
use augurs_mstl::MSTLModel;
use domain::computed::{
    Assumption, AssumptionStatus, Computed, ComputedBody, ForecastBody, Holdout, Narrative,
    SeasonalComponent, UncertaintyKind, WarningSeverity,
};
use serde_json::Value;

use crate::data;
use crate::describe::variance;
use crate::envelope::{self, auto_decimals, bounded, count, number, warning, with_ci};
use crate::reproduce::{py_col, py_frame, r_col, r_frame};

/// Most steps forecast.
pub const MAX_HORIZON: usize = 1000;
/// Most observations.
pub const MAX_SERIES: usize = 200_000;
/// The backend recorded in the envelope.
pub const AUGURS: &str = "augurs 0.10.2 (MSTL, AutoETS)";

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// A fitted model's forecast at both levels.
struct Paths {
    point: Vec<f64>,
    lo80: Vec<f64>,
    hi80: Vec<f64>,
    lo95: Vec<f64>,
    hi95: Vec<f64>,
}

fn intervals(f: Forecast) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let (lo, hi) = f.intervals.map(|i| (i.lower, i.upper)).unwrap_or_default();
    (f.point, lo, hi)
}

/// Fits on `y` and forecasts `horizon` steps; with periods also returns
/// the decomposition (trend, seasonal per period, remainder).
#[allow(clippy::type_complexity)]
fn fit_and_forecast(
    y: &[f64],
    periods: &[usize],
    horizon: usize,
) -> Result<(Paths, Option<(Vec<f64>, Vec<Vec<f64>>, Vec<f64>)>), String> {
    let ets = AutoETS::non_seasonal();
    let to_f64 = |v: &[f32]| v.iter().map(|x| f64::from(*x)).collect::<Vec<f64>>();
    if periods.is_empty() {
        let fitted = ets.fit(y).map_err(|e| e.to_string())?;
        let (point, lo80, hi80) =
            intervals(fitted.predict(horizon, 0.8).map_err(|e| e.to_string())?);
        let (_, lo95, hi95) = intervals(fitted.predict(horizon, 0.95).map_err(|e| e.to_string())?);
        return Ok((
            Paths {
                point,
                lo80,
                hi80,
                lo95,
                hi95,
            },
            None,
        ));
    }
    let model = MSTLModel::new(periods.to_vec(), AutoETSTrendModel::from(ets));
    let fitted = model.fit(y).map_err(|e| e.to_string())?;
    let (point, lo80, hi80) = intervals(fitted.predict(horizon, 0.8).map_err(|e| e.to_string())?);
    let (_, lo95, hi95) = intervals(fitted.predict(horizon, 0.95).map_err(|e| e.to_string())?);
    let fit = fitted.fit();
    let parts = (
        to_f64(fit.trend()),
        fit.seasonal().iter().map(|s| to_f64(s)).collect(),
        to_f64(fit.remainder()),
    );
    Ok((
        Paths {
            point,
            lo80,
            hi80,
            lo95,
            hi95,
        },
        Some(parts),
    ))
}

/// Accuracy of `predicted` against `actual`: (MAPE %, sMAPE %, MAE); the
/// MAPE is `None` when an actual value is 0.
pub fn accuracy(actual: &[f64], predicted: &[f64]) -> (Option<f64>, f64, f64) {
    let n = actual.len() as f64;
    let mae = actual
        .iter()
        .zip(predicted)
        .map(|(a, p)| (a - p).abs())
        .sum::<f64>()
        / n;
    let mape = (actual.iter().all(|a| *a != 0.0)).then(|| {
        100.0
            * actual
                .iter()
                .zip(predicted)
                .map(|(a, p)| ((a - p) / a).abs())
                .sum::<f64>()
            / n
    });
    let smape = 100.0
        * actual
            .iter()
            .zip(predicted)
            .map(|(a, p)| {
                let d = a.abs() + p.abs();
                if d == 0.0 {
                    0.0
                } else {
                    2.0 * (a - p).abs() / d
                }
            })
            .sum::<f64>()
        / n;
    (mape, smape, mae)
}

/// Strength of a component against the remainder (Wang, Smith and
/// Hyndman 2006): max(0, 1 − Var(R) / Var(C + R)).
pub fn strength(component: &[f64], remainder: &[f64]) -> f64 {
    let sum: Vec<f64> = component
        .iter()
        .zip(remainder)
        .map(|(c, r)| c + r)
        .collect();
    (1.0 - variance(remainder) / variance(&sum)).max(0.0)
}

/// Runs a forecast request.
pub fn run(
    request: &ForecastRequest,
    id: String,
) -> Result<(Computed, ComputedBody), ComputeError> {
    if request.horizon == 0 || request.horizon > MAX_HORIZON {
        return Err(invalid(format!(
            "`horizon` must be between 1 and {MAX_HORIZON}"
        )));
    }
    let mut periods = request.periods.clone();
    periods.sort_unstable();
    periods.dedup();
    if periods.iter().any(|p| *p < 2) {
        return Err(invalid(
            "seasonal `periods` must be at least 2 observations",
        ));
    }
    let values = data::numeric(&request.data, &request.y)?;
    if values.iter().any(Option::is_none) {
        return Err(invalid(format!(
            "`{}` has missing values; a forecast needs every time step (fill or drop them)",
            request.y
        )));
    }
    let y: Vec<f64> = data::present(&values);
    if y.len() > MAX_SERIES {
        return Err(invalid(format!("at most {MAX_SERIES} observations")));
    }
    let time: Vec<String> = match &request.time {
        Some(column) => {
            let (rows, _) = data::labels(&request.data, column)?;
            rows.into_iter().map(Option::unwrap_or_default).collect()
        }
        None => Vec::new(),
    };
    let mut names = vec![request.y.as_str()];
    if let Some(t) = &request.time {
        names.push(t);
    }
    let used = data::used_columns(&request.data, &names);
    let model_name = if periods.is_empty() {
        "AutoETS".to_owned()
    } else {
        let list: Vec<String> = periods.iter().map(ToString::to_string).collect();
        format!("MSTL (periods {}) + AutoETS trend", list.join(", "))
    };
    let mut engine = envelope::engine();
    engine.backend = Some(AUGURS.into());
    let mut computed = Computed::new(
        id,
        "stats.forecast",
        format!("Forecast: {model_name}"),
        engine,
    );
    computed.uncertainty_kind = Some(UncertaintyKind::FrequentistCi);
    computed
        .inputs
        .push(envelope::input(&request.data, &used, None));
    reproduce(&mut computed, request, &periods, &used);
    let mut body = ForecastBody {
        variable: request.y.clone(),
        time,
        model: model_name,
        values: y.clone(),
        trend: Vec::new(),
        seasonal: Vec::new(),
        remainder: Vec::new(),
        forecast: Vec::new(),
        lo80: Vec::new(),
        hi80: Vec::new(),
        lo95: Vec::new(),
        hi95: Vec::new(),
        holdout: None,
    };
    let n = y.len();
    let longest = periods.last().copied().unwrap_or(0);
    if n < 10 || (longest > 0 && n < 2 * longest) {
        envelope::fail(
            &mut computed,
            format!(
                "{n} observations are too few: a forecast needs at least 10{}",
                if longest > 0 {
                    format!(
                        " and two full cycles of the longest period ({})",
                        2 * longest
                    )
                } else {
                    String::new()
                }
            ),
        );
        return Ok((computed, ComputedBody::Forecast(body)));
    }
    // The hold-out: refit without the tail and forecast it.
    let holdout = request
        .holdout
        .unwrap_or_else(|| request.horizon.min(n / 5))
        .min(n.saturating_sub(10.max(2 * longest)));
    let decimals = auto_decimals(variance(&y).sqrt());
    if holdout > 0 {
        match fit_and_forecast(&y[..n - holdout], &periods, holdout) {
            Ok((paths, _)) => {
                let actual = &y[n - holdout..];
                let (mape, smape, mae) = accuracy(actual, &paths.point);
                if let Some(mape) = mape {
                    computed.values.insert("mape".into(), number(mape, 2));
                }
                computed.values.insert("smape".into(), number(smape, 2));
                computed.values.insert("mae".into(), number(mae, decimals));
                computed
                    .values
                    .insert("holdout".into(), count(holdout as u64));
                body.holdout = Some(Holdout {
                    start: n - holdout,
                    predicted: paths.point,
                });
            }
            Err(e) => computed.warnings.push(warning(
                "holdout_failed",
                WarningSeverity::Info,
                format!("the hold-out fit failed ({e}); no accuracy measured"),
            )),
        }
    }
    let (paths, parts) = match fit_and_forecast(&y, &periods, request.horizon) {
        Ok(done) => done,
        Err(e) => {
            envelope::fail(&mut computed, format!("the model could not be fitted: {e}"));
            return Ok((computed, ComputedBody::Forecast(body)));
        }
    };
    if let Some((trend, seasonal, remainder)) = parts {
        computed.values.insert(
            "trend_strength".into(),
            bounded(strength(&trend, &remainder), 2),
        );
        for (period, values) in periods.iter().zip(&seasonal) {
            computed.values.insert(
                format!("seasonal_strength_{period}"),
                bounded(strength(values, &remainder), 2),
            );
        }
        body.trend = trend;
        body.seasonal = periods
            .iter()
            .zip(seasonal)
            .map(|(&period, values)| SeasonalComponent { period, values })
            .collect();
        body.remainder = remainder;
    }
    let h = request.horizon;
    let at = |i: usize, v: &[f64]| v.get(i).copied().unwrap_or(f64::NAN);
    for (key, i) in [("next", 0), ("end", h - 1)] {
        computed.values.insert(
            key.into(),
            with_ci(
                number(at(i, &paths.point), decimals),
                0.95,
                at(i, &paths.lo95),
                at(i, &paths.hi95),
                "ets_95",
            ),
        );
    }
    computed.values.insert("horizon".into(), count(h as u64));
    computed.values.insert("n".into(), count(n as u64));
    body.forecast = paths.point;
    body.lo80 = paths.lo80;
    body.hi80 = paths.hi80;
    body.lo95 = paths.lo95;
    body.hi95 = paths.hi95;
    if body.lo95.len() != h {
        // A model without intervals: keep the point forecast only.
        body.lo80 = body.forecast.clone();
        body.hi80 = body.forecast.clone();
        body.lo95 = body.forecast.clone();
        body.hi95 = body.forecast.clone();
        computed.warnings.push(warning(
            "no_intervals",
            WarningSeverity::Warn,
            "the model gave no prediction intervals",
        ));
    }
    let params = &mut computed.method.params;
    params.insert("periods".into(), Value::from(periods.clone()));
    params.insert("horizon".into(), Value::from(h));
    params.insert("holdout".into(), Value::from(holdout));
    params.insert("levels".into(), Value::from(vec![0.8, 0.95]));
    computed.method.why = Some(if periods.is_empty() {
        "no seasonal period given: AutoETS on the series".into()
    } else {
        "MSTL removes the seasonal cycles; AutoETS (lowest AICc) forecasts the rest".into()
    });
    computed.method.alternatives = vec!["sarimax".into(), "prophet".into()];
    computed.assumptions = vec![
        Assumption {
            id: "regular_spacing".into(),
            check: None,
            stat: None,
            p: None,
            status: AssumptionStatus::NotChecked,
            consequence: None,
            note: Some("one row per time step, equally spaced, in order".into()),
        },
        Assumption {
            id: "stable_pattern".into(),
            check: None,
            stat: None,
            p: None,
            status: AssumptionStatus::NotChecked,
            consequence: None,
            note: Some("the intervals assume the trend and seasonality carry on as fitted".into()),
        },
    ];
    computed.warnings.push(warning(
        "extrapolation",
        if h > n / 2 || (longest > 0 && h > 2 * longest) {
            WarningSeverity::Warn
        } else {
            WarningSeverity::Info
        },
        format!(
            "the forecast extrapolates {h} steps beyond the {n} observed; it holds only while \
             the pattern does"
        ),
    ));
    if holdout == 0 {
        computed.warnings.push(warning(
            "no_holdout",
            WarningSeverity::Info,
            "no observations were held out, so the accuracy is not measured",
        ));
    }
    computed.narrative = Some(Narrative {
        template: "forecast".into(),
        text_slots: ["next", "end", "mape", "smape", "horizon"]
            .into_iter()
            .filter(|k| computed.values.contains_key(*k))
            .map(str::to_owned)
            .collect(),
    });
    Ok((computed, ComputedBody::Forecast(body)))
}

fn reproduce(
    computed: &mut Computed,
    request: &ForecastRequest,
    periods: &[usize],
    used: &[DataColumn],
) {
    let list: Vec<String> = periods.iter().map(ToString::to_string).collect();
    let r_series = if periods.is_empty() {
        format!("ts({})", r_col(&request.y))
    } else {
        format!(
            "forecast::msts({}, seasonal.periods = c({}))",
            r_col(&request.y),
            list.join(", ")
        )
    };
    let r_model = if periods.is_empty() {
        format!(
            "forecast::forecast(forecast::ets(y), h = {}, level = c(80, 95))",
            request.horizon
        )
    } else {
        format!(
            "forecast::stlf(y, h = {}, method = \"ets\", level = c(80, 95)); forecast::mstl(y)",
            request.horizon
        )
    };
    computed.reproduce.r = Some(format!(
        "{}; y <- {r_series}; {r_model}",
        r_frame(&request.data, used)
    ));
    computed.reproduce.python = Some(format!(
        "{}; from augurs import MSTL, AutoETS; m = MSTL.ets([{}]); m.fit({}.to_numpy()); \
         m.predict({}, level=0.95)",
        py_frame(&request.data, used),
        list.join(", "),
        py_col(&request.y),
        request.horizon
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accuracy_measures() {
        let (mape, smape, mae) = accuracy(&[100.0, 200.0], &[110.0, 180.0]);
        assert_eq!(mape, Some(10.0));
        assert!((mae - 15.0).abs() < 1e-12);
        let expected = 100.0 * (20.0 / 210.0 + 40.0 / 380.0) / 2.0;
        assert!((smape - expected).abs() < 1e-12);
        assert_eq!(accuracy(&[0.0, 1.0], &[0.0, 1.0]).0, None);
        assert_eq!(
            strength(&[1.0, -1.0, 1.0, -1.0], &[0.0, 0.0, 0.0, 0.0]),
            1.0
        );
    }
}
