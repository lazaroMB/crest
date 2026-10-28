//! Renderer of the `molecule` block kind (`show_molecule`): the 2D
//! depiction and a validation strip of values computed from the SMILES.
//!
//! - The depiction is chematic's SVG (black bonds and carbons on white,
//!   CPK heteroatom labels), tinted to the theme (bonds in the
//!   foreground color, no background, CPK colors kept) and rasterized off
//!   the UI thread ([`super::svg`]), on a bordered card.
//! - The strip: the formula with subscripts, the molecular weight in
//!   g/mol, the net charge and the canonical SMILES in the monospace font.
//! - When the model stated a formula, a chip says whether it matches the
//!   computed one (green) or not (red, with both formulas).
//! - A SMILES that did not parse shows the parse error instead.

use base64::Engine as _;
use domain::block::{BlockKind, ImageData};
use domain::science::MoleculeSpec;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, FontWeight, Hsla, ImageSource, InteractiveElement as _, IntoElement as _,
    ObjectFit, ParentElement as _, SharedString, Styled as _, StyledImage as _, Window, div, img,
    px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::svg::SvgState;
use crate::timeline::TimelineEntry;

/// Tallest the depiction is drawn, in pixels.
const MAX_HEIGHT: f32 = 280.0;

/// The minus sign of charges.
const MINUS: char = '\u{2212}';

/// `formula` with its counts as Unicode subscripts: `C8H10N4O2` →
/// `C₈H₁₀N₄O₂`.
#[must_use]
pub fn subscript_formula(formula: &str) -> String {
    formula
        .chars()
        .map(|c| match c.to_digit(10) {
            Some(d) => char::from_u32(0x2080 + d).unwrap_or(c),
            None => c,
        })
        .collect()
}

/// A net charge as chemists write it: `0`, `+1`, `−2`.
#[must_use]
pub fn charge_text(charge: i32) -> String {
    match charge {
        0 => "0".to_owned(),
        c if c > 0 => format!("+{c}"),
        c => format!("{MINUS}{}", c.unsigned_abs()),
    }
}

/// What the claimed-formula chip says and whether it is good news;
/// `None` when the model claimed nothing.
#[must_use]
pub fn claim_text(spec: &MoleculeSpec) -> Option<(String, Option<bool>)> {
    let claimed = spec.claimed_formula.as_deref()?;
    let computed = spec.formula.as_deref().unwrap_or("?");
    Some(match spec.claimed_matches {
        Some(true) => (
            format!("✓ claimed {} matches", subscript_formula(claimed)),
            Some(true),
        ),
        Some(false) => (
            format!(
                "✗ model said {}, computed {}",
                subscript_formula(claimed),
                subscript_formula(computed)
            ),
            Some(false),
        ),
        None => (format!("claimed {claimed} could not be checked"), None),
    })
}

/// The SVG of the block as image data, made once per block.
struct Depiction(Option<ImageData>);

fn chip(text: String, color: Hsla) -> gpui::Div {
    div()
        .px_1p5()
        .rounded_sm()
        .border_1()
        .border_color(color)
        .text_xs()
        .text_color(color)
        .child(text)
}

fn field(label: &'static str, value: String, mono: Option<SharedString>, cx: &App) -> gpui::Div {
    let theme = cx.theme();
    h_flex()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label),
        )
        .child(
            div()
                .when_some(mono, |this, font| this.font_family(font))
                .child(value),
        )
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Molecule(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    let svg = spec.svg.clone();
    let depiction = super::block_state(entry.block.id, window, cx, move |_, _| {
        Depiction(svg.map(|svg| ImageData::Base64 {
            data: base64::engine::general_purpose::STANDARD.encode(svg),
        }))
    });
    let data = depiction.read(cx).0.clone();
    let state = data.as_ref().map(|data| {
        let tint = super::svg::Tint::from_theme(cx);
        super::svg::use_svg_tinted(data, 1.0, Some(tint), window, cx)
    });
    let theme = cx.theme();
    let mono = theme.mono_font_family.clone();
    let mut card = v_flex().gap_2().debug_selector(|| "molecule".to_owned());
    if let Some(name) = &spec.name {
        card = card.child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(SharedString::from(name.clone())),
        );
    }
    if let Some(error) = &spec.error {
        return card
            .child(
                chip(format!("SMILES parse error: {error}"), theme.danger)
                    .debug_selector(|| "molecule-error".to_owned()),
            )
            .child(
                div()
                    .font_family(mono)
                    .text_sm()
                    .child(SharedString::from(spec.smiles.clone())),
            )
            .into_any_element();
    }
    let picture = match state {
        Some(SvgState::Ready(raster)) => {
            let (w, h) = raster.size;
            let scale = (MAX_HEIGHT / h).min(1.0);
            img(ImageSource::Render(raster.image.clone()))
                .debug_selector(|| "molecule-picture".to_owned())
                .w(px(w * scale))
                .max_w_full()
                .aspect_ratio(w / h)
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        }
        Some(SvgState::Failed(err)) => div()
            .text_color(theme.danger)
            .child(SharedString::from(err))
            .into_any_element(),
        Some(SvgState::Loading) | None => div()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child("drawing…")
            .into_any_element(),
    };
    card = card.child(
        h_flex().child(
            div()
                .p_2()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .child(picture),
        ),
    );
    let mut strip = h_flex()
        .gap_3()
        .flex_wrap()
        .text_sm()
        .debug_selector(|| "molecule-strip".to_owned());
    if let Some(formula) = &spec.formula {
        strip = strip.child(field("formula", subscript_formula(formula), None, cx));
    }
    if let Some(mw) = spec.mw {
        strip = strip.child(field("MW", format!("{mw:.2} g/mol"), None, cx));
    }
    if let Some(charge) = spec.charge {
        strip = strip.child(field("charge", charge_text(charge), None, cx));
    }
    if let Some(canonical) = &spec.canonical {
        strip = strip.child(field("canonical", canonical.clone(), Some(mono), cx));
    }
    card = card.child(strip);
    if let Some((text, good)) = claim_text(spec) {
        let color = match good {
            Some(true) => theme.success,
            Some(false) => theme.danger,
            None => theme.warning,
        };
        card = card.child(
            h_flex().child(chip(text, color).debug_selector(|| "molecule-claim".to_owned())),
        );
    }
    card.into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use gpui::{TestAppContext, point, size};

    use super::*;
    use crate::timeline::TimelineView;

    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><rect width="40" height="30" fill="white"/></svg>"#;

    fn spec() -> MoleculeSpec {
        MoleculeSpec {
            smiles: "CCO".into(),
            name: Some("ethanol".into()),
            svg: Some(SVG.into()),
            formula: Some("C2H6O".into()),
            mw: Some(46.069),
            charge: Some(0),
            canonical: Some("CCO".into()),
            error: None,
            claimed_formula: Some("C2H4O".into()),
            claimed_matches: Some(false),
        }
    }

    #[test]
    fn strip_texts() {
        assert_eq!(subscript_formula("C8H10N4O2"), "C₈H₁₀N₄O₂");
        assert_eq!(charge_text(-1), "−1");
        assert_eq!(charge_text(2), "+2");
        assert_eq!(
            claim_text(&spec()),
            Some(("✗ model said C₂H₄O, computed C₂H₆O".into(), Some(false)))
        );
        let mut unclaimed = spec();
        unclaimed.claimed_formula = None;
        assert_eq!(claim_text(&unclaimed), None);
    }

    #[gpui::test]
    fn draws_the_depiction_strip_and_claim(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let bad = MoleculeSpec {
            error: Some("invalid SMILES: unexpected end of input at position 4".into()),
            svg: None,
            formula: None,
            mw: None,
            charge: None,
            canonical: None,
            claimed_formula: None,
            claimed_matches: None,
            ..spec()
        };
        view.update(cx, |view, cx| {
            view.apply_all(
                [
                    DomainEvent::BlockAdded {
                        block: Block::new(BlockKind::Molecule(spec())),
                    },
                    DomainEvent::BlockAdded {
                        block: Block::new(BlockKind::Molecule(bad)),
                    },
                ],
                cx,
            );
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(point(px(0.), px(0.)), size(px(800.), px(900.)), |_, _| {
                div().size_full().child(view.clone())
            });
            cx.run_until_parked();
        };
        draw(cx);
        // The SVG is rasterized in the background, then redrawn.
        draw(cx);
        for selector in [
            "molecule",
            "molecule-picture",
            "molecule-strip",
            "molecule-claim",
            "molecule-error",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
    }
}
