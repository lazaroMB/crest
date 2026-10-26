//! The plain-language sentence and the APA line of an envelope.
//!
//! The engine names a template in `narrative.template` (`apa_ttest`,
//! `ols`...) and the value keys it uses; [`sentence`] and [`apa_line`]
//! fill them from the envelope's values, so the words around the numbers
//! come from a fixed template and every number from the engine, never
//! from the model. The card shows both; the statistics tools also hand the
//! APA line to the model, which cites its numbers through slots.
//!
//! Significance in the sentences is judged at α = .05 and said as such.

use super::apa::{self, ApaLine, ApaToken};
use super::{Computed, ComputedBody, ComputedStatus, PAdjust};

/// The level the sentences call significant.
const ALPHA: f64 = 0.05;

fn display(computed: &Computed, key: &str) -> Option<String> {
    computed.value(key).map(super::ComputedValue::display_text)
}

fn raw(computed: &Computed, key: &str) -> Option<f64> {
    computed.value(key).map(|v| v.v)
}

/// "statistically significant" or not, at [`ALPHA`].
fn significance(p: f64) -> &'static str {
    if p < ALPHA {
        "statistically significant"
    } else {
        "not statistically significant"
    }
}

/// `sym = display` as one APA part.
fn equals(sym: &str, value: &str) -> Vec<ApaToken> {
    vec![
        ApaToken::Italic(sym.to_owned()),
        ApaToken::Plain(format!(" = {value}")),
    ]
}

/// The template of `computed`, unless it failed.
fn template(computed: &Computed) -> Option<&str> {
    if computed.status == ComputedStatus::Error {
        return None;
    }
    computed.narrative.as_ref().map(|n| n.template.as_str())
}

/// The effect size symbol of a test template.
fn effect_symbol(template: &str) -> &'static str {
    match template {
        "apa_ttest" => "d",
        "apa_mann_whitney" | "apa_wilcoxon" => "r",
        "apa_chisq" => "V",
        _ => "OR",
    }
}

/// The correlation symbol stored by the engine, `r` by default.
fn correlation_symbol(computed: &Computed) -> String {
    computed
        .method
        .params
        .get("symbol")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("r")
        .to_owned()
}

/// The APA results line of `computed`, e.g. `t(18.33) = −3.77, p = .001,
/// d = −1.48, 95% CI [−2.27, −0.67]`; `None` when the template has none
/// (a correlation matrix) or the computation failed.
#[must_use]
pub fn apa_line(computed: &Computed, body: &ComputedBody) -> Option<ApaLine> {
    let template = template(computed)?;
    let p = raw(computed, "p_value");
    let line = match template {
        "apa_ttest" | "apa_mann_whitney" | "apa_wilcoxon" => {
            let symbol = match body {
                ComputedBody::Test(test) => test.test.statistic_symbol(),
                _ => "t",
            };
            let df: Vec<f64> = raw(computed, "df").into_iter().collect();
            let mut line = ApaLine::new().stat(symbol, &df, &display(computed, "statistic")?);
            if let Some(p) = p {
                line = line.p(p);
            }
            if let Some(effect) = computed.value("effect") {
                line = line.part(equals(effect_symbol(template), &effect.display_text()));
                if let Some(ci) = &effect.ci {
                    line = line.ci(ci, template != "apa_ttest");
                }
            }
            line
        }
        "apa_chisq" => {
            let n = raw(computed, "n").unwrap_or_default();
            // A count is a whole non-negative number.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let mut line = apa::chi_square(
                raw(computed, "statistic")?,
                raw(computed, "df")?,
                n.max(0.0).round() as u64,
                p?,
            );
            if let Some(effect) = display(computed, "effect") {
                line = line.part(equals("V", &effect));
            }
            line
        }
        "fisher_exact" => {
            let mut line = ApaLine::new().p(p?);
            if let Some(effect) = computed.value("effect") {
                line = line.part(equals("OR", &effect.display_text()));
                if let Some(ci) = &effect.ci {
                    line = line.ci(ci, false);
                }
            }
            line
        }
        "ols" => {
            let mut line = ApaLine::new().part(vec![
                ApaToken::Italic("R".into()),
                ApaToken::Plain(format!("² = {}", display(computed, "r_squared")?)),
            ]);
            if let (Some(f), Some(d1), Some(d2)) = (
                display(computed, "f"),
                raw(computed, "df_model"),
                raw(computed, "df_resid"),
            ) {
                line = line.stat("F", &[d1, d2], &f);
            }
            if let Some(p) = p {
                line = line.p(p);
            }
            line
        }
        "apa_correlation" => {
            let sym = correlation_symbol(computed);
            let df: Vec<f64> = raw(computed, "df").into_iter().collect();
            let mut line = ApaLine::new().stat(&sym, &df, &display(computed, "r")?);
            if let Some(p) = p {
                line = line.p(p);
            }
            if let Some(ci) = computed.value("r").and_then(|r| r.ci.as_ref()) {
                line = line.ci(ci, true);
            }
            line
        }
        "describe" => {
            let slots = &computed.narrative.as_ref()?.text_slots;
            let mean = slots.iter().find(|k| k.ends_with(".mean"))?;
            let sd = slots.iter().find(|k| k.ends_with(".sd"));
            let mut line = ApaLine::new().stat("M", &[], &display(computed, mean)?);
            if let Some(sd) = sd.and_then(|k| display(computed, k)) {
                line = line.stat("SD", &[], &sd);
            }
            line
        }
        _ => return None,
    };
    Some(line)
}

/// The plain-language sentence of `computed`, with the engine's numbers
/// filled in; `None` when the computation failed or the template is
/// unknown.
#[must_use]
pub fn sentence(computed: &Computed, body: &ComputedBody) -> Option<String> {
    let template = template(computed)?;
    let p = raw(computed, "p_value");
    let effect = || {
        computed.value("effect").map(|e| {
            let label = e
                .label
                .as_deref()
                .map(|l| format!("a {l} effect, "))
                .unwrap_or_default();
            format!(
                " ({label}{} = {})",
                effect_symbol(template),
                e.display_text()
            )
        })
    };
    let text = match (template, body) {
        ("apa_ttest" | "apa_mann_whitney" | "apa_wilcoxon", ComputedBody::Test(test)) => {
            let center = if template == "apa_ttest" { "M" } else { "Mdn" };
            let groups: Vec<String> = test
                .groups
                .iter()
                .filter_map(|g| {
                    let value = if template == "apa_ttest" {
                        g.mean
                    } else {
                        g.median.or(g.mean)
                    }?;
                    Some(format!(
                        "{} ({center} = {}, n = {})",
                        g.name,
                        apa::number(value, 2),
                        g.n
                    ))
                })
                .collect();
            let compared = match groups.as_slice() {
                [] => String::new(),
                [one] => format!("for {one} "),
                many => format!("between {} ", many.join(" and ")),
            };
            format!(
                "{}: the difference {compared}is {} at α = .05{}.",
                computed.method.name,
                significance(p?),
                effect().unwrap_or_default()
            )
        }
        ("apa_chisq", _) => format!(
            "The association between the two variables (N = {}) is {} at α = .05{}.",
            display(computed, "n")?,
            significance(p?),
            effect().unwrap_or_default()
        ),
        ("fisher_exact", _) => {
            let odds = computed
                .value("effect")
                .map(|e| format!("The odds ratio is {}; the", e.display_text()))
                .unwrap_or_else(|| "The".into());
            format!(
                "{odds} association is {} at α = .05 (Fisher's exact test).",
                significance(p?)
            )
        }
        ("ols", ComputedBody::Regression(reg)) => format!(
            "The model explains R² = {} of the variance of {} (adjusted R² = {}, n = {}); \
             the overall F test is {} at α = .05.",
            display(computed, "r_squared")?,
            reg.dependent,
            display(computed, "adj_r_squared").unwrap_or_else(|| "?".into()),
            reg.fit.n,
            significance(p?)
        ),
        ("apa_correlation", ComputedBody::Correlation(cor)) => {
            let label = computed
                .value("r")
                .and_then(|r| r.label.as_deref())
                .map(|l| format!(", a {l} correlation"))
                .unwrap_or_default();
            format!(
                "{} between {} is {}{label}, {} at α = .05 (n = {}).",
                correlation_symbol(computed),
                cor.variables.join(" and "),
                display(computed, "r")?,
                significance(p?),
                display(computed, "n").unwrap_or_else(|| "?".into())
            )
        }
        ("correlation_matrix", ComputedBody::Correlation(cor)) => {
            let mut pairs = 0;
            let mut significant = 0;
            for (i, row) in cor.p.iter().enumerate() {
                for p in row.iter().skip(i + 1).flatten() {
                    pairs += 1;
                    if *p < ALPHA {
                        significant += 1;
                    }
                }
            }
            let adjustment = match cor.adjustment {
                PAdjust::None => "without adjustment".to_owned(),
                PAdjust::Holm => "after Holm adjustment".to_owned(),
                PAdjust::Bh => "after Benjamini-Hochberg adjustment".to_owned(),
                PAdjust::Bonferroni => "after Bonferroni adjustment".to_owned(),
            };
            format!(
                "{significant} of {pairs} pairs of {} variables are significantly correlated \
                 at α = .05 {adjustment}.",
                cor.variables.len()
            )
        }
        ("describe", ComputedBody::Descriptive(desc)) => {
            let first = desc.variables.first()?;
            let line = apa_line(computed, body).map(|l| l.plain());
            let count = desc
                .variables
                .iter()
                .map(|v| v.name.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            let groups = desc
                .by
                .as_ref()
                .map(|by| format!(" by {by}"))
                .unwrap_or_default();
            match line {
                Some(line) => format!(
                    "Summary of {count} variable{}{groups}; {} has n = {} ({line}).",
                    if count == 1 { "" } else { "s" },
                    first.name,
                    first.n
                ),
                None => format!(
                    "Summary of {count} variable{}{groups}; {} has n = {}.",
                    if count == 1 { "" } else { "s" },
                    first.name,
                    first.n
                ),
            }
        }
        _ => return None,
    };
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computed::{
        Alternative, ComputedValue, CorrelationBody, CorrelationMethod, Engine, GroupSummary,
        Interval, Narrative, TestBody, TestKind, ValueFormat,
    };

    fn engine() -> Engine {
        Engine {
            name: "crest-stats".into(),
            version: "0".into(),
            backend: None,
            platform: None,
        }
    }

    fn shown(v: f64, display: &str) -> ComputedValue {
        ComputedValue {
            display: Some(display.into()),
            ..ComputedValue::new(v)
        }
    }

    fn welch() -> (Computed, ComputedBody) {
        let mut c = Computed::new(
            "st_1",
            "stats.ttest_welch",
            "Welch two-sample t-test",
            engine(),
        );
        c.values.insert("statistic".into(), shown(-3.767, "−3.77"));
        c.values.insert("df".into(), shown(18.33, "18.33"));
        c.values.insert(
            "p_value".into(),
            ComputedValue {
                fmt: Some(ValueFormat::Apa),
                ..shown(0.001_374, ".001")
            },
        );
        c.values.insert(
            "effect".into(),
            ComputedValue {
                ci: Some(Interval {
                    level: 0.95,
                    lo: -2.27,
                    hi: -0.67,
                    method: None,
                }),
                label: Some("large".into()),
                ..shown(-1.48, "−1.48")
            },
        );
        c.narrative = Some(Narrative {
            template: "apa_ttest".into(),
            text_slots: vec![],
        });
        let group = |name: &str, mean| GroupSummary {
            name: name.into(),
            n: 13,
            mean: Some(mean),
            sd: Some(1.0),
            median: None,
        };
        let body = ComputedBody::Test(TestBody {
            test: TestKind::WelchT,
            alternative: Alternative::TwoSided,
            groups: vec![group("auto", 17.15), group("manual", 24.39)],
            observed: None,
            expected: None,
        });
        (c, body)
    }

    #[test]
    fn a_t_test_line_has_italic_symbols_and_the_effect_interval() {
        let (c, body) = welch();
        let line = apa_line(&c, &body).unwrap();
        assert_eq!(
            line.plain(),
            "t(18.33) = −3.77, p = .001, d = −1.48, 95% CI [−2.27, −0.67]"
        );
        assert_eq!(
            line.markdown(),
            "*t*(18.33) = −3.77, *p* = .001, *d* = −1.48, 95% CI [−2.27, −0.67]"
        );
        assert_eq!(
            sentence(&c, &body).unwrap(),
            "Welch two-sample t-test: the difference between auto (M = 17.15, n = 13) and \
             manual (M = 24.39, n = 13) is statistically significant at α = .05 (a large \
             effect, d = −1.48)."
        );
    }

    #[test]
    fn failed_envelopes_and_matrices_have_no_line() {
        let (mut c, body) = welch();
        c.status = ComputedStatus::Error;
        assert_eq!(apa_line(&c, &body), None);
        assert_eq!(sentence(&c, &body), None);

        let mut m = Computed::new("st_2", "stats.correlation_pearson", "Pearson", engine());
        m.narrative = Some(Narrative {
            template: "correlation_matrix".into(),
            text_slots: vec![],
        });
        let body = ComputedBody::Correlation(CorrelationBody {
            method: CorrelationMethod::Pearson,
            adjustment: PAdjust::Holm,
            variables: vec!["a".into(), "b".into(), "c".into()],
            r: vec![vec![None; 3]; 3],
            p: vec![
                vec![None, Some(0.01), Some(0.2)],
                vec![Some(0.01), None, Some(0.04)],
                vec![Some(0.2), Some(0.04), None],
            ],
            n: vec![vec![10; 3]; 3],
        });
        assert_eq!(apa_line(&m, &body), None);
        assert_eq!(
            sentence(&m, &body).unwrap(),
            "2 of 3 pairs of 3 variables are significantly correlated at α = .05 after Holm \
             adjustment."
        );
    }
}
