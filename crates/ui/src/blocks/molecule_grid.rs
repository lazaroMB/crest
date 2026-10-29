//! Renderer of the `molecule_grid` block kind (`show_molecule_grid`): a
//! structure–activity grid.
//!
//! A toolbar sorts the molecules (given order, activity, MW, cLogP, TPSA,
//! HBD, HBA, rotatable bonds) up or down, filters them (text matches the
//! id, name, formula or SMILES; `mw < 500`, `logp <= 3`, `tpsa > 90`,
//! `activity < 100` compare a value; clauses joined by commas or spaces
//! all apply) and can keep only rule-of-five compliant ones. Each card
//! has the depiction, the id and name, the activity on a chip whose
//! colour goes from faint (worst) to solid (best) in the given direction,
//! and the descriptors the engine computed. The envelope's provenance and
//! warnings come last.

use std::rc::Rc;

use domain::block::{BlockId, BlockKind, ImageData};
use domain::science::molecule_grid::{GridMolecule, MoleculeGridSpec};
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};

use super::computed::provenance_text;
use super::molecule::{subscript_formula, svg_data, tinted_picture};
use crate::timeline::TimelineEntry;
use crate::widgets::{readout, register_input, tool_button};

/// Cards drawn at most; the rest are counted.
pub const MAX_CARDS: usize = 120;
/// Width of a card, in pixels.
const CARD_WIDTH: f32 = 190.0;
/// Tallest a depiction is drawn.
const PICTURE_HEIGHT: f32 = 120.0;

/// What the grid is sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    /// The order given.
    Given,
    /// The activity.
    Activity,
    /// A descriptor.
    Descriptor(&'static str),
}

/// The sort choices with their labels.
pub const SORTS: [(SortKey, &str); 8] = [
    (SortKey::Given, "given"),
    (SortKey::Activity, "activity"),
    (SortKey::Descriptor("mw"), "MW"),
    (SortKey::Descriptor("logp"), "cLogP"),
    (SortKey::Descriptor("tpsa"), "TPSA"),
    (SortKey::Descriptor("hbd"), "HBD"),
    (SortKey::Descriptor("hba"), "HBA"),
    (SortKey::Descriptor("rotatable_bonds"), "rot"),
];

/// A comparison of a filter clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

/// One clause of a filter.
#[derive(Debug, Clone, PartialEq)]
pub enum Clause {
    /// A value compared with a number (`mw < 500`).
    Compare(String, Op, f64),
    /// Text in the id, name, formula or SMILES (case-insensitive).
    Text(String),
}

/// The value names a filter can compare.
const FIELDS: [&str; 11] = [
    "activity",
    "mw",
    "heavy_atoms",
    "hbd",
    "hba",
    "rotatable_bonds",
    "rot",
    "rings",
    "logp",
    "tpsa",
    "ro5",
];

/// The clauses of `text`: comparisons of a known value with a number,
/// the rest as text.
#[must_use]
pub fn parse_filter(text: &str) -> Vec<Clause> {
    // Glue `mw < 500` into `mw<500` so splitting on spaces keeps it whole.
    let mut glued = text.to_ascii_lowercase();
    for op in ["<=", ">=", "<", ">", "="] {
        glued = glued
            .replace(&format!(" {op} "), op)
            .replace(&format!(" {op}"), op);
        glued = glued.replace(&format!("{op} "), op);
    }
    glued
        .split([',', ' '])
        .map(str::trim)
        .filter(|w| !w.is_empty() && *w != "and")
        .map(|word| {
            for (sym, op) in [
                ("<=", Op::Le),
                (">=", Op::Ge),
                ("<", Op::Lt),
                (">", Op::Gt),
                ("=", Op::Eq),
            ] {
                if let Some((field, value)) = word.split_once(sym)
                    && FIELDS.contains(&field)
                    && let Ok(v) = value.parse::<f64>()
                {
                    return Clause::Compare(field.to_owned(), op, v);
                }
            }
            Clause::Text(word.to_owned())
        })
        .collect()
}

/// The value `field` of `m`.
fn field_value(m: &GridMolecule, field: &str) -> Option<f64> {
    match field {
        "activity" => m.activity,
        "rot" => m.descriptors.as_ref()?.get("rotatable_bonds"),
        "ro5" => m.descriptors.as_ref()?.get("ro5_violations"),
        other => m.descriptors.as_ref()?.get(other),
    }
}

/// Whether `m` passes every clause.
#[must_use]
pub fn passes(m: &GridMolecule, clauses: &[Clause]) -> bool {
    clauses.iter().all(|c| match c {
        Clause::Compare(field, op, v) => field_value(m, field).is_some_and(|x| match op {
            Op::Lt => x < *v,
            Op::Le => x <= *v,
            Op::Gt => x > *v,
            Op::Ge => x >= *v,
            Op::Eq => (x - v).abs() < 1e-9,
        }),
        Clause::Text(t) => [
            Some(m.id.as_str()),
            m.molecule.name.as_deref(),
            m.molecule.formula.as_deref(),
            Some(m.molecule.smiles.as_str()),
        ]
        .into_iter()
        .flatten()
        .any(|s| s.to_ascii_lowercase().contains(t)),
    })
}

/// The molecules shown: filtered, optionally rule-of-five compliant only,
/// then sorted (missing values last whatever the direction).
#[must_use]
pub fn visible(
    spec: &MoleculeGridSpec,
    clauses: &[Clause],
    ro5_only: bool,
    sort: SortKey,
    descending: bool,
) -> Vec<usize> {
    let mut shown: Vec<usize> = spec
        .molecules
        .iter()
        .enumerate()
        .filter(|(_, m)| passes(m, clauses))
        .filter(|(_, m)| {
            !ro5_only
                || m.descriptors
                    .as_ref()
                    .is_some_and(|d| d.lipinski_violations() == 0)
        })
        .map(|(i, _)| i)
        .collect();
    let key = |i: usize| -> Option<f64> {
        let m = &spec.molecules[i];
        match sort {
            SortKey::Given => None,
            SortKey::Activity => m.activity,
            SortKey::Descriptor(d) => m.descriptors.as_ref().and_then(|x| x.get(d)),
        }
    };
    if sort == SortKey::Given {
        if descending {
            shown.reverse();
        }
        return shown;
    }
    shown.sort_by(|&a, &b| match (key(a), key(b)) {
        (Some(x), Some(y)) => {
            let ord = x.total_cmp(&y);
            if descending { ord.reverse() } else { ord }
        }
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.cmp(&b),
    });
    shown
}

/// Per-block state: the depictions, the filter and the sort.
pub struct MoleculeGridView {
    spec: Rc<MoleculeGridSpec>,
    pictures: Rc<Vec<Option<ImageData>>>,
    input: Entity<InputState>,
    clauses: Vec<Clause>,
    sort: SortKey,
    descending: bool,
    ro5_only: bool,
    _input_events: Subscription,
}

fn pictures(spec: &MoleculeGridSpec) -> Rc<Vec<Option<ImageData>>> {
    Rc::new(
        spec.molecules
            .iter()
            .map(|m| m.molecule.svg.as_deref().map(svg_data))
            .collect(),
    )
}

impl MoleculeGridView {
    fn new(spec: &MoleculeGridSpec, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("filter · text, mw<500, logp<=3, tpsa>90")
        });
        register_input(&input, cx);
        let subscription =
            cx.subscribe(&input, |this: &mut Self, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let text = input.read(cx).value().to_string();
                    this.set_filter(&text, cx);
                }
            });
        Self {
            spec: Rc::new(spec.clone()),
            pictures: pictures(spec),
            input,
            clauses: Vec::new(),
            sort: SortKey::Given,
            descending: false,
            ro5_only: false,
            _input_events: subscription,
        }
    }

    fn sync(&mut self, spec: &MoleculeGridSpec) {
        if *self.spec != *spec {
            self.spec = Rc::new(spec.clone());
            self.pictures = pictures(spec);
        }
    }

    /// Filters with `text` (see [`parse_filter`]).
    pub fn set_filter(&mut self, text: &str, cx: &mut Context<Self>) {
        self.clauses = parse_filter(text);
        cx.notify();
    }

    /// Sorts by `key`; the same key again flips the direction.
    pub fn sort_by(&mut self, key: SortKey, cx: &mut Context<Self>) {
        if self.sort == key {
            self.descending = !self.descending;
        } else {
            self.sort = key;
            self.descending = false;
        }
        cx.notify();
    }

    /// Keeps only rule-of-five compliant molecules, or all.
    pub fn set_ro5_only(&mut self, on: bool, cx: &mut Context<Self>) {
        self.ro5_only = on;
        cx.notify();
    }

    /// The molecules shown, in order.
    pub fn visible(&self) -> Vec<usize> {
        visible(
            &self.spec,
            &self.clauses,
            self.ro5_only,
            self.sort,
            self.descending,
        )
    }

    fn toolbar(&self, shown: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let total = self.spec.molecules.len();
        let mut row = h_flex()
            .gap_1()
            .flex_wrap()
            .text_xs()
            .child(
                div()
                    .w(px(280.))
                    .debug_selector(|| "molecule-grid-filter".to_owned())
                    .child(Input::new(&self.input).small()),
            )
            .child(
                readout(format!("{shown} of {total}"), cx)
                    .debug_selector(|| "molecule-grid-count".to_owned()),
            )
            .child(div().pl_2().child("sort:"));
        for (i, (key, label)) in SORTS.iter().enumerate() {
            let key = *key;
            let text = if self.sort == key && key != SortKey::Given {
                format!("{label} {}", if self.descending { "↓" } else { "↑" })
            } else {
                (*label).to_owned()
            };
            row = row.child(
                tool_button(SORT_IDS[i], text, cx)
                    .when(self.sort == key, |b| b.border_1())
                    .on_click(cx.listener(move |this, _, _, cx| this.sort_by(key, cx))),
            );
        }
        row.child(
            tool_button(
                "molecule-grid-ro5",
                if self.ro5_only {
                    "Ro5: compliant"
                } else {
                    "Ro5: all"
                },
                cx,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                let on = !this.ro5_only;
                this.set_ro5_only(on, cx);
            })),
        )
    }

    fn card(&self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let m = &self.spec.molecules[ix];
        let picture = self
            .pictures
            .get(ix)
            .and_then(Option::as_ref)
            .map(|data| tinted_picture(data, PICTURE_HEIGHT, window, cx));
        let theme = cx.theme();
        let (muted, border, success, danger) = (
            theme.muted_foreground,
            theme.border,
            theme.success,
            theme.danger,
        );
        let unit = self
            .spec
            .activity_unit
            .as_deref()
            .map(|u| format!(" {u}"))
            .unwrap_or_default();
        let activity = m.activity.map(|a| {
            let score = self.spec.activity_score(a).unwrap_or(0.5) as f32;
            div()
                .px_1()
                .rounded_sm()
                .bg(success.opacity(0.12 + 0.6 * score))
                .child(SharedString::from(format!("{a}{unit}")))
        });
        let details = m.descriptors.as_ref().map(|d| {
            let violations = d.lipinski_violations();
            v_flex()
                .text_xs()
                .text_color(muted)
                .child(format!("MW {:.1} · cLogP {:.2}", d.mw, d.logp))
                .child(format!(
                    "TPSA {:.1} · HBD {} · HBA {}",
                    d.tpsa, d.hbd, d.hba
                ))
                .child(format!(
                    "rot {} · rings {} · heavy {}",
                    d.rotatable_bonds, d.rings, d.heavy_atoms
                ))
                .when(violations > 0, |c| {
                    c.child(
                        div()
                            .text_color(danger)
                            .child(format!("{violations} Ro5 violation(s)")),
                    )
                })
        });
        v_flex()
            .id(("molecule-grid-card", ix))
            .debug_selector(move || format!("molecule-grid-card-{ix}"))
            .w(px(CARD_WIDTH))
            .p_1()
            .gap_0p5()
            .rounded_md()
            .border_1()
            .border_color(border)
            .children(picture.map(|p| h_flex().justify_center().h(px(PICTURE_HEIGHT)).child(p)))
            .child(
                h_flex()
                    .gap_1()
                    .text_sm()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(SharedString::from(m.id.clone())),
                    )
                    .children(activity),
            )
            .when_some(m.molecule.name.clone(), |c, name| {
                c.child(div().text_xs().child(SharedString::from(name)))
            })
            .when_some(
                m.molecule.formula.as_deref().map(subscript_formula),
                |c, f| c.child(div().text_xs().child(SharedString::from(f))),
            )
            .when_some(m.molecule.error.clone(), |c, e| {
                c.child(
                    div()
                        .text_xs()
                        .text_color(danger)
                        .child(SharedString::from(e)),
                )
            })
            .children(details)
            .into_any_element()
    }
}

/// Ids of the sort buttons, in [`SORTS`] order.
const SORT_IDS: [&str; 8] = [
    "molecule-grid-sort-given",
    "molecule-grid-sort-activity",
    "molecule-grid-sort-mw",
    "molecule-grid-sort-logp",
    "molecule-grid-sort-tpsa",
    "molecule-grid-sort-hbd",
    "molecule-grid-sort-hba",
    "molecule-grid-sort-rot",
];

impl Render for MoleculeGridView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.visible();
        let toolbar = self.toolbar(shown.len(), cx);
        let cards: Vec<AnyElement> = shown
            .iter()
            .take(MAX_CARDS)
            .map(|&ix| self.card(ix, window, cx))
            .collect();
        let more = shown.len().saturating_sub(MAX_CARDS);
        let spec = Rc::clone(&self.spec);
        let theme = cx.theme();
        let (muted, warning) = (theme.muted_foreground, theme.warning);
        let label = match (&spec.activity_label, &spec.activity_unit) {
            (Some(l), Some(u)) => Some(format!("activity: {l} ({u})")),
            (Some(l), None) => Some(format!("activity: {l}")),
            (None, Some(u)) => Some(format!("activity in {u}")),
            (None, None) => None,
        };
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "molecule-grid".to_owned())
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when_some(spec.title.clone(), |d, t| {
                d.child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(SharedString::from(t)),
                )
            })
            .when_some(label, |d, l| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(SharedString::from(l)),
                )
            })
            .child(toolbar)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .debug_selector(|| "molecule-grid-cards".to_owned())
                    .children(cards),
            )
            .when(more > 0, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("… {more} more; narrow the filter to see them")),
                )
            })
            .children(spec.descriptors.warnings.iter().map(|w| {
                div()
                    .text_xs()
                    .text_color(warning)
                    .child(SharedString::from(w.text.clone()))
            }))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .debug_selector(|| "molecule-grid-provenance".to_owned())
                    .child(SharedString::from(format!(
                        "{} · {}",
                        spec.descriptors.method.name,
                        provenance_text(&spec.descriptors)
                    ))),
            )
    }
}

/// The grid entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &MoleculeGridSpec,
    window: &mut Window,
    cx: &mut App,
) -> Entity<MoleculeGridView> {
    let view = super::block_state(id, window, cx, |window, cx| {
        MoleculeGridView::new(spec, window, cx)
    });
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::MoleculeGrid(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::computed::{Computed, Engine};
    use domain::science::MoleculeSpec;
    use domain::science::molecule_grid::{ActivityDirection, Descriptors};
    use gpui::{Modifiers, TestAppContext, point, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><rect width="40" height="30" fill="white"/></svg>"#;

    fn molecule(id: &str, mw: f64, logp: f64, activity: Option<f64>) -> GridMolecule {
        GridMolecule {
            id: id.into(),
            molecule: MoleculeSpec {
                smiles: "CCO".into(),
                name: Some(format!("{id} name")),
                svg: Some(SVG.into()),
                formula: Some("C2H6O".into()),
                mw: Some(mw),
                charge: Some(0),
                canonical: None,
                error: None,
                claimed_formula: None,
                claimed_matches: None,
            },
            activity,
            descriptors: Some(Descriptors {
                mw,
                heavy_atoms: 3,
                hbd: 1,
                hba: 1,
                rotatable_bonds: 0,
                rings: 0,
                aromatic_rings: 0,
                logp,
                tpsa: 20.2,
            }),
        }
    }

    fn spec() -> MoleculeGridSpec {
        MoleculeGridSpec {
            title: Some("series A".into()),
            activity_label: Some("IC50".into()),
            activity_unit: Some("nM".into()),
            direction: ActivityDirection::Lower,
            molecules: vec![
                molecule("A1", 300.0, 2.0, Some(50.0)),
                molecule("A2", 620.0, 6.0, Some(5.0)),
                molecule("A3", 150.0, 1.0, None),
            ],
            descriptors: Computed::new(
                "ch_1",
                "chem.descriptors",
                "Molecular descriptors",
                Engine {
                    name: "crest-chem".into(),
                    version: "0.14.0".into(),
                    backend: None,
                    platform: None,
                },
            ),
        }
    }

    #[test]
    fn filters_and_sorts() {
        assert_eq!(
            parse_filter("mw < 500, logp<=3 a1"),
            vec![
                Clause::Compare("mw".into(), Op::Lt, 500.0),
                Clause::Compare("logp".into(), Op::Le, 3.0),
                Clause::Text("a1".into()),
            ]
        );
        let s = spec();
        assert_eq!(
            visible(&s, &parse_filter("mw<500"), false, SortKey::Given, false),
            [0, 2]
        );
        assert_eq!(visible(&s, &[], true, SortKey::Given, false), [0, 2]);
        // Missing activities go last either way.
        assert_eq!(visible(&s, &[], false, SortKey::Activity, false), [1, 0, 2]);
        assert_eq!(visible(&s, &[], false, SortKey::Activity, true), [0, 1, 2]);
        assert_eq!(
            visible(&s, &[], false, SortKey::Descriptor("logp"), true),
            [1, 0, 2]
        );
        assert_eq!(
            visible(&s, &parse_filter("name"), false, SortKey::Given, true),
            [2, 1, 0]
        );
    }

    #[gpui::test]
    fn draws_cards_and_sorts_on_click(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::MoleculeGrid(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        for _ in 0..2 {
            cx.draw(point(px(0.), px(0.)), size(px(1000.), px(1200.)), |_, _| {
                div().size_full().child(view.clone())
            });
            cx.run_until_parked();
        }
        for selector in [
            "molecule-grid",
            "molecule-grid-filter",
            "molecule-grid-count",
            "molecule-grid-cards",
            "molecule-grid-card-0",
            "molecule-grid-card-2",
            "molecule-grid-provenance",
            "molecule-picture",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let button = cx.debug_bounds("molecule-grid-sort-mw").unwrap().center();
        cx.simulate_click(button, Modifiers::none());
        let grid = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        assert_eq!(grid.read_with(cx, |g, _| g.visible()), [2, 0, 1]);
        grid.update(cx, |g, cx| g.set_filter("logp > 1.5", cx));
        assert_eq!(grid.read_with(cx, |g, _| g.visible()), [0, 1]);
    }
}
