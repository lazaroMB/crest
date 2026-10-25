//! Sentences and APA lines of the phase-2 cards: distribution,
//! contingency, power, A/B test, bootstrap, forecast, optimisation,
//! equation solver, Monte Carlo, cash flow and decision matrix.
//!
//! As in the parent module, the words come from fixed templates and every
//! number from the envelope (`display` of a value) or the engine's body.

use super::{display, raw};
use crate::computed::apa::ApaLine;
use crate::computed::{Computed, ComputedBody};

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
