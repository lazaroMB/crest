//! Sentences and APA lines of the phase-2 cards: distribution,
//! contingency, power, A/B test, bootstrap, forecast, optimisation,
//! equation solver, Monte Carlo, cash flow and decision matrix.
//!
//! As in the parent module, the words come from fixed templates and every
//! number from the envelope (`display` of a value) or the engine's body.

use super::{display, equals, raw, significance};
use crate::computed::apa::{self, ApaLine};
use crate::computed::{Alternative, Computed, ComputedBody, ContingencyTest, PowerTarget};

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
        _ => None,
    }
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
