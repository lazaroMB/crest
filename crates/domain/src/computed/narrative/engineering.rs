//! Sentences of the engineering cards: Bode / Nyquist.
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
        _ => None,
    }
}
