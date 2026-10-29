//! Renderer of the `reaction` block kind (`show_reaction`): a reaction
//! scheme with its balance check.
//!
//! The reactants, then an arrow with the agents and conditions over it,
//! then the products, each molecule's depiction (tinted like
//! `show_molecule`) above its coefficient, formula and name. Under it a
//! pill says whether the reaction balances, a table gives each element's
//! count per side (rows that do not match in the danger colour, plus the
//! charge), the suggested coefficients come as an equation, and the
//! envelope's provenance and warnings close the card.

use domain::block::{BlockKind, ImageData};
use domain::science::reaction::{ReactionComponent, ReactionRole, ReactionSpec, Stoichiometry};
use gpui::{
    AnyElement, App, FontWeight, Hsla, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::computed::grid::{Grid, GridCell};
use super::computed::provenance_text;
use super::molecule::{subscript_formula, svg_data, tinted_picture};
use crate::timeline::TimelineEntry;

/// Tallest a depiction in the scheme is drawn.
const MAX_HEIGHT: f32 = 150.0;

/// The depictions of a block as image data, made once.
struct Depictions(Vec<Option<ImageData>>);

/// The balance table: element, reactants, products, Δ; mismatches in
/// `danger`, then the charge.
pub fn balance_grid(spec: &ReactionSpec, danger: Hsla) -> Grid {
    let mut grid = Grid::new(["Element", "Reactants", "Products", "Δ"]);
    let cell = |text: String, bad: bool| GridCell {
        text,
        bg: bad.then(|| danger.opacity(0.12)),
        fg: bad.then_some(danger),
    };
    for e in &spec.elements {
        let bad = e.delta() != 0;
        grid.rows.push(vec![
            cell(e.element.clone(), bad),
            cell(e.reactants.to_string(), bad),
            cell(e.products.to_string(), bad),
            cell(format!("{:+}", e.delta()), bad),
        ]);
    }
    let (l, r) = spec.charge;
    let bad = l != r;
    grid.rows.push(vec![
        cell("charge".into(), bad),
        cell(format!("{l:+}"), bad),
        cell(format!("{r:+}"), bad),
        cell(format!("{:+}", r - l), bad),
    ]);
    grid
}

/// The verdict pill text and whether it is good news.
#[must_use]
pub fn verdict(spec: &ReactionSpec) -> (String, bool) {
    if spec.is_balanced() {
        return ("✓ balanced: atoms and charge".to_owned(), true);
    }
    let text = match &spec.stoichiometry {
        Stoichiometry::Suggested { coefficients } => format!(
            "✗ not balanced · balances as {}",
            spec.equation(Some(coefficients))
        ),
        Stoichiometry::Impossible => {
            "✗ not balanced · no coefficients balance these molecules".to_owned()
        }
        Stoichiometry::Ambiguous => {
            "✗ not balanced · several balances possible; give coefficients".to_owned()
        }
        Stoichiometry::Balanced | Stoichiometry::NotChecked => "✗ not balanced".to_owned(),
    };
    (text, false)
}

fn molecule_card(
    ix: usize,
    c: &ReactionComponent,
    data: Option<&ImageData>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let picture = data.map(|d| tinted_picture(d, MAX_HEIGHT, window, cx));
    let theme = cx.theme();
    let formula = c
        .molecule
        .formula
        .as_deref()
        .map(subscript_formula)
        .unwrap_or_else(|| c.molecule.smiles.clone());
    let label = if c.coefficient == 1 {
        formula
    } else {
        format!("{} {formula}", c.coefficient)
    };
    v_flex()
        .debug_selector(move || format!("reaction-molecule-{ix}"))
        .items_center()
        .gap_1()
        .children(picture)
        .when_some(c.molecule.error.clone(), |d, e| {
            d.child(
                div()
                    .text_xs()
                    .text_color(theme.danger)
                    .child(SharedString::from(e)),
            )
        })
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(SharedString::from(label)),
        )
        .when_some(c.molecule.name.clone(), |d, name| {
            d.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(name)),
            )
        })
        .into_any_element()
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Reaction(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    let svgs: Vec<Option<String>> = spec
        .components
        .iter()
        .map(|c| c.molecule.svg.clone())
        .collect();
    let depictions = super::block_state(entry.block.id, window, cx, move |_, _| {
        Depictions(svgs.iter().map(|s| s.as_deref().map(svg_data)).collect())
    });
    let data = depictions.read(cx).0.clone();
    let mut card = v_flex()
        .w_full()
        .gap_2()
        .debug_selector(|| "reaction".to_owned());
    if let Some(title) = &spec.title {
        card = card.child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(SharedString::from(title.clone())),
        );
    }
    if let Some(error) = &spec.error {
        let theme = cx.theme();
        return card
            .child(
                div()
                    .text_sm()
                    .text_color(theme.danger)
                    .debug_selector(|| "reaction-error".to_owned())
                    .child(SharedString::from(error.clone())),
            )
            .child(
                div()
                    .font_family(theme.mono_font_family.clone())
                    .text_sm()
                    .child(SharedString::from(spec.smiles.clone())),
            )
            .into_any_element();
    }
    let side = |role: ReactionRole, window: &mut Window, cx: &mut App| {
        let mut items: Vec<AnyElement> = Vec::new();
        for (ix, c) in spec
            .components
            .iter()
            .enumerate()
            .filter(|(_, c)| c.role == role)
        {
            if !items.is_empty() {
                items.push(div().text_lg().child("+").into_any_element());
            }
            items.push(molecule_card(
                ix,
                c,
                data.get(ix).and_then(Option::as_ref),
                window,
                cx,
            ));
        }
        items
    };
    let reactants = side(ReactionRole::Reactant, window, cx);
    let products = side(ReactionRole::Product, window, cx);
    let theme = cx.theme();
    let (muted, success, danger, warning) = (
        theme.muted_foreground,
        theme.success,
        theme.danger,
        theme.warning,
    );
    let over: Vec<String> = spec
        .side(ReactionRole::Agent)
        .map(|c| {
            c.molecule
                .name
                .clone()
                .or_else(|| c.molecule.formula.as_deref().map(subscript_formula))
                .unwrap_or_else(|| c.molecule.smiles.clone())
        })
        .chain(spec.conditions.clone())
        .collect();
    let arrow = v_flex()
        .items_center()
        .debug_selector(|| "reaction-arrow".to_owned())
        .px_2()
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(SharedString::from(over.join(", "))),
        )
        .child(div().text_xl().child("⟶"));
    let scheme = h_flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .debug_selector(|| "reaction-scheme".to_owned())
        .children(reactants)
        .child(arrow)
        .children(products);
    let (text, good) = verdict(spec);
    let color = if good { success } else { danger };
    let pill = div()
        .px_1p5()
        .rounded_sm()
        .border_1()
        .border_color(color)
        .text_xs()
        .text_color(color)
        .debug_selector(|| "reaction-verdict".to_owned())
        .child(SharedString::from(text));
    card.child(scheme)
        .child(h_flex().child(pill))
        .child(balance_grid(spec, danger).render("reaction-balance", cx))
        .children(spec.balance.warnings.iter().map(|w| {
            div()
                .text_xs()
                .text_color(warning)
                .child(SharedString::from(w.text.clone()))
        }))
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .debug_selector(|| "reaction-provenance".to_owned())
                .child(SharedString::from(format!(
                    "{} · {}",
                    spec.balance.method.name,
                    provenance_text(&spec.balance)
                ))),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::computed::{Computed, Engine};
    use domain::science::MoleculeSpec;
    use domain::science::reaction::ElementBalance;
    use gpui::{TestAppContext, point, px, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><rect width="40" height="30" fill="white"/></svg>"#;

    fn component(role: ReactionRole, formula: &str) -> ReactionComponent {
        ReactionComponent {
            role,
            molecule: MoleculeSpec {
                smiles: formula.into(),
                name: None,
                svg: Some(SVG.into()),
                formula: Some(formula.into()),
                mw: Some(1.0),
                charge: Some(0),
                canonical: None,
                error: None,
                claimed_formula: None,
                claimed_matches: None,
            },
            coefficient: 1,
            atoms: BTreeMap::new(),
        }
    }

    fn spec() -> ReactionSpec {
        ReactionSpec {
            title: Some("water".into()),
            smiles: "[H][H].O=O>[Pt]>O".into(),
            conditions: Some("Δ".into()),
            components: vec![
                component(ReactionRole::Reactant, "H2"),
                component(ReactionRole::Reactant, "O2"),
                component(ReactionRole::Agent, "Pt"),
                component(ReactionRole::Product, "H2O"),
            ],
            elements: vec![
                ElementBalance {
                    element: "H".into(),
                    reactants: 2,
                    products: 2,
                },
                ElementBalance {
                    element: "O".into(),
                    reactants: 2,
                    products: 1,
                },
            ],
            charge: (0, 0),
            stoichiometry: Stoichiometry::Suggested {
                coefficients: vec![2, 1, 2],
            },
            balance: Computed::new(
                "ch_1",
                "chem.reaction_balance",
                "Atom and charge balance",
                Engine {
                    name: "crest-chem".into(),
                    version: "0.14.0".into(),
                    backend: None,
                    platform: None,
                },
            ),
            error: None,
        }
    }

    #[test]
    fn verdict_and_table() {
        assert_eq!(
            verdict(&spec()),
            (
                "✗ not balanced · balances as 2 H2 + O2 → 2 H2O".to_owned(),
                false
            )
        );
        let grid = balance_grid(&spec(), gpui::red());
        assert_eq!(grid.rows.len(), 3);
        assert_eq!(grid.rows[1][3].text, "-1");
        assert!(grid.rows[1][3].fg.is_some());
        assert!(grid.rows[0][3].fg.is_none());
        assert_eq!(grid.rows[2][0].text, "charge");
    }

    #[gpui::test]
    fn draws_the_scheme_and_the_balance(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let mut bad = spec();
        bad.error = Some("invalid reaction SMILES".into());
        view.update(cx, |view, cx| {
            view.apply_all(
                [
                    DomainEvent::BlockAdded {
                        block: Block::new(BlockKind::Reaction(Box::new(spec()))),
                    },
                    DomainEvent::BlockAdded {
                        block: Block::new(BlockKind::Reaction(Box::new(bad))),
                    },
                ],
                cx,
            );
        });
        for _ in 0..2 {
            cx.draw(point(px(0.), px(0.)), size(px(900.), px(1000.)), |_, _| {
                div().size_full().child(view.clone())
            });
            cx.run_until_parked();
        }
        for selector in [
            "reaction",
            "reaction-scheme",
            "reaction-arrow",
            "reaction-molecule-0",
            "reaction-molecule-3",
            "reaction-verdict",
            "reaction-provenance",
            "reaction-error",
            "molecule-picture",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
    }
}
