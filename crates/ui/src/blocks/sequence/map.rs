//! The circular map of a plasmid as an SVG: the backbone, a ring arc per
//! feature in its kind's colour (with a pointed end for its strand), the
//! single-cutter enzymes as ticks with their names outside, and the name
//! and length in the middle. Labels on each side are pushed apart so they
//! never overlap; a leader line joins each to its tick.

use std::f64::consts::{PI, TAU};
use std::fmt::Write as _;

use domain::science::sequence::{SequenceSpec, Strand};

/// Width and height of the drawing, in SVG units.
pub const WIDTH: f64 = 520.0;
pub const HEIGHT: f64 = 420.0;
const CX: f64 = WIDTH / 2.0;
const CY: f64 = HEIGHT / 2.0;
/// Radius of the backbone.
const R: f64 = 130.0;
/// Width of a feature arc.
const RING: f64 = 12.0;
/// Space between labels on one side.
const LABEL_GAP: f64 = 13.0;
/// Most enzyme labels drawn.
pub const MAX_SITE_LABELS: usize = 24;

/// The colours the map is drawn with, as `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MapColors {
    /// Backbone, ticks and the name.
    pub foreground: String,
    /// Labels and the length.
    pub muted: String,
    /// The feature colour of each palette slot.
    pub palette: Vec<String>,
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The angle of position `pos` of `len`: 0 at the top, clockwise.
fn angle(pos: f64, len: f64) -> f64 {
    pos / len * TAU - PI / 2.0
}

fn at(radius: f64, a: f64) -> (f64, f64) {
    (CX + radius * a.cos(), CY + radius * a.sin())
}

/// An arc of `radius` from `a0` to `a1` (clockwise) as a path.
fn arc_path(radius: f64, a0: f64, a1: f64) -> String {
    let (x0, y0) = at(radius, a0);
    let (x1, y1) = at(radius, a1);
    let large = u8::from(a1 - a0 > PI);
    format!("M{x0:.2},{y0:.2} A{radius},{radius} 0 {large} 1 {x1:.2},{y1:.2}")
}

/// A label to place on the left or right of the circle.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    /// Text.
    pub text: String,
    /// Angle of what it names.
    pub angle: f64,
    /// Where it ends up.
    pub y: f64,
    /// Whether it is on the right side.
    pub right: bool,
}

/// Pushes `labels` of one side apart, top to bottom, at least
/// [`LABEL_GAP`] from each other and inside the drawing.
pub fn spread(labels: &mut [Label]) {
    labels.sort_by(|a, b| a.y.total_cmp(&b.y));
    let mut floor = LABEL_GAP;
    for label in labels.iter_mut() {
        label.y = label.y.max(floor);
        floor = label.y + LABEL_GAP;
    }
    // Pushed past the bottom: shift back up from the end.
    let mut ceiling = HEIGHT - LABEL_GAP / 2.0;
    for label in labels.iter_mut().rev() {
        label.y = label.y.min(ceiling);
        ceiling = label.y - LABEL_GAP;
    }
}

/// The SVG map of `spec` in `colors`.
#[must_use]
pub fn plasmid_svg(spec: &SequenceSpec, colors: &MapColors) -> String {
    let len = spec.len().max(1) as f64;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{WIDTH}\" height=\"{HEIGHT}\" \
         viewBox=\"0 0 {WIDTH} {HEIGHT}\" font-family=\"sans-serif\">"
    );
    let fg = &colors.foreground;
    let muted = &colors.muted;
    let _ = write!(
        svg,
        "<circle cx=\"{CX}\" cy=\"{CY}\" r=\"{R}\" fill=\"none\" stroke=\"{fg}\" stroke-width=\"1.5\"/>"
    );
    // Ticks every tenth of the length.
    for i in 0..10 {
        let a = angle(f64::from(i) * len / 10.0, len);
        let (x0, y0) = at(R - 3.0, a);
        let (x1, y1) = at(R + 3.0, a);
        let _ = write!(
            svg,
            "<line x1=\"{x0:.2}\" y1=\"{y0:.2}\" x2=\"{x1:.2}\" y2=\"{y1:.2}\" stroke=\"{muted}\" stroke-width=\"1\"/>"
        );
    }
    let mut labels: Vec<Label> = Vec::new();
    for f in &spec.features {
        let slot = domain::science::sequence::feature_slot(&f.kind);
        let color = colors
            .palette
            .get(slot % colors.palette.len().max(1))
            .cloned()
            .unwrap_or_else(|| fg.clone());
        for s in &f.segments {
            let (a0, a1) = (angle(s.start as f64, len), angle(s.end as f64, len));
            let _ = write!(
                svg,
                "<path d=\"{}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"{RING}\" stroke-opacity=\"0.85\"/>",
                arc_path(R - RING, a0, a1.max(a0 + 0.004))
            );
        }
        // An arrow head at the reading end.
        if let Some((start, end)) = f.bounds() {
            let tip = match f.strand {
                Strand::Forward => Some((end as f64, -1.0)),
                Strand::Reverse => Some((start as f64, 1.0)),
                Strand::None => None,
            };
            if let Some((pos, back)) = tip {
                let a = angle(pos, len);
                let da = back * 0.06_f64.min(f.len() as f64 / len * TAU * 0.5);
                let (tx, ty) = at(R - RING, a);
                let (ox, oy) = at(R - RING / 2.0 + 4.0, a + da);
                let (ix, iy) = at(R - RING * 1.5 - 4.0, a + da);
                let _ = write!(
                    svg,
                    "<path d=\"M{ox:.2},{oy:.2} L{tx:.2},{ty:.2} L{ix:.2},{iy:.2} Z\" fill=\"{color}\"/>"
                );
            }
            // The label inside the ring, at the middle of the feature.
            let mid = if end > start {
                (start + end) as f64 / 2.0
            } else {
                ((start as f64 + end as f64 + len) / 2.0) % len
            };
            let a = angle(mid, len);
            let (x, y) = at(R - RING * 2.0 - 10.0, a);
            let anchor = if a.cos() > 0.3 {
                "end"
            } else if a.cos() < -0.3 {
                "start"
            } else {
                "middle"
            };
            if f.len() as f64 / len > 0.02 {
                let _ = write!(
                    svg,
                    "<text x=\"{x:.2}\" y=\"{:.2}\" font-size=\"10\" text-anchor=\"{anchor}\" fill=\"{color}\">{}</text>",
                    y + 3.5,
                    escape(&short(f.name(), 18))
                );
            }
        }
    }
    // Single cutters outside, with leader lines.
    let unique = spec.unique_cutters();
    for site in spec
        .cut_sites
        .iter()
        .filter(|c| unique.contains(&c.enzyme.as_str()))
        .take(MAX_SITE_LABELS)
    {
        let a = angle(site.cut as f64, len);
        let (x0, y0) = at(R, a);
        let (x1, y1) = at(R + 8.0, a);
        let _ = write!(
            svg,
            "<line x1=\"{x0:.2}\" y1=\"{y0:.2}\" x2=\"{x1:.2}\" y2=\"{y1:.2}\" stroke=\"{fg}\" stroke-width=\"1\"/>"
        );
        labels.push(Label {
            text: format!("{} ({})", site.enzyme, site.cut),
            angle: a,
            y: at(R + 24.0, a).1,
            right: a.cos() >= 0.0,
        });
    }
    let (mut left, mut right): (Vec<Label>, Vec<Label>) = labels.into_iter().partition(|l| !l.right);
    spread(&mut left);
    spread(&mut right);
    for label in left.iter().chain(right.iter()) {
        let (tx, ty) = at(R + 8.0, label.angle);
        let x = if label.right {
            CX + R + 40.0
        } else {
            CX - R - 40.0
        };
        let anchor = if label.right { "start" } else { "end" };
        let edge = if label.right { x - 3.0 } else { x + 3.0 };
        let _ = write!(
            svg,
            "<polyline points=\"{tx:.2},{ty:.2} {edge:.2},{:.2}\" fill=\"none\" stroke=\"{muted}\" stroke-width=\"0.7\"/>\
             <text x=\"{x:.2}\" y=\"{:.2}\" font-size=\"10\" text-anchor=\"{anchor}\" fill=\"{fg}\">{}</text>",
            label.y,
            label.y + 3.5,
            escape(&label.text)
        );
    }
    let name = spec.name.clone().unwrap_or_else(|| spec.display_title());
    let _ = write!(
        svg,
        "<text x=\"{CX}\" y=\"{}\" font-size=\"14\" font-weight=\"bold\" text-anchor=\"middle\" fill=\"{fg}\">{}</text>\
         <text x=\"{CX}\" y=\"{}\" font-size=\"11\" text-anchor=\"middle\" fill=\"{muted}\">{} {}</text>",
        CY - 2.0,
        escape(&short(&name, 24)),
        CY + 14.0,
        spec.len(),
        spec.alphabet.unit()
    );
    svg.push_str("</svg>");
    svg
}

/// `text` cut to `max` characters.
fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let cut: String = text.chars().take(max - 1).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use domain::science::sequence::{
        Alphabet, CutSite, SeqFeature, SeqSpan, SequenceFormat, Topology,
    };

    use super::*;

    fn spec() -> SequenceSpec {
        SequenceSpec {
            title: None,
            path: None,
            format: SequenceFormat::Genbank,
            name: Some("p<1>".into()),
            description: None,
            accession: None,
            organism: None,
            alphabet: Alphabet::Dna,
            topology: Topology::Circular,
            sequence: "A".repeat(1000),
            features: vec![SeqFeature {
                kind: "CDS".into(),
                label: Some("bla".into()),
                segments: vec![SeqSpan { start: 900, end: 1000 }, SeqSpan { start: 0, end: 100 }],
                strand: Strand::Forward,
                qualifiers: Vec::new(),
            }],
            cut_sites: (0..5)
                .map(|i| CutSite {
                    enzyme: format!("E{i}"),
                    site: "GAATTC".into(),
                    start: 10 + i,
                    cut: 11 + i,
                    strand: Strand::Forward,
                })
                .collect(),
            enzyme_source: None,
            records: 1,
            record: 0,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn the_map_names_features_sites_and_escapes() {
        let colors = MapColors {
            foreground: "#111111".into(),
            muted: "#777777".into(),
            palette: vec!["#ff0000".into()],
        };
        let svg = plasmid_svg(&spec(), &colors);
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(">bla</text>"));
        assert!(svg.contains("p&lt;1&gt;"));
        assert!(svg.contains(">1000 bp</text>"));
        for i in 0..5 {
            assert!(svg.contains(&format!(">E{i} ({})</text>", 11 + i)), "E{i}");
        }
        // The origin-crossing CDS is two arcs.
        assert_eq!(svg.matches("stroke=\"#ff0000\"").count(), 2);
        assert!(crate::blocks::svg::rasterize(svg.as_bytes(), 1.0).is_ok());
    }

    #[test]
    fn labels_never_overlap() {
        let mut labels: Vec<Label> = (0..5)
            .map(|i| Label {
                text: String::new(),
                angle: 0.0,
                y: 100.0 + f64::from(i),
                right: true,
            })
            .collect();
        spread(&mut labels);
        for pair in labels.windows(2) {
            assert!(pair[1].y - pair[0].y >= LABEL_GAP - 1e-9);
        }
        let mut low: Vec<Label> = (0..3)
            .map(|_| Label {
                text: String::new(),
                angle: 0.0,
                y: HEIGHT,
                right: false,
            })
            .collect();
        spread(&mut low);
        assert!(low.iter().all(|l| l.y <= HEIGHT));
    }
}
