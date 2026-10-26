//! Renderer of the `ghs` block kind (`show_ghs`): a GHS hazard card from
//! PubChem.
//!
//! The compound and its PubChem id come first, with the PubChem page (a
//! click opens it) and the retrieval date. Each source's classification
//! follows: the source and its licence note, the signal word on a pill
//! (danger red, warning amber), the pictograms as red-bordered diamonds
//! with their code (drawn here; no GHS artwork is bundled) and the
//! source's label, the hazard statements as written and the
//! precautionary statement codes. A compound PubChem does not know, or
//! one without a classification, is a warning that says so: the card
//! shows nothing it did not fetch.

use domain::block::{BlockKind, ImageData};
use domain::science::ghs::{GhsClassification, GhsSpec};
use gpui::{
    AnyElement, App, FontWeight, Hsla, ImageSource, InteractiveElement as _, IntoElement as _,
    ObjectFit, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, div, img, prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::molecule::svg_data;
use super::svg::SvgState;
use crate::timeline::TimelineEntry;

/// Size a pictogram is drawn at, in pixels.
const PICTOGRAM: f32 = 52.0;

/// The diamond drawn for pictogram `code`: a red-bordered white square on
/// its corner with the code inside.
#[must_use]
pub fn pictogram_svg(code: &str) -> String {
    let number: String = code
        .trim_start_matches("GHS")
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\" viewBox=\"0 0 64 64\">\
         <polygon points=\"32,4 60,32 32,60 4,32\" fill=\"#ffffff\" stroke=\"#e00000\" stroke-width=\"5\"/>\
         <text x=\"32\" y=\"30\" font-family=\"sans-serif\" font-size=\"9\" text-anchor=\"middle\" fill=\"#000000\">GHS</text>\
         <text x=\"32\" y=\"44\" font-family=\"sans-serif\" font-size=\"15\" font-weight=\"bold\" text-anchor=\"middle\" fill=\"#000000\">{number}</text>\
         </svg>"
    )
}

/// The colour of a signal word.
fn signal_color(signal: &str, cx: &App) -> Hsla {
    if signal.eq_ignore_ascii_case("danger") {
        cx.theme().danger
    } else {
        cx.theme().warning
    }
}

fn pictogram(
    data: &ImageData,
    code: &str,
    label: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let picture = match super::svg::use_svg(data, 1.0, window, cx) {
        SvgState::Ready(raster) => img(ImageSource::Render(raster.image.clone()))
            .size(px(PICTOGRAM))
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        SvgState::Failed(_) | SvgState::Loading => div()
            .size(px(PICTOGRAM))
            .child(SharedString::from(code.to_owned()))
            .into_any_element(),
    };
    let theme = cx.theme();
    v_flex()
        .items_center()
        .w(px(PICTOGRAM + 30.0))
        .child(picture)
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(label.unwrap_or(code).to_owned())),
        )
        .into_any_element()
}

fn classification(
    ix: usize,
    c: &GhsClassification,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let pictograms: Vec<AnyElement> = c
        .pictograms
        .iter()
        .map(|p| {
            let data = svg_data(&pictogram_svg(&p.code));
            pictogram(
                &data,
                &p.code,
                p.label.as_deref().or(p.symbol_name()),
                window,
                cx,
            )
        })
        .collect();
    let theme = cx.theme();
    let (muted, border) = (theme.muted_foreground, theme.border);
    let signal = c.signal.clone().map(|s| {
        let color = signal_color(&s, cx);
        div()
            .px_1p5()
            .rounded_sm()
            .border_1()
            .border_color(color)
            .text_color(color)
            .font_weight(FontWeight::BOLD)
            .child(SharedString::from(s))
    });
    let url = c.url.clone();
    v_flex()
        .debug_selector(move || format!("ghs-source-{ix}"))
        .gap_1()
        .pt_1()
        .border_t_1()
        .border_color(border)
        .child(
            h_flex().gap_2().flex_wrap().children(signal).child(
                div()
                    .id(("ghs-source-link", ix))
                    .text_xs()
                    .text_color(muted)
                    .when(url.is_some(), |d| d.cursor_pointer().underline())
                    .on_click(move |_, _, cx| {
                        if let Some(url) = &url {
                            cx.open_url(url);
                        }
                    })
                    .child(SharedString::from(match &c.substance {
                        Some(s) => format!("{} · {s}", c.source),
                        None => c.source.clone(),
                    })),
            ),
        )
        .when(!pictograms.is_empty(), |d| {
            d.child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .debug_selector(move || format!("ghs-pictograms-{ix}"))
                    .children(pictograms),
            )
        })
        .children(
            c.hazards
                .iter()
                .map(|h| div().text_sm().child(SharedString::from(h.text.clone()))),
        )
        .when(!c.precautionary.is_empty(), |d| {
            d.child(div().text_xs().child(SharedString::from(format!(
                "Precautionary statements: {}",
                c.precautionary.join(", ")
            ))))
        })
        .children(c.notes.iter().map(|n| {
            div()
                .text_xs()
                .text_color(muted)
                .child(SharedString::from(n.clone()))
        }))
        .when_some(c.license.clone(), |d, l| {
            d.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(l)),
            )
        })
        .into_any_element()
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Ghs(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    render_spec(spec, window, cx)
}

/// Draws `spec`.
pub fn render_spec(spec: &GhsSpec, window: &mut Window, cx: &mut App) -> AnyElement {
    let sources: Vec<AnyElement> = spec
        .classifications
        .iter()
        .enumerate()
        .map(|(i, c)| classification(i, c, window, cx))
        .collect();
    let theme = cx.theme();
    let (muted, warning) = (theme.muted_foreground, theme.warning);
    let title = match (&spec.name, spec.cid) {
        (Some(n), Some(c)) => format!("{n} · PubChem CID {c}"),
        (None, Some(c)) => format!("PubChem CID {c}"),
        _ => spec.query.clone(),
    };
    let url = spec.source_url.clone();
    v_flex()
        .w_full()
        .gap_1()
        .debug_selector(|| "ghs".to_owned())
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(SharedString::from(format!("GHS hazards · {title}"))),
        )
        .child(
            div()
                .id("ghs-pubchem")
                .debug_selector(|| "ghs-provenance".to_owned())
                .text_xs()
                .text_color(muted)
                .cursor_pointer()
                .on_click(move |_, _, cx| cx.open_url(&url))
                .child(SharedString::from(format!(
                    "From PubChem: {} · retrieved {}",
                    spec.source_url, spec.retrieved
                ))),
        )
        .when_some(spec.message.clone(), |d, m| {
            d.child(
                div()
                    .text_sm()
                    .text_color(warning)
                    .debug_selector(|| "ghs-message".to_owned())
                    .child(SharedString::from(format!(
                        "{m} No hazard data is shown: it could not be verified here."
                    ))),
            )
        })
        .children(sources)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::science::ghs::{HazardStatement, Pictogram};
    use gpui::{TestAppContext, point, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> GhsSpec {
        GhsSpec {
            query: "acetone".into(),
            cid: Some(180),
            name: Some("Acetone".into()),
            classifications: vec![GhsClassification {
                source: "Regulation (EC) No 1272/2008".into(),
                substance: Some("acetone".into()),
                url: Some("https://eur-lex.europa.eu/eli/reg/2008/1272/oj".into()),
                license: Some("CC BY 4.0".into()),
                pictograms: vec![Pictogram {
                    code: "GHS02".into(),
                    label: Some("Flammable".into()),
                }],
                signal: Some("Danger".into()),
                hazards: vec![HazardStatement {
                    code: "H225".into(),
                    text: "H225: Highly Flammable liquid and vapor".into(),
                    share: None,
                }],
                precautionary: vec!["P210".into()],
                notes: Vec::new(),
            }],
            source_url: "https://pubchem.ncbi.nlm.nih.gov/compound/180#section=GHS-Classification"
                .into(),
            fetched_url: "https://pubchem.ncbi.nlm.nih.gov/rest/pug_view/data/compound/180/JSON"
                .into(),
            retrieved: "2026-10-25".into(),
            message: None,
        }
    }

    #[test]
    fn pictograms_are_drawn_here() {
        let svg = pictogram_svg("GHS07");
        assert!(svg.contains(">07</text>") && svg.contains("#e00000"));
        assert!(crate::blocks::svg::rasterize(svg.as_bytes(), 1.0).is_ok());
    }

    #[gpui::test]
    fn draws_the_card_and_the_not_found_one(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let missing = GhsSpec {
            cid: None,
            name: None,
            classifications: Vec::new(),
            message: Some("PubChem has no compound named `x`.".into()),
            ..spec()
        };
        view.update(cx, |view, cx| {
            view.apply_all(
                [
                    DomainEvent::BlockAdded {
                        block: Block::new(BlockKind::Ghs(Box::new(spec()))),
                    },
                    DomainEvent::BlockAdded {
                        block: Block::new(BlockKind::Ghs(Box::new(missing))),
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
            "ghs",
            "ghs-provenance",
            "ghs-source-0",
            "ghs-pictograms-0",
            "ghs-message",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
    }
}
