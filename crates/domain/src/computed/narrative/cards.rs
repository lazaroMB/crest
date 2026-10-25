//! Sentences and APA lines of the phase-2 cards: distribution,
//! contingency, power, A/B test, bootstrap, forecast, optimisation,
//! equation solver, Monte Carlo, cash flow and decision matrix.
//!
//! As in the parent module, the words come from fixed templates and every
//! number from the envelope (`display` of a value) or the engine's body.

use super::{display, equals, raw, significance};
use crate::computed::apa::{self, ApaLine};
use crate::computed::{
    Alternative, BootStatistic, Computed, ComputedBody, ContingencyTest, PowerTarget,
};

/// The APA line of a phase-2 template, if it has one.
pub(super) fn apa_line(
    template: &str,
    computed: &Computed,
    body: &ComputedBody,
) -> Option<ApaLine> {
    let _ = body;
    match template {
        "distribution" => {
            let mut line = ApaLine::new().stat("M", &[], &display(computed, "mean")?);
            if let Some(sd) = display(computed, "sd") {
                line = line.stat("SD", &[], &sd);
            }
            if let Some(median) = display(computed, "median") {
                line = line.stat("Mdn", &[], &median);
            }
            Some(line)
        }
        "contingency" => match body {
            ComputedBody::Contingency(t) if t.test == ContingencyTest::ChiSquare => {
                let n = raw(computed, "n").unwrap_or_default();
                // A count is a whole non-negative number.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let mut line = apa::chi_square(
                    raw(computed, "statistic")?,
                    raw(computed, "df")?,
                    n.max(0.0).round() as u64,
                    raw(computed, "p_value")?,
                );
                if let Some(v) = display(computed, "cramers_v") {
                    line = line.part(equals("V", &v));
                }
                Some(line)
            }
            ComputedBody::Contingency(_) => {
                let mut line = ApaLine::new().p(raw(computed, "p_value")?);
                if let Some(or) = computed.value("odds_ratio") {
                    line = line.part(equals("OR", &or.display_text()));
                    if let Some(ci) = &or.ci {
                        line = line.ci(ci, false);
                    }
                } else if let Some(v) = display(computed, "cramers_v") {
                    line = line.part(equals("V", &v));
                }
                Some(line)
            }
            _ => None,
        },
        "bootstrap" => {
            let ComputedBody::Bootstrap(b) = body else {
                return None;
            };
            let symbol = match b.statistic {
                BootStatistic::Mean | BootStatistic::TrimmedMean => "M",
                BootStatistic::Median => "Mdn",
                BootStatistic::Sd => "SD",
                BootStatistic::MeanDifference => "ΔM",
                BootStatistic::MedianDifference => "ΔMdn",
                BootStatistic::Correlation => "r",
            };
            let estimate = computed.value("estimate")?;
            let mut line = ApaLine::new().stat(symbol, &[], &estimate.display_text());
            if let Some(ci) = &estimate.ci {
                line = line.ci(ci, b.statistic == BootStatistic::Correlation);
            }
            Some(line)
        }
        "ab_test" => {
            let mut line = ApaLine::new();
            if let Some(z) = display(computed, "statistic") {
                line = line.stat("z", &[], &z);
            }
            Some(line.p(raw(computed, "p_value")?))
        }
        _ => None,
    }
}

/// The plain-language sentence of a phase-2 template, if it has one.
pub(super) fn sentence(template: &str, computed: &Computed, body: &ComputedBody) -> Option<String> {
    match (template, body) {
        ("distribution", ComputedBody::Distribution(d)) => {
            let bins = format!(
                "{} bins of width {} ({})",
                display(computed, "bins")?,
                display(computed, "bin_width")?,
                d.bin_rule.label()
            );
            match &d.by {
                None => {
                    let outliers = match raw(computed, "outliers") {
                        Some(k) if k >= 1.0 => format!(
                            "; {} value{} beyond 1.5 × IQR",
                            display(computed, "outliers")?,
                            if (k - 1.0).abs() < f64::EPSILON {
                                " lies"
                            } else {
                                "s lie"
                            }
                        ),
                        _ => String::new(),
                    };
                    Some(format!(
                        "{} (n = {}) has median {} and IQR {}{outliers}. The histogram uses \
                         {bins}; the density curve a bandwidth of {} ({}).",
                        d.variable,
                        display(computed, "n")?,
                        display(computed, "median")?,
                        display(computed, "iqr")?,
                        display(computed, "bandwidth")?,
                        d.bandwidth_rule.label()
                    ))
                }
                Some(by) => Some(format!(
                    "{} by {by}: {} groups (n = {}); the histograms share {bins}.",
                    d.variable,
                    d.groups.len(),
                    display(computed, "n")?,
                )),
            }
        }
        ("contingency", ComputedBody::Contingency(t)) => {
            let what = match (&t.row_variable, &t.column_variable) {
                (Some(r), Some(c)) => format!(" of {r} by {c}"),
                _ => String::new(),
            };
            let effect = match computed.value("odds_ratio") {
                Some(or) if t.test != ContingencyTest::ChiSquare => {
                    format!(" (odds ratio {})", or.display_text())
                }
                _ => display(computed, "cramers_v")
                    .map(|v| format!(" (Cramér's V = {v})"))
                    .unwrap_or_default(),
            };
            let cell = computed
                .method
                .params
                .get("max_residual_cell")
                .and_then(serde_json::Value::as_str);
            let departs = match (cell, display(computed, "max_residual")) {
                (Some(cell), Some(res)) => format!(
                    " The cell that departs most from independence is {cell} (adjusted \
                     standardised residual {res})."
                ),
                _ => String::new(),
            };
            Some(format!(
                "{} on the {}×{} table{what} (N = {}): the association is {} at α = .05{effect}.{departs}",
                t.test.label(),
                t.row_levels.len(),
                t.column_levels.len(),
                display(computed, "n")?,
                significance(raw(computed, "p_value")?),
            ))
        }
        ("power", ComputedBody::Power(p)) => {
            let sym = p.test.effect_symbol();
            let side = match p.alternative {
                Alternative::TwoSided => "two-sided",
                Alternative::Less | Alternative::Greater => "one-sided",
            };
            let design = format!("a {side} {}", p.test.label());
            let per = if p.test.n_per_group() {
                " per group"
            } else {
                ""
            };
            let effect = display(computed, "effect")?;
            let alpha = display(computed, "alpha")?;
            let power = display(computed, "power")?;
            Some(match p.solved_for {
                PowerTarget::N => {
                    let total = display(computed, "n_total")
                        .map(|t| format!(" ({t} in total)"))
                        .unwrap_or_default();
                    format!(
                        "To detect {sym} = {effect} with power {power} at α = {alpha} in \
                         {design}, n = {}{per} is needed: recruit {}{per}{total}.",
                        display(computed, "n")?,
                        display(computed, "n_required")?,
                    )
                }
                PowerTarget::Power => format!(
                    "With n = {}{per} and {sym} = {effect}, {design} at α = {alpha} has power \
                     {power}.",
                    display(computed, "n")?,
                ),
                PowerTarget::Effect => format!(
                    "With n = {}{per}, power {power} and α = {alpha}, {design} detects effects \
                     from {sym} = {effect}.",
                    display(computed, "n")?,
                ),
                PowerTarget::Alpha => format!(
                    "For power {power} with n = {}{per} and {sym} = {effect}, {design} needs \
                     α = {alpha}.",
                    display(computed, "n")?,
                ),
            })
        }
        ("ab_test", ComputedBody::AbTest(t)) => {
            let [a, b] = [t.arms.first()?, t.arms.get(1)?];
            let lift = computed.value("abs_lift")?;
            let interval = lift
                .ci
                .as_ref()
                .map(|ci| {
                    format!(
                        " ({}% CI {} to {} pp)",
                        apa::number(ci.level * 100.0, 0),
                        apa::number(ci.lo * 100.0, 2),
                        apa::number(ci.hi * 100.0, 2)
                    )
                })
                .unwrap_or_default();
            let relative = display(computed, "rel_lift")
                .map(|r| format!(", {r} relative"))
                .unwrap_or_default();
            let srm = if t.sample_ratio_mismatch {
                "Sample-ratio mismatch: the arms did not get the planned share of visitors, so \
                 the comparison below is not reliable. "
            } else {
                ""
            };
            let bayes = display(computed, "prob_b_better")
                .map(|p| format!(" P({} > {}) = {p} with uniform priors.", b.name, a.name))
                .unwrap_or_default();
            Some(format!(
                "{srm}{} converts at {} against {} for {} ({} and {} visitors): a lift of {}{interval}{relative}, \
                 {} at α = {} ({}).{bayes}",
                b.name,
                display(computed, "treatment_rate")?,
                display(computed, "control_rate")?,
                a.name,
                b.visitors,
                a.visitors,
                lift.display_text(),
                significance(raw(computed, "p_value")?),
                apa::bounded(1.0 - t.conf_level, 2),
                t.method.label()
            ))
        }
        ("bootstrap", ComputedBody::Bootstrap(b)) => {
            let estimate = computed.value("estimate")?;
            let versus = b
                .versus
                .as_ref()
                .map(|v| match b.statistic {
                    BootStatistic::Correlation => format!(" and {v}"),
                    _ => format!(" ({v})"),
                })
                .unwrap_or_default();
            let interval = estimate
                .ci
                .as_ref()
                .map(|ci| {
                    format!(
                        "{}% {} interval {} to {}",
                        apa::number(ci.level * 100.0, 0),
                        ci.method.as_deref().unwrap_or("bootstrap"),
                        apa::number(ci.lo, estimate.decimals.unwrap_or(2)),
                        apa::number(ci.hi, estimate.decimals.unwrap_or(2))
                    )
                })
                .unwrap_or_else(|| "no interval".into());
            let mcse = match (display(computed, "lo_mcse"), display(computed, "hi_mcse")) {
                (Some(lo), Some(hi)) => {
                    format!(" Another seed moves the endpoints by about ±{lo} and ±{hi} (MCSE).")
                }
                _ => String::new(),
            };
            Some(format!(
                "The {} of {}{versus} is {} ({interval}; bootstrap SE {}, bias {}) from {} \
                 resamples with seed {}.{mcse}",
                b.statistic.label(),
                b.variable,
                estimate.display_text(),
                display(computed, "se")?,
                display(computed, "bias")?,
                display(computed, "resamples")?,
                computed
                    .seed
                    .map_or_else(|| "?".to_owned(), |s| s.to_string()),
            ))
        }
        ("forecast", ComputedBody::Forecast(f)) => {
            let horizon = display(computed, "horizon")?;
            let end = computed.value("end")?;
            let interval = end
                .ci
                .as_ref()
                .map(|ci| {
                    format!(
                        " (95% interval {} to {})",
                        apa::number(ci.lo, end.decimals.unwrap_or(2)),
                        apa::number(ci.hi, end.decimals.unwrap_or(2))
                    )
                })
                .unwrap_or_default();
            let accuracy = match (display(computed, "mape"), display(computed, "smape")) {
                (Some(mape), _) => format!(
                    " On the {} held-out observations the model, fitted without them, was off \
                     by MAPE {mape}%.",
                    display(computed, "holdout").unwrap_or_default()
                ),
                (None, Some(smape)) => format!(
                    " On the {} held-out observations the model, fitted without them, was off \
                     by sMAPE {smape}%.",
                    display(computed, "holdout").unwrap_or_default()
                ),
                _ => String::new(),
            };
            Some(format!(
                "{} ({}): the forecast {horizon} steps ahead is {}{interval}, starting at {}.{accuracy}",
                f.variable,
                f.model,
                end.display_text(),
                display(computed, "next")?,
            ))
        }
        ("optimization", ComputedBody::Optimization(o)) => {
            use crate::computed::{OptimizationStatus, Sense};
            let sense = match o.sense {
                Sense::Maximize => "maximum",
                Sense::Minimize => "minimum",
            };
            match o.status {
                OptimizationStatus::Infeasible => {
                    return Some(
                        "The problem is infeasible: no point meets every constraint.".into(),
                    );
                }
                OptimizationStatus::Unbounded => {
                    return Some(format!(
                        "The problem is unbounded: the objective has no {sense}."
                    ));
                }
                OptimizationStatus::Stopped => {
                    return Some("The solver stopped before finding a feasible point.".into());
                }
                OptimizationStatus::Optimal | OptimizationStatus::Feasible => {}
            }
            let values: Vec<String> = o
                .variables
                .iter()
                .filter_map(|v| {
                    let decimals = if v.integer { 0 } else { 4 };
                    v.value
                        .map(|x| format!("{} = {}", v.name, trim(&apa::number(x, decimals))))
                })
                .take(8)
                .collect();
            let more = o.variables.len().saturating_sub(8);
            let more = if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            };
            let binding: Vec<String> = o
                .constraints
                .iter()
                .filter(|k| k.binding)
                .take(3)
                .map(|k| match k.dual {
                    Some(d) if d.abs() > 1e-12 => format!(
                        "'{}' binds: one more unit of its right-hand side changes the objective \
                         by {}",
                        k.name,
                        trim(&apa::number(d, 4))
                    ),
                    _ => format!("'{}' binds at no marginal value", k.name),
                })
                .collect();
            let binding = if binding.is_empty() {
                " No constraint binds.".to_owned()
            } else {
                format!(" {}.", binding.join("; "))
            };
            let proven = if o.status == OptimizationStatus::Feasible {
                " (not proven optimal)"
            } else {
                ""
            };
            Some(format!(
                "The {sense} of {} is {}{proven} at {}{more}.{binding}",
                o.objective_expr,
                display(computed, "objective")
                    .map(|t| trim(&t))
                    .unwrap_or_default(),
                values.join(", ")
            ))
        }
        ("equations", ComputedBody::Equations(e)) => {
            use crate::computed::SolveMethod;
            if !e.converged {
                return Some(format!(
                    "No solution met the tolerance ({}): see the warning.",
                    apa::significant(e.tolerance, 2)
                ));
            }
            let residual = display(computed, "residual_norm").unwrap_or_default();
            Some(match e.method {
                SolveMethod::BracketScan => {
                    let name = e.unknowns.first().map_or("x", |u| u.name.as_str());
                    let roots: Vec<String> = (1..=e.roots.len())
                        .filter_map(|i| display(computed, &format!("root_{i}")))
                        .collect();
                    format!(
                        "{} has {} root{} in the range: {name} = {} (largest residual {residual}).",
                        e.plug_back
                            .first()
                            .map_or("The equation", |p| p.equation.as_str()),
                        roots.len(),
                        if roots.len() == 1 { "" } else { "s" },
                        roots.join(", ")
                    )
                }
                SolveMethod::Newton => {
                    let values: Vec<String> = e
                        .unknowns
                        .iter()
                        .map(|u| format!("{} = {}", u.name, apa::significant(u.value, 10)))
                        .collect();
                    let condition = display(computed, "condition_number")
                        .map(|c| format!("; Jacobian condition number {c}"))
                        .unwrap_or_default();
                    format!(
                        "Newton converged in {} iterations to {}; plugged back, the largest \
                         residual is {residual}{condition}.",
                        e.iterations,
                        values.join(", ")
                    )
                }
            })
        }
        ("simulation", ComputedBody::Simulation(m)) => {
            let unit = m.unit.as_ref().map(|u| format!(" {u}")).unwrap_or_default();
            let range = format!(
                "{}{unit} to {}{unit}",
                display(computed, "p5")?,
                display(computed, "p95")?
            );
            let driver = m
                .tornado
                .first()
                .map(|bar| {
                    format!(
                        " {} moves it most: from {} to {} between its 10th and 90th percentiles.",
                        bar.input,
                        apa::significant(bar.at_p10, 4),
                        apa::significant(bar.at_p90, 4)
                    )
                })
                .unwrap_or_default();
            let seed = computed
                .seed
                .map_or_else(|| "?".to_owned(), |s| s.to_string());
            Some(if m.fermi {
                let exact = match (
                    display(computed, "analytic_p5"),
                    display(computed, "analytic_p50"),
                    display(computed, "analytic_p95"),
                ) {
                    (Some(lo), Some(mid), Some(hi)) => format!(
                        " As a product of lognormal ranges it is lognormal: median {mid}, 90% \
                         range {lo} to {hi} exactly."
                    ),
                    _ => String::new(),
                };
                format!(
                    "The Fermi estimate of {} = {} is about {}{unit} (90% range {range}; {} \
                     draws, seed {seed}).{exact}{driver}",
                    m.output,
                    m.model,
                    display(computed, "p50")?,
                    m.samples
                )
            } else {
                format!(
                    "Over {} draws (seed {seed}), {} = {} has mean {}{unit} ± {} (MCSE), median \
                     {}{unit} and 90% of outcomes between {range}.{driver}",
                    m.samples,
                    m.output,
                    m.model,
                    display(computed, "mean")?,
                    display(computed, "mcse")?,
                    display(computed, "p50")?,
                )
            })
        }
        _ => None,
    }
}

/// `3.5000` as `3.5`, `2.0000` as `2`: trailing zeros of a decimal number
/// dropped (minus sign kept).
fn trim(text: &str) -> String {
    if !text.contains('.') {
        return text.to_owned();
    }
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computed::{
        BandwidthRule, BinRule, BoxSummary, ComputedValue, Curve, DistributionBody,
        DistributionGroup, Engine, Histogram, Narrative,
    };

    fn shown(v: f64, display: &str) -> ComputedValue {
        ComputedValue {
            display: Some(display.into()),
            ..ComputedValue::new(v)
        }
    }

    fn computed(template: &str, values: &[(&str, f64, &str)]) -> Computed {
        let engine = Engine {
            name: "crest-stats".into(),
            version: "0".into(),
            backend: None,
            platform: None,
        };
        let mut c = Computed::new("st_1", "stats.x", "X", engine);
        for (key, v, text) in values {
            c.values.insert((*key).into(), shown(*v, text));
        }
        c.narrative = Some(Narrative {
            template: template.into(),
            text_slots: Vec::new(),
        });
        c
    }

    #[test]
    fn a_sample_size_says_how_many_to_recruit() {
        let c = computed(
            "power",
            &[
                ("n", 25.52, "25.52"),
                ("n_required", 26.0, "26"),
                ("n_total", 52.0, "52"),
                ("effect", 0.8, "0.800"),
                ("alpha", 0.05, ".050"),
                ("power", 0.8, ".800"),
            ],
        );
        let body = ComputedBody::Power(crate::computed::PowerBody {
            test: crate::computed::PowerTest::TwoSampleT,
            solved_for: PowerTarget::N,
            alternative: Alternative::TwoSided,
            n: 25.52,
            effect: 0.8,
            alpha: 0.05,
            power: 0.8,
            groups: None,
            curve: Curve::default(),
        });
        assert_eq!(
            sentence("power", &c, &body).unwrap(),
            "To detect d = 0.800 with power .800 at α = .050 in a two-sided two-sample t-test, \
             n = 25.52 per group is needed: recruit 26 per group (52 in total)."
        );
        assert_eq!(apa_line("power", &c, &body), None);
    }

    #[test]
    fn a_distribution_states_its_bins_and_bandwidth() {
        let c = computed(
            "distribution",
            &[
                ("n", 32.0, "32"),
                ("mean", 20.09, "20.09"),
                ("sd", 6.03, "6.03"),
                ("median", 19.2, "19.20"),
                ("iqr", 7.38, "7.38"),
                ("outliers", 1.0, "1"),
                ("bins", 5.0, "5"),
                ("bin_width", 4.7, "4.70"),
                ("bandwidth", 2.48, "2.48"),
            ],
        );
        let group = DistributionGroup {
            name: None,
            n: 32,
            histogram: Histogram {
                edges: vec![0.0, 1.0],
                counts: vec![32],
            },
            kde: Curve::default(),
            bandwidth: 2.48,
            boxplot: BoxSummary {
                lower_hinge: 15.35,
                median: 19.2,
                upper_hinge: 22.8,
                whisker_lo: 10.4,
                whisker_hi: 30.4,
                outliers: vec![33.9],
            },
            qq: Vec::new(),
            ecdf: Vec::new(),
            dkw_epsilon: 0.24,
        };
        let body = ComputedBody::Distribution(DistributionBody {
            variable: "mpg".into(),
            unit: None,
            by: None,
            bin_rule: BinRule::FreedmanDiaconis,
            bin_width: 4.7,
            bandwidth_rule: BandwidthRule::Silverman,
            conf_level: 0.95,
            groups: vec![group],
        });
        assert_eq!(
            sentence("distribution", &c, &body).unwrap(),
            "mpg (n = 32) has median 19.20 and IQR 7.38; 1 value lies beyond 1.5 × IQR. The \
             histogram uses 5 bins of width 4.70 (Freedman-Diaconis); the density curve a \
             bandwidth of 2.48 (Silverman)."
        );
        assert_eq!(
            apa_line("distribution", &c, &body).unwrap().plain(),
            "M = 20.09, SD = 6.03, Mdn = 19.20"
        );
    }
}
