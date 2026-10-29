//! Sentences of the engineering cards: Bode / Nyquist, Karnaugh maps,
//! beams, tolerance stacks, BOM costs.
//!
//! As elsewhere, the words come from fixed templates and every number
//! from the envelope (`display` of a value) or the engine's body.

use super::display;
use crate::computed::{Computed, ComputedBody};

/// The sentence of an engineering template, if it has one.
pub(super) fn sentence(template: &str, computed: &Computed, body: &ComputedBody) -> Option<String> {
    match (template, body) {
        ("bode", ComputedBody::Bode(b)) => {
            let unit = b.unit.symbol();
            let pm = match (
                display(computed, "phase_margin"),
                display(computed, "gain_crossover"),
            ) {
                (Some(pm), Some(wc)) => {
                    format!("The phase margin is {pm}° at the gain crossover {wc} {unit}")
                }
                _ => "|G| does not cross 0 dB on the sweep, so there is no phase margin".to_owned(),
            };
            let gm = match (
                display(computed, "gain_margin"),
                display(computed, "phase_crossover"),
            ) {
                (Some(gm), Some(wpc)) => {
                    format!("the gain margin is {gm} dB at the phase crossover {wpc} {unit}")
                }
                _ => "the phase never reaches −180°, so the gain margin is infinite".to_owned(),
            };
            let closed = computed
                .value("closed_loop_max_re")
                .and_then(|v| v.label.as_deref())
                .map(|label| format!(" With unity feedback the closed loop is {label}."))
                .unwrap_or_default();
            Some(format!("{pm}; {gm}.{closed}"))
        }
        ("karnaugh", ComputedBody::Karnaugh(k)) => {
            let how = match k.method {
                crate::computed::CoverMethod::Exact => "a minimal",
                crate::computed::CoverMethod::Greedy => "a (possibly not minimal)",
            };
            Some(format!(
                "F({}) has {how} sum of products F = {} ({} terms, {} literals) and product \
                 of sums F = {}.",
                k.variables.join(", "),
                k.sop_expression,
                display(computed, "sop_terms")?,
                display(computed, "sop_literals")?,
                k.pos_expression
            ))
        }
        ("beam", ComputedBody::Beam(b)) => {
            let reactions: Vec<String> = b
                .reactions
                .iter()
                .map(|r| {
                    let key = format!("reaction.{}", r.name);
                    let force = display(computed, &key).unwrap_or_default();
                    match display(computed, &format!("moment.{}", r.name)) {
                        Some(m) => format!(
                            "{} = {force} {} and {m} {}",
                            r.name,
                            b.force_unit.symbol(),
                            b.moment_unit()
                        ),
                        None => format!("{} = {force} {}", r.name, b.force_unit.symbol()),
                    }
                })
                .collect();
            let mut text = format!("Reactions: {}.", reactions.join(", "));
            let at = |key: &str| display(computed, key).unwrap_or_default();
            if let Some(m) = display(computed, "max_moment") {
                text.push_str(&format!(
                    " The largest sagging moment is {m} {} at x = {} {}.",
                    b.moment_unit(),
                    at("max_moment_at"),
                    b.length_unit.symbol()
                ));
            }
            if let Some(m) = display(computed, "min_moment") {
                text.push_str(&format!(
                    " The largest hogging moment is {m} {} at x = {} {}.",
                    b.moment_unit(),
                    at("min_moment_at"),
                    b.length_unit.symbol()
                ));
            }
            if let Some(d) = display(computed, "max_deflection") {
                text.push_str(&format!(
                    " The largest deflection is {d} {} at x = {} {}.",
                    b.length_unit.deflection_unit().symbol(),
                    at("max_deflection_at"),
                    b.length_unit.symbol()
                ));
            }
            Some(text)
        }
        ("tolerance", ComputedBody::Tolerance(t)) => {
            let unit = t
                .unit
                .as_deref()
                .map(|u| format!(" {u}"))
                .unwrap_or_default();
            let mut text = format!(
                "The stack is {}{unit} nominal: {} to {}{unit} worst case, {} to {}{unit} by RSS.",
                display(computed, "nominal")?,
                display(computed, "worst_case_min")?,
                display(computed, "worst_case_max")?,
                display(computed, "rss_min")?,
                display(computed, "rss_max")?,
            );
            if let Some(p) = display(computed, "out_of_spec") {
                text.push_str(&format!(
                    " In {} seeded Monte Carlo draws, {p} fall outside the limits.",
                    t.samples
                ));
            }
            if let Some(top) = t
                .dimensions
                .iter()
                .max_by(|a, b| a.variance_share.total_cmp(&b.variance_share))
            {
                text.push_str(&format!(" {} contributes most of the variance.", top.name));
            }
            Some(text)
        }
        ("bom", ComputedBody::Bom(b)) => {
            let currency = b
                .currency
                .as_deref()
                .map(|c| format!(" {c}"))
                .unwrap_or_default();
            let mut text = format!(
                "One unit costs {}{currency} over {} lines",
                display(computed, "total")?,
                display(computed, "lines")?
            );
            if let Some(all) = display(computed, "total_builds") {
                text.push_str(&format!(", {all}{currency} for {} units", b.builds));
            }
            text.push('.');
            let missing = b.missing_prices().len();
            if missing > 0 {
                text.push_str(&format!(
                    " {missing} lines have no price and are not in the total."
                ));
            }
            Some(text)
        }
        _ => None,
    }
}
