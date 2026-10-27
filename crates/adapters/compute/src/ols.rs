//! Ordinary least squares with an intercept.
//!
//! The predictors are centered and scaled to unit length before a
//! Householder QR, so the slopes come from a well-conditioned problem and
//! the intercept from the means; this keeps the certified digits of NIST's
//! ill-conditioned Longley data. Categorical predictors are dummy coded
//! against their first sorted level (`x[T.level]`, as patsy names them).
//!
//! The footer follows statsmodels: AIC/BIC from the Gaussian
//! log-likelihood without a σ parameter, Jarque-Bera on the residuals,
//! Breusch-Pagan in Koenker's studentized form, and the condition number
//! of the raw design (constant included). The collinearity warning uses
//! the scaled condition number (unit-length columns, Belsley's rule of 30).

use application::ports::compute::{ColumnValues, ComputeError, RegressRequest};
use domain::computed::{
    Assumption, AssumptionStatus, CoefficientRow, Computed, ComputedBody, Diagnostics, FitStats,
    Narrative, RegressionBody, UncertaintyKind, WarningSeverity, apa,
};
use serde_json::Value;

use crate::data::{self, key_part};
use crate::envelope::{
    self, auto_decimals, bounded, count, df, number, p_value, significant, warning, with_ci,
};
use crate::linalg::{Qr, singular_values};
use crate::math::{p_from_t, pchisq_upper, pf_upper, qt};
use crate::reproduce::{py_frame, r_frame, r_name};

/// A design column: its term name, values on the complete rows, and the
/// predictor it came from.
struct Term {
    name: String,
    values: Vec<f64>,
}

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// Runs a regression request.
#[allow(clippy::too_many_lines)]
pub fn run(request: &RegressRequest, id: String) -> Result<(Computed, ComputedBody), ComputeError> {
    let data = &request.data;
    if request.x.is_empty() {
        return Err(invalid("`x` needs at least one predictor"));
    }
    if !(request.conf_level > 0.0 && request.conf_level < 1.0) {
        return Err(invalid("`conf_level` must be between 0 and 1"));
    }
    let y_all = data::numeric(data, &request.y)?;
    // Complete rows across y and every predictor.
    let mut complete: Vec<bool> = y_all.iter().map(Option::is_some).collect();
    enum Predictor {
        Numeric(Vec<Option<f64>>),
        Categorical(Vec<Option<String>>),
    }
    let mut predictors = Vec::new();
    for name in &request.x {
        let column = data::column(data, name)?;
        let predictor = match &column.values {
            ColumnValues::Numeric(_) => Predictor::Numeric(data::numeric(data, name)?),
            ColumnValues::Categorical(_) => {
                let (labels, _) = data::labels(data, name)?;
                Predictor::Categorical(labels)
            }
        };
        let present: Vec<bool> = match &predictor {
            Predictor::Numeric(v) => v.iter().map(Option::is_some).collect(),
            Predictor::Categorical(v) => v.iter().map(Option::is_some).collect(),
        };
        for (c, p) in complete.iter_mut().zip(present) {
            *c &= p;
        }
        predictors.push((name, predictor));
    }
    let pick = |v: &[Option<f64>]| -> Vec<f64> {
        v.iter()
            .zip(&complete)
            .filter(|(_, c)| **c)
            .filter_map(|(v, _)| *v)
            .collect()
    };
    let y = pick(&y_all);
    let n = y.len();
    let mut terms = Vec::new();
    for (name, predictor) in &predictors {
        match predictor {
            Predictor::Numeric(v) => terms.push(Term {
                name: (*name).clone(),
                values: pick(v),
            }),
            Predictor::Categorical(labels) => {
                let rows: Vec<&str> = labels
                    .iter()
                    .zip(&complete)
                    .filter(|(_, c)| **c)
                    .filter_map(|(l, _)| l.as_deref())
                    .collect();
                let mut levels: Vec<&str> = rows.clone();
                levels.sort_unstable();
                levels.dedup();
                for level in levels.iter().skip(1) {
                    terms.push(Term {
                        name: format!("{name}[T.{level}]"),
                        values: rows
                            .iter()
                            .map(|r| f64::from(u8::from(r == level)))
                            .collect(),
                    });
                }
            }
        }
    }

    let mut computed = Computed::new(
        id,
        "stats.ols",
        "Ordinary least squares",
        envelope::engine(),
    );
    computed.uncertainty_kind = Some(UncertaintyKind::FrequentistCi);
    let params = &mut computed.method.params;
    params.insert("conf_level".into(), Value::from(request.conf_level));
    params.insert(
        "solver".into(),
        Value::from("Householder QR on centered, scaled predictors"),
    );
    params.insert("breusch_pagan".into(), Value::from("Koenker (studentized)"));
    params.insert(
        "information_criteria".into(),
        Value::from("statsmodels (no σ parameter)"),
    );
    computed.method.alternatives = vec!["robust_se".into(), "spearman".into()];
    let mut used: Vec<&str> = vec![request.y.as_str()];
    used.extend(request.x.iter().map(String::as_str));
    let used = data::used_columns(data, &used);
    computed.inputs.push(envelope::input(data, &used, None));
    reproduce(&mut computed, request, &used);
    let dropped = y_all.len() - n;
    if dropped > 0 {
        computed.warnings.push(warning(
            "missing_dropped",
            WarningSeverity::Info,
            format!("{dropped} rows with missing values were left out"),
        ));
    }
    let empty_body = |n: usize| {
        ComputedBody::Regression(RegressionBody {
            dependent: request.y.clone(),
            ci_level: request.conf_level,
            coefficients: Vec::new(),
            fit: FitStats {
                n: n as u64,
                r_squared: 0.0,
                adj_r_squared: 0.0,
                f: 0.0,
                df_model: 0.0,
                df_resid: 0.0,
                f_p: 1.0,
                sigma: None,
                aic: None,
                bic: None,
            },
            diagnostics: Diagnostics::default(),
            fitted: Vec::new(),
            residuals: Vec::new(),
        })
    };
    let k = terms.len();
    if k == 0 {
        envelope::fail(
            &mut computed,
            "the categorical predictors have a single level each, so there is nothing to fit",
        );
        return Ok((computed, empty_body(n)));
    }
    if n <= k + 1 {
        envelope::fail(
            &mut computed,
            format!(
                "{n} complete rows for {} coefficients: needs more observations than coefficients",
                k + 1
            ),
        );
        return Ok((computed, empty_body(n)));
    }
    let nf = n as f64;
    let center = |v: &[f64]| -> (f64, Vec<f64>) {
        let m = crate::describe::mean(v);
        (m, v.iter().map(|x| x - m).collect())
    };
    let (y_mean, yc) = center(&y);
    let sst: f64 = yc.iter().map(|v| v * v).sum();
    if sst == 0.0 {
        envelope::fail(
            &mut computed,
            format!("{} is constant, so there is nothing to explain", request.y),
        );
        return Ok((computed, empty_body(n)));
    }
    let mut means = Vec::with_capacity(k);
    let mut scales = Vec::with_capacity(k);
    let mut z = Vec::with_capacity(k);
    for term in &terms {
        let (m, c) = center(&term.values);
        let s = c.iter().map(|v| v * v).sum::<f64>().sqrt();
        if s == 0.0 {
            envelope::fail(
                &mut computed,
                format!(
                    "predictor {} is constant (collinear with the intercept)",
                    term.name
                ),
            );
            return Ok((computed, empty_body(n)));
        }
        means.push(m);
        scales.push(s);
        z.push(c.iter().map(|v| v / s).collect::<Vec<f64>>());
    }
    let qr = Qr::new(&z);
    let rdiag = qr.r_diag();
    if let Some(j) = rdiag.iter().position(|r| r.abs() < 1e-10) {
        envelope::fail(
            &mut computed,
            format!(
                "predictors are perfectly collinear: {} is a linear combination of the others",
                terms[j].name
            ),
        );
        return Ok((computed, empty_body(n)));
    }
    let bz = qr.solve(&yc);
    let slopes: Vec<f64> = bz.iter().zip(&scales).map(|(b, s)| b / s).collect();
    let intercept = y_mean - slopes.iter().zip(&means).map(|(b, m)| b * m).sum::<f64>();
    let residuals: Vec<f64> = (0..n)
        .map(|i| yc[i] - (0..k).map(|j| z[j][i] * bz[j]).sum::<f64>())
        .collect();
    let fitted: Vec<f64> = y.iter().zip(&residuals).map(|(y, e)| y - e).collect();
    let ssr: f64 = residuals.iter().map(|e| e * e).sum();
    let df_model = k as f64;
    let df_resid = nf - df_model - 1.0;
    let sigma2 = ssr / df_resid;
    let sigma = sigma2.sqrt();
    let inv = qr.inverse_gram();
    // Covariance of the slopes in the original units.
    let cov = |i: usize, j: usize| sigma2 * inv[i][j] / (scales[i] * scales[j]);
    let var_intercept = sigma2 / nf
        + (0..k)
            .map(|i| (0..k).map(|j| means[i] * cov(i, j) * means[j]).sum::<f64>())
            .sum::<f64>();
    let level = request.conf_level;
    let q = qt(1.0 - (1.0 - level) / 2.0, df_resid);
    let sd_y = (sst / (nf - 1.0)).sqrt();
    let mut coefficients = Vec::with_capacity(k + 1);
    let row = |term: &str, estimate: f64, se: f64, std_beta: Option<f64>, vif: Option<f64>| {
        let t = estimate / se;
        CoefficientRow {
            term: term.to_owned(),
            estimate,
            se,
            t,
            p: p_from_t(t, df_resid, domain::computed::Alternative::TwoSided),
            ci_lo: estimate - q * se,
            ci_hi: estimate + q * se,
            std_beta,
            vif,
        }
    };
    coefficients.push(row(
        "(Intercept)",
        intercept,
        var_intercept.sqrt(),
        None,
        None,
    ));
    let mut vifs = Vec::with_capacity(k);
    for j in 0..k {
        let sd_x = scales[j] / (nf - 1.0).sqrt();
        let vif = inv[j][j];
        vifs.push(vif);
        coefficients.push(row(
            &terms[j].name,
            slopes[j],
            cov(j, j).sqrt(),
            Some(slopes[j] * sd_x / sd_y),
            (k > 1).then_some(vif),
        ));
    }
    let r2 = 1.0 - ssr / sst;
    let adj = 1.0 - (1.0 - r2) * (nf - 1.0) / df_resid;
    let f = ((sst - ssr) / df_model) / sigma2;
    let f_p = pf_upper(f, df_model, df_resid);
    let llf = -nf / 2.0 * ((2.0 * std::f64::consts::PI).ln() + (ssr / nf).ln() + 1.0);
    let params_n = df_model + 1.0;
    let aic = -2.0 * llf + 2.0 * params_n;
    let bic = -2.0 * llf + params_n * nf.ln();

    // Diagnostics.
    let dw = residuals
        .windows(2)
        .map(|w| (w[1] - w[0]).powi(2))
        .sum::<f64>()
        / ssr;
    let (jb, jb_p) = jarque_bera(&residuals);
    let (bp, bp_p) = breusch_pagan(&qr, &residuals, df_model);
    let mut raw = vec![vec![1.0; n]];
    raw.extend(terms.iter().map(|t| t.values.clone()));
    let condition = ratio(&singular_values(&raw));
    let scaled: Vec<Vec<f64>> = raw
        .iter()
        .map(|c| {
            let norm = c.iter().map(|v| v * v).sum::<f64>().sqrt();
            c.iter().map(|v| v / norm).collect()
        })
        .collect();
    let scaled_condition = ratio(&singular_values(&scaled));
    let leverage = qr.leverages();
    let cooks: Vec<f64> = residuals
        .iter()
        .zip(&leverage)
        .map(|(e, hz)| {
            let h = 1.0 / nf + hz;
            e * e / (params_n * sigma2) * h / ((1.0 - h) * (1.0 - h))
        })
        .collect();
    let max_cooks = cooks
        .iter()
        .copied()
        .filter(|c| c.is_finite())
        .fold(0.0, f64::max);
    let max_vif = (k > 1).then(|| vifs.iter().copied().fold(0.0, f64::max));

    // Values.
    let values = &mut computed.values;
    values.insert("n".into(), count(n as u64));
    values.insert("r_squared".into(), bounded(r2, 2));
    values.insert("adj_r_squared".into(), bounded(adj, 2));
    values.insert("f".into(), number(f, 2));
    values.insert("statistic".into(), number(f, 2));
    values.insert("df_model".into(), df(df_model));
    values.insert("df_resid".into(), df(df_resid));
    values.insert("f_p".into(), p_value(f_p));
    values.insert("p_value".into(), p_value(f_p));
    values.insert("sigma".into(), number(sigma, auto_decimals(sigma)));
    values.insert("aic".into(), number(aic, 2));
    values.insert("bic".into(), number(bic, 2));
    values.insert("durbin_watson".into(), number(dw, 2));
    values.insert("condition_number".into(), significant(condition, 3));
    let unit_y = data.column(&request.y).and_then(|c| c.unit.clone());
    for c in &coefficients {
        let key = if c.term == "(Intercept)" {
            "intercept".to_owned()
        } else {
            key_part(&c.term)
        };
        let decimals = auto_decimals(c.se);
        let mut estimate = with_ci(
            number(c.estimate, decimals),
            level,
            c.ci_lo,
            c.ci_hi,
            "student_t",
        );
        if c.term == "(Intercept)" {
            estimate.unit = unit_y.clone();
        }
        values.insert(format!("coef.{key}"), estimate);
        values.insert(format!("se.{key}"), number(c.se, decimals));
        values.insert(format!("t.{key}"), number(c.t, 2));
        values.insert(format!("p.{key}"), p_value(c.p));
        if let Some(beta) = c.std_beta {
            values.insert(format!("beta.{key}"), number(beta, 2));
        }
        if let Some(vif) = c.vif {
            values.insert(format!("vif.{key}"), number(vif, 2));
        }
    }

    // Assumptions.
    let check =
        |id: &str, name: &str, stat: f64, p: f64, note: &str, consequence: &str| Assumption {
            id: id.into(),
            check: Some(name.into()),
            stat: Some(stat),
            p: Some(p),
            status: if p < 0.05 {
                AssumptionStatus::Fail
            } else {
                AssumptionStatus::Pass
            },
            consequence: (p < 0.05).then(|| consequence.into()),
            note: Some(note.into()),
        };
    computed.assumptions = vec![
        Assumption {
            id: "linearity".into(),
            check: None,
            stat: None,
            p: None,
            status: AssumptionStatus::NotChecked,
            consequence: None,
            note: Some("look at residuals vs fitted".into()),
        },
        check(
            "normality_residuals",
            "Jarque-Bera",
            jb,
            jb_p,
            "normal residuals",
            "intervals and p-values may be off in small samples",
        ),
        check(
            "homoscedasticity",
            "Breusch-Pagan",
            bp,
            bp_p,
            "constant residual variance",
            "standard errors may be wrong; consider robust (HC3) errors",
        ),
        Assumption {
            id: "independence".into(),
            check: Some("Durbin-Watson".into()),
            stat: Some(dw),
            p: None,
            status: if (1.5..=2.5).contains(&dw) {
                AssumptionStatus::Pass
            } else {
                AssumptionStatus::Warn
            },
            consequence: (!(1.5..=2.5).contains(&dw))
                .then(|| "residuals look autocorrelated in row order".into()),
            note: Some("independent residuals (in row order); also a design property".into()),
        },
    ];
    if let Some(max_vif) = max_vif {
        computed.assumptions.push(Assumption {
            id: "no_multicollinearity".into(),
            check: Some("max VIF".into()),
            stat: Some(max_vif),
            p: None,
            status: if max_vif > 10.0 {
                AssumptionStatus::Fail
            } else if max_vif > 5.0 {
                AssumptionStatus::Warn
            } else {
                AssumptionStatus::Pass
            },
            consequence: (max_vif > 10.0)
                .then(|| "coefficients are unstable; drop or combine predictors".into()),
            note: None,
        });
    }

    // Misuse warnings.
    let high: Vec<String> = coefficients
        .iter()
        .filter(|c| c.vif.is_some_and(|v| v > 10.0))
        .map(|c| format!("{} ({})", c.term, apa::number(c.vif.unwrap_or_default(), 1)))
        .collect();
    if !high.is_empty() {
        computed.warnings.push(warning(
            "vif_gt_10",
            WarningSeverity::Warn,
            format!("VIF > 10, strong multicollinearity: {}", high.join(", ")),
        ));
    }
    if scaled_condition > 30.0 {
        computed.warnings.push(warning(
            "large_condition_number",
            WarningSeverity::Warn,
            format!(
                "the scaled condition number is {} (> 30): the predictors are nearly collinear",
                apa::significant(scaled_condition, 3)
            ),
        ));
    }
    if n < 10 * k {
        computed.warnings.push(warning(
            "small_n",
            WarningSeverity::Info,
            format!("{n} observations for {k} predictors: fewer than 10 per predictor"),
        ));
    }
    if max_cooks > 1.0 {
        computed.warnings.push(warning(
            "influential_points",
            WarningSeverity::Info,
            format!(
                "a point has Cook's distance {} > 1",
                apa::number(max_cooks, 2)
            ),
        ));
    }
    computed.narrative = Some(Narrative {
        template: "ols".into(),
        text_slots: ["r_squared", "f", "df_model", "df_resid", "p_value"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
    });
    let body = ComputedBody::Regression(RegressionBody {
        dependent: request.y.clone(),
        ci_level: level,
        coefficients,
        fit: FitStats {
            n: n as u64,
            r_squared: r2,
            adj_r_squared: adj,
            f,
            df_model,
            df_resid,
            f_p,
            sigma: Some(sigma),
            aic: Some(aic),
            bic: Some(bic),
        },
        diagnostics: Diagnostics {
            durbin_watson: Some(dw),
            jarque_bera: Some(jb),
            jarque_bera_p: Some(jb_p),
            breusch_pagan: Some(bp),
            breusch_pagan_p: Some(bp_p),
            condition_number: Some(condition),
            max_vif,
            max_cooks_d: Some(max_cooks),
        },
        fitted,
        residuals,
    });
    Ok((computed, body))
}

fn ratio(singular: &[f64]) -> f64 {
    match (singular.first(), singular.last()) {
        (Some(max), Some(min)) if *min > 0.0 => max / min,
        _ => f64::INFINITY,
    }
}

/// Jarque-Bera statistic of the residuals and its χ²(2) p-value.
fn jarque_bera(e: &[f64]) -> (f64, f64) {
    let n = e.len() as f64;
    let m = e.iter().sum::<f64>() / n;
    let moment = |k: i32| e.iter().map(|x| (x - m).powi(k)).sum::<f64>() / n;
    let m2 = moment(2);
    let skew = moment(3) / m2.powf(1.5);
    let kurt = moment(4) / (m2 * m2);
    let jb = n / 6.0 * (skew * skew + (kurt - 3.0).powi(2) / 4.0);
    (jb, pchisq_upper(jb, 2.0))
}

/// Koenker's Breusch-Pagan LM = n·R² of e² on the predictors, χ²(k).
fn breusch_pagan(qr: &Qr, e: &[f64], k: f64) -> (f64, f64) {
    let n = e.len() as f64;
    let u: Vec<f64> = e.iter().map(|x| x * x).collect();
    let mean = u.iter().sum::<f64>() / n;
    let uc: Vec<f64> = u.iter().map(|x| x - mean).collect();
    let sst: f64 = uc.iter().map(|x| x * x).sum();
    if sst == 0.0 {
        return (0.0, 1.0);
    }
    // The explained sum of squares is the squared norm of the first k
    // entries of Qᵀ u.
    let qtu = qr.qt_mul(&uc);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let explained: f64 = qtu.iter().take(k as usize).map(|x| x * x).sum();
    let lm = n * explained / sst;
    (lm, pchisq_upper(lm, k))
}

fn py_term(name: &str) -> String {
    let identifier = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if identifier {
        name.to_owned()
    } else {
        format!("Q(\"{}\")", name.replace('"', "\\\""))
    }
}

fn reproduce(
    computed: &mut Computed,
    request: &RegressRequest,
    used: &[application::ports::compute::DataColumn],
) {
    let data = &request.data;
    let categorical = |name: &str| data.column(name).is_some_and(|c| !data::is_numeric(c));
    let py_x: Vec<String> = request
        .x
        .iter()
        .map(|x| {
            if categorical(x) {
                format!("C({})", py_term(x))
            } else {
                py_term(x)
            }
        })
        .collect();
    let formula = format!("{} ~ {}", py_term(&request.y), py_x.join(" + "));
    computed.reproduce.python = Some(format!(
        "import statsmodels.formula.api as smf; {}; smf.ols({}, data=df).fit().summary()",
        py_frame(data, used),
        crate::reproduce::py_str(&formula)
    ));
    let r_x: Vec<String> = request.x.iter().map(|x| r_name(x)).collect();
    computed.reproduce.r = Some(format!(
        "{}; fit <- lm({} ~ {}, data = df); summary(fit); confint(fit, level = {})",
        r_frame(data, used),
        r_name(&request.y),
        r_x.join(" + "),
        request.conf_level
    ));
    let request_json = serde_json::to_value(application::ports::compute::ComputeRequest::Regress(
        request.clone(),
    ))
    .unwrap_or_default();
    computed.reproduce.lua = Some(crate::reproduce::lua("regress", &request_json, Some(data)));
}
