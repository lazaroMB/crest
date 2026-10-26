//! Renderer of the `sequence` block kind (`show_sequence`): a DNA, RNA or
//! protein sequence viewer.
//!
//! - A summary line (topology, alphabet, length, GC content, feature and
//!   site counts) and the record's description and organism.
//! - A toolbar: a search box (IUPAC codes, both strands, across the
//!   origin of a circle) with the hit count and previous/next, toggles for
//!   the translation frames and the plasmid map, copy of the selection
//!   (and its reverse complement) and a selection readout (range, length,
//!   GC content).
//! - The plasmid map of a circular DNA ([`map`], an SVG drawn off the UI
//!   thread).
//! - The residues, wrapped in lines of 60 in groups of 10, under a ruler,
//!   with the features as coloured lanes (label, arrow at the reading
//!   end) and, when on, the three forward frames (stops in red,
//!   methionines in green). Rows are drawn by `uniform_list`, so a genome
//!   scrolls as fast as a plasmid. A click on a residue starts a
//!   selection, a shift-click extends it.
//! - The feature list (click to select), the single cutters, CDS problems
//!   and reader notes.

pub mod map;

use std::ops::Range;
use std::rc::Rc;

use base64::Engine as _;
use domain::block::{BlockId, BlockKind, ImageData};
use domain::science::sequence::{
    Alphabet, SeqHit, SequenceSpec, Strand, feature_slot, gc_percent, reverse_complement,
};
use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, FontWeight, HighlightStyle,
    Hsla, ImageSource, InteractiveElement as _, InteractiveText, IntoElement, ObjectFit,
    ParentElement as _, Render, ScrollStrategy, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, StyledText, Subscription, UniformListScrollHandle, Window, div,
    img, prelude::FluentBuilder as _, px, uniform_list,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};

use self::map::{MapColors, plasmid_svg};
use super::svg::{SvgState, hex};
use crate::timeline::TimelineEntry;
use crate::widgets::{palette, readout, register_input, tool_button};

/// Residues per line.
pub const LINE_WIDTH: usize = 60;
/// Residues per group within a line.
const GROUP: usize = 10;
/// Height of one text line of a row, in pixels.
const LINE_HEIGHT: f32 = 17.0;
/// Most feature lanes under a line.
pub const MAX_LANES: usize = 4;
/// Tallest the residue view is, in pixels.
const MAX_HEIGHT: f32 = 460.0;
/// Features listed under the viewer.
const MAX_LISTED: usize = 200;
/// Notes listed.
const NOTES: usize = 6;
/// Width of the map, in pixels.
const MAP_WIDTH: f32 = 520.0;

/// The column of residue `k` of a line: groups are separated by a space.
#[must_use]
pub fn column(k: usize) -> usize {
    k + k / GROUP
}

/// The parts of `start..end` (across the origin when `end <= start` and
/// `wrap`) that fall in `row`, as residue offsets within the row.
#[must_use]
pub fn row_parts(
    start: usize,
    end: usize,
    wrap: bool,
    len: usize,
    row: Range<usize>,
) -> Vec<Range<usize>> {
    let mut pieces: Vec<Range<usize>> = Vec::new();
    if start < end {
        pieces.push(start..end);
    } else if wrap {
        pieces.push(start..len);
        pieces.push(0..end);
    }
    pieces
        .into_iter()
        .filter_map(|p| {
            let a = p.start.max(row.start);
            let b = p.end.min(row.end);
            (a < b).then(|| (a - row.start)..(b - row.start))
        })
        .collect()
}

/// The features in lanes: each lane holds features that do not overlap,
/// first come first served by start; features past [`MAX_LANES`] are
/// left out (their count is returned).
#[must_use]
pub fn lanes(spec: &SequenceSpec) -> (Vec<Vec<usize>>, usize) {
    let len = spec.len();
    let mut order: Vec<usize> = (0..spec.features.len()).collect();
    order.sort_by_key(|&i| spec.features[i].bounds().map_or(0, |b| b.0));
    // Per lane: where the occupied stretch from the left ends, and where
    // an origin-crossing feature's tail begins (len when none).
    let mut lanes: Vec<(Vec<usize>, usize, usize)> = Vec::new();
    let mut left_out = 0;
    // Origin-crossing features first: they take both ends of a lane.
    order.sort_by_key(|&i| {
        let wraps = spec.features[i].bounds().is_some_and(|(s, e)| e <= s);
        !wraps
    });
    for i in order {
        let Some((start, end)) = spec.features[i].bounds() else {
            continue;
        };
        let wraps = end <= start;
        let slot = lanes.iter().position(|(_, last, tail)| {
            if wraps {
                *last <= end && *tail >= len && start >= *last
            } else {
                start >= *last && end <= *tail
            }
        });
        match slot {
            Some(l) => {
                let lane = &mut lanes[l];
                lane.0.push(i);
                if wraps {
                    lane.1 = end;
                    lane.2 = start;
                } else {
                    lane.1 = end;
                }
            }
            None if lanes.len() < MAX_LANES => {
                let (last, tail) = if wraps { (end, start) } else { (end, len) };
                lanes.push((vec![i], last, tail));
            }
            None => left_out += 1,
        }
    }
    (lanes.into_iter().map(|(l, _, _)| l).collect(), left_out)
}

/// The summary line.
#[must_use]
pub fn summary_text(spec: &SequenceSpec) -> String {
    let mut parts = vec![format!(
        "{} {}",
        if spec.is_circular() {
            "circular"
        } else {
            "linear"
        },
        spec.alphabet.name()
    )];
    parts.push(format!("{} {}", spec.len(), spec.alphabet.unit()));
    if let Some(gc) = spec.gc() {
        parts.push(format!("GC {gc:.1}%"));
    }
    parts.push(format!("{} features", spec.features.len()));
    if spec.enzyme_source.is_some() {
        parts.push(format!(
            "{} cut sites ({} single cutters)",
            spec.cut_sites.len(),
            spec.unique_cutters().len()
        ));
    }
    if spec.records > 1 {
        parts.push(format!("record {} of {}", spec.record + 1, spec.records));
    }
    parts.join(" · ")
}

/// The selection readout: `118..213 · 96 bp · GC 52.1%`.
#[must_use]
pub fn selection_text(spec: &SequenceSpec, start: usize, end: usize) -> String {
    let residues = spec.subsequence(start, end);
    let mut text = format!(
        "{}..{} · {} {}",
        start + 1,
        if end == 0 { spec.len() } else { end },
        residues.len(),
        spec.alphabet.unit()
    );
    if spec.alphabet.is_nucleic()
        && let Some(gc) = gc_percent(residues.as_bytes())
    {
        text.push_str(&format!(" · GC {gc:.1}%"));
    }
    text
}

/// How a residue is marked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    None,
    Hit,
    Current,
    Selected,
}

/// Per-block state of the viewer.
pub struct SequenceView {
    spec: Rc<SequenceSpec>,
    input: Entity<InputState>,
    query: String,
    query_error: Option<String>,
    hits: Rc<Vec<SeqHit>>,
    current: usize,
    /// Selected residues `start..end` (across the origin when
    /// `end <= start`).
    selection: Option<(usize, usize)>,
    anchor: Option<usize>,
    frames: bool,
    show_map: bool,
    lanes: Rc<Vec<Vec<usize>>>,
    left_out: usize,
    translations: Rc<[String; 3]>,
    map: Option<(MapColors, ImageData)>,
    scroll: UniformListScrollHandle,
    _input_events: Subscription,
}

impl SequenceView {
    fn new(spec: &SequenceSpec, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("find · IUPAC, both strands"));
        register_input(&input, cx);
        let subscription =
            cx.subscribe(&input, |this: &mut Self, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let text = input.read(cx).value().to_string();
                    this.set_query(&text, cx);
                }
            });
        let (lanes, left_out) = lanes(spec);
        Self {
            spec: Rc::new(spec.clone()),
            input,
            query: String::new(),
            query_error: None,
            hits: Rc::default(),
            current: 0,
            selection: None,
            anchor: None,
            frames: false,
            show_map: spec.is_circular() && spec.alphabet.is_nucleic(),
            lanes: Rc::new(lanes),
            left_out,
            translations: Rc::new(translations(spec)),
            map: None,
            scroll: UniformListScrollHandle::new(),
            _input_events: subscription,
        }
    }

    fn sync(&mut self, spec: &SequenceSpec, cx: &mut Context<Self>) {
        if *self.spec == *spec {
            return;
        }
        let (lanes, left_out) = lanes(spec);
        self.spec = Rc::new(spec.clone());
        self.lanes = Rc::new(lanes);
        self.left_out = left_out;
        self.translations = Rc::new(translations(spec));
        self.selection = None;
        self.anchor = None;
        self.map = None;
        let query = self.query.clone();
        self.set_query(&query, cx);
    }

    /// Searches for `query` (an empty one clears the hits).
    pub fn set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        self.query = query.trim().to_owned();
        self.current = 0;
        if self.query.is_empty() {
            self.hits = Rc::default();
            self.query_error = None;
        } else {
            match self.spec.search(&self.query) {
                Ok(hits) => {
                    self.hits = Rc::new(hits);
                    self.query_error = None;
                    self.reveal_current();
                }
                Err(err) => {
                    self.hits = Rc::default();
                    self.query_error = Some(err.to_string());
                }
            }
        }
        cx.notify();
    }

    /// The hits of the search.
    pub fn hits(&self) -> &[SeqHit] {
        &self.hits
    }

    /// The selected range, if any.
    pub fn selection(&self) -> Option<(usize, usize)> {
        self.selection
    }

    /// Selects `start..end` and scrolls to it.
    pub fn select(&mut self, start: usize, end: usize, cx: &mut Context<Self>) {
        self.selection = Some((start, end));
        self.anchor = Some(start);
        self.scroll
            .scroll_to_item(start / LINE_WIDTH, ScrollStrategy::Center);
        cx.notify();
    }

    /// Clears the selection.
    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selection = None;
        self.anchor = None;
        cx.notify();
    }

    /// A click on residue `at`: a new one-residue selection, or with
    /// `extend` from the anchor to it.
    pub fn click(&mut self, at: usize, extend: bool, cx: &mut Context<Self>) {
        match (extend, self.anchor) {
            (true, Some(anchor)) => {
                let (a, b) = if at >= anchor {
                    (anchor, at + 1)
                } else {
                    (at, anchor + 1)
                };
                self.selection = Some((a, b));
            }
            _ => {
                self.anchor = Some(at);
                self.selection = Some((at, at + 1));
            }
        }
        cx.notify();
    }

    /// Selects feature `ix`.
    pub fn select_feature(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some((start, end)) = self.spec.features.get(ix).and_then(|f| f.bounds()) {
            self.select(start, end, cx);
        }
    }

    /// Moves to the next (or previous) hit and selects it.
    pub fn step_hit(&mut self, forward: bool, cx: &mut Context<Self>) {
        let n = self.hits.len();
        if n == 0 {
            return;
        }
        self.current = if forward {
            (self.current + 1) % n
        } else {
            (self.current + n - 1) % n
        };
        self.reveal_current();
        cx.notify();
    }

    fn reveal_current(&mut self) {
        if let Some(hit) = self.hits.get(self.current) {
            self.selection = Some((hit.start, hit.end));
            self.anchor = Some(hit.start);
            self.scroll
                .scroll_to_item(hit.start / LINE_WIDTH, ScrollStrategy::Center);
        }
    }

    /// The selected residues, as reverse complement when `revcomp`.
    pub fn selected_residues(&self, revcomp: bool) -> Option<String> {
        let (start, end) = self.selection?;
        let residues = self.spec.subsequence(start, end);
        Some(if revcomp && self.spec.alphabet.is_nucleic() {
            reverse_complement(&residues, self.spec.alphabet == Alphabet::Rna)
        } else {
            residues
        })
    }

    /// Whether the frames are drawn.
    pub fn frames(&self) -> bool {
        self.frames
    }

    /// Shows or hides the three forward frames.
    pub fn set_frames(&mut self, on: bool, cx: &mut Context<Self>) {
        self.frames = on && self.spec.alphabet.is_nucleic();
        cx.notify();
    }

    fn rows(&self) -> usize {
        self.spec.len().div_ceil(LINE_WIDTH)
    }

    fn lines_per_row(&self) -> usize {
        2 + self.lanes.len() + if self.frames { 3 } else { 0 }
    }

    fn marks(&self, row: Range<usize>) -> Vec<Mark> {
        let len = self.spec.len();
        let wrap = self.spec.is_circular();
        let mut marks = vec![Mark::None; row.len()];
        let paint = |start: usize, end: usize, mark: Mark, marks: &mut Vec<Mark>| {
            for part in row_parts(start, end, wrap, len, row.clone()) {
                for m in &mut marks[part] {
                    *m = mark;
                }
            }
        };
        // Only the hits that can touch this row.
        for (i, hit) in self.hits.iter().enumerate() {
            let mark = if i == self.current {
                Mark::Current
            } else {
                Mark::Hit
            };
            paint(hit.start, hit.end, mark, &mut marks);
        }
        if let Some((start, end)) = self.selection {
            paint(start, end, Mark::Selected, &mut marks);
        }
        marks
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        range.map(|r| self.row(r, cx)).collect()
    }

    fn row(&self, r: usize, cx: &mut Context<Self>) -> AnyElement {
        let spec = Rc::clone(&self.spec);
        let len = spec.len();
        let start = r * LINE_WIDTH;
        let end = (start + LINE_WIDTH).min(len);
        let row = start..end;
        let theme = cx.theme();
        let (muted, primary, warning, danger, success) = (
            theme.muted_foreground,
            theme.primary,
            theme.warning,
            theme.danger,
            theme.success,
        );
        let mono = theme.mono_font_family.clone();
        let digits = len.to_string().len();
        let gutter = |text: String| {
            div()
                .w(px(digits as f32 * 8.0 + 8.0))
                .flex_none()
                .text_color(muted)
                .child(SharedString::from(text))
        };
        let line = |content: AnyElement, label: String| {
            h_flex()
                .h(px(LINE_HEIGHT))
                .whitespace_nowrap()
                .child(gutter(label))
                .child(content)
                .into_any_element()
        };
        let mut lines: Vec<AnyElement> = Vec::new();
        // Ruler: the position of each group's first residue.
        let mut ruler = String::new();
        for k in (0..row.len()).step_by(GROUP) {
            let label = (start + k + 1).to_string();
            while ruler.chars().count() < column(k) {
                ruler.push(' ');
            }
            ruler.push_str(&label);
        }
        lines.push(line(
            div()
                .text_color(muted)
                .child(SharedString::from(ruler))
                .into_any_element(),
            String::new(),
        ));
        // The residues with the search and selection marks.
        let bytes = &spec.sequence.as_bytes()[row.clone()];
        let mut text = String::with_capacity(column(bytes.len()) + 1);
        let mut offsets = Vec::with_capacity(bytes.len());
        for (k, &b) in bytes.iter().enumerate() {
            if k > 0 && k % GROUP == 0 {
                text.push(' ');
            }
            offsets.push(text.len());
            text.push(char::from(b));
        }
        let marks = self.marks(row.clone());
        let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
        let mut k = 0;
        while k < marks.len() {
            let mark = marks[k];
            let mut j = k;
            while j + 1 < marks.len() && marks[j + 1] == mark {
                j += 1;
            }
            let bg = match mark {
                Mark::None => None,
                Mark::Hit => Some(warning.opacity(0.25)),
                Mark::Current => Some(warning.opacity(0.55)),
                Mark::Selected => Some(primary.opacity(0.3)),
            };
            if let Some(bg) = bg {
                highlights.push((
                    offsets[k]..offsets[j] + 1,
                    HighlightStyle {
                        background_color: Some(bg),
                        ..HighlightStyle::default()
                    },
                ));
            }
            k = j + 1;
        }
        let ranges: Vec<Range<usize>> = offsets.iter().map(|&o| o..o + 1).collect();
        let this = cx.entity().downgrade();
        let residues = InteractiveText::new(
            ("sequence-line", r),
            StyledText::new(text).with_highlights(highlights),
        )
        .on_click(ranges, move |k, window, cx| {
            let extend = window.modifiers().shift;
            this.update(cx, |view, cx| view.click(start + k, extend, cx))
                .ok();
        });
        lines.push(line(
            div()
                .debug_selector(move || format!("sequence-row-{r}"))
                .cursor_text()
                .child(residues)
                .into_any_element(),
            (start + 1).to_string(),
        ));
        // Feature lanes.
        for lane in self.lanes.iter() {
            let mut cells: Vec<Option<(usize, char)>> = vec![None; row.len()];
            for &fi in lane {
                let f = &spec.features[fi];
                let Some((fs, fe)) = f.bounds() else {
                    continue;
                };
                let covered: Vec<Range<usize>> = f
                    .segments
                    .iter()
                    .flat_map(|s| row_parts(s.start, s.end, false, len, row.clone()))
                    .collect();
                if covered.is_empty() {
                    continue;
                }
                let label: Vec<char> = f.name().chars().collect();
                // The label starts where the feature starts in this row.
                let first = covered.iter().map(|c| c.start).min().unwrap_or(0);
                for part in &covered {
                    for k in part.clone() {
                        let at = start + k;
                        let ch = if f.strand == Strand::Forward && at + 1 == fe {
                            '▶'
                        } else if f.strand == Strand::Reverse && at == fs {
                            '◀'
                        } else {
                            label.get(k - first).copied().unwrap_or('━')
                        };
                        cells[k] = Some((fi, ch));
                    }
                }
            }
            let mut text = String::new();
            let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
            for (k, cell) in cells.iter().enumerate() {
                if k > 0 && k % GROUP == 0 {
                    // The group gap continues a feature that spans it.
                    let joined = matches!((cells[k - 1], cell), (Some((a, _)), Some((b, _))) if a == *b);
                    let at = text.len();
                    text.push(if joined { '━' } else { ' ' });
                    if let (true, Some((fi, _))) = (joined, cell) {
                        let color = palette(feature_slot(&spec.features[*fi].kind), cx);
                        highlights.push((at..text.len(), color_style(color)));
                    }
                }
                let at = text.len();
                match cell {
                    Some((fi, ch)) => {
                        text.push(*ch);
                        let color = palette(feature_slot(&spec.features[*fi].kind), cx);
                        highlights.push((at..text.len(), color_style(color)));
                    }
                    None => text.push(' '),
                }
            }
            lines.push(line(
                div()
                    .child(StyledText::new(text).with_highlights(merge(highlights)))
                    .into_any_element(),
                String::new(),
            ));
        }
        // Forward frames: each amino acid under the middle of its codon.
        if self.frames {
            for (f, protein) in self.translations.iter().enumerate() {
                let protein = protein.as_bytes();
                let mut text = String::new();
                let mut highlights = Vec::new();
                for k in 0..row.len() {
                    if k > 0 && k % GROUP == 0 {
                        text.push(' ');
                    }
                    let at = start + k;
                    let aa = (at > f && (at - 1 - f).is_multiple_of(3))
                        .then(|| protein.get((at - 1 - f) / 3).copied())
                        .flatten();
                    let offset = text.len();
                    match aa {
                        Some(a) => {
                            text.push(char::from(a));
                            let color = match a {
                                b'*' => Some(danger),
                                b'M' => Some(success),
                                _ => None,
                            };
                            if let Some(color) = color {
                                highlights.push((offset..offset + 1, color_style(color)));
                            }
                        }
                        None => text.push(' '),
                    }
                }
                lines.push(line(
                    div()
                        .text_color(muted)
                        .child(StyledText::new(text).with_highlights(highlights))
                        .into_any_element(),
                    format!("+{}", f + 1),
                ));
            }
        }
        v_flex()
            .id(("sequence-row", r))
            .font_family(mono)
            .text_xs()
            .children(lines)
            .into_any_element()
    }

    fn map_image(&mut self, cx: &App) -> ImageData {
        let colors = MapColors {
            foreground: hex(cx.theme().foreground),
            muted: hex(cx.theme().muted_foreground),
            palette: (0..5).map(|i| hex(palette(i, cx))).collect(),
        };
        if let Some((drawn, data)) = &self.map
            && *drawn == colors
        {
            return data.clone();
        }
        let svg = plasmid_svg(&self.spec, &colors);
        let data = ImageData::Base64 {
            data: base64::engine::general_purpose::STANDARD.encode(svg),
        };
        self.map = Some((colors, data.clone()));
        data
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        let nucleic = self.spec.alphabet.is_nucleic();
        let circular = self.spec.is_circular();
        let count = match (&self.query_error, self.hits.len()) {
            (Some(_), _) => None,
            (None, 0) if self.query.is_empty() => None,
            (None, 0) => Some("no hits".to_owned()),
            (None, n) => Some(format!("{}/{n}", self.current + 1)),
        };
        h_flex()
            .gap_2()
            .flex_wrap()
            .text_xs()
            .child(
                div()
                    .w(px(240.))
                    .debug_selector(|| "sequence-search".to_owned())
                    .child(Input::new(&self.input).small()),
            )
            .when_some(count, |row, count| {
                row.child(
                    readout(count, cx).debug_selector(|| "sequence-hits".to_owned()),
                )
                .child(
                    tool_button("sequence-prev", "◀", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.step_hit(false, cx))),
                )
                .child(
                    tool_button("sequence-next", "▶", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.step_hit(true, cx))),
                )
            })
            .when_some(self.query_error.clone(), |row, error| {
                row.child(div().text_color(danger).child(SharedString::from(error)))
            })
            .when(nucleic, |row| {
                row.child(
                    tool_button(
                        "sequence-frames",
                        if self.frames { "frames: on" } else { "frames: off" },
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        let on = !this.frames;
                        this.set_frames(on, cx);
                    })),
                )
            })
            .when(nucleic && circular, |row| {
                row.child(
                    tool_button(
                        "sequence-map-toggle",
                        if self.show_map { "map: on" } else { "map: off" },
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_map = !this.show_map;
                        cx.notify();
                    })),
                )
            })
            .when_some(self.selection, |row, (start, end)| {
                row.child(
                    div()
                        .text_color(muted)
                        .debug_selector(|| "sequence-selection".to_owned())
                        .child(SharedString::from(selection_text(&self.spec, start, end))),
                )
                .child(
                    tool_button("sequence-copy", "copy", cx).on_click(cx.listener(
                        |this, _, _, cx| {
                            if let Some(text) = this.selected_residues(false) {
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                            }
                        },
                    )),
                )
                .when(nucleic, |row| {
                    row.child(
                        tool_button("sequence-copy-revcomp", "copy rev-comp", cx).on_click(
                            cx.listener(|this, _, _, cx| {
                                if let Some(text) = this.selected_residues(true) {
                                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                                }
                            }),
                        ),
                    )
                })
                .child(
                    tool_button("sequence-clear", "clear", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.clear_selection(cx))),
                )
            })
    }

    fn feature_list(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let unit = self.spec.alphabet.unit();
        let rows: Vec<AnyElement> = self
            .spec
            .features
            .iter()
            .enumerate()
            .take(MAX_LISTED)
            .map(|(i, f)| {
                let color = palette(feature_slot(&f.kind), cx);
                h_flex()
                    .id(("sequence-feature", i))
                    .debug_selector(move || format!("sequence-feature-{i}"))
                    .gap_2()
                    .px_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.select_feature(i, cx);
                    }))
                    .child(div().size(px(9.)).rounded_sm().bg(color))
                    .child(div().text_color(muted).child(f.kind.clone()))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(SharedString::from(f.name().to_owned())),
                    )
                    .child(
                        div()
                            .text_color(muted)
                            .child(format!("{} · {} {unit}", f.location_text(), f.len())),
                    )
                    .into_any_element()
            })
            .collect();
        let more = self.spec.features.len().saturating_sub(MAX_LISTED);
        v_flex()
            .debug_selector(|| "sequence-features".to_owned())
            .text_xs()
            .border_t_1()
            .border_color(border)
            .pt_1()
            .children(rows)
            .when(more > 0, |d| {
                d.child(div().text_color(muted).child(format!("… {more} more features")))
            })
    }
}

/// A text colour as a highlight.
fn color_style(color: Hsla) -> HighlightStyle {
    HighlightStyle {
        color: Some(color),
        ..HighlightStyle::default()
    }
}

/// Adjacent highlight runs of the same style joined.
fn merge(runs: Vec<(Range<usize>, HighlightStyle)>) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut out: Vec<(Range<usize>, HighlightStyle)> = Vec::with_capacity(runs.len());
    for (range, style) in runs {
        match out.last_mut() {
            Some((last, s)) if last.end == range.start && *s == style => last.end = range.end,
            _ => out.push((range, style)),
        }
    }
    out
}

/// The three forward frames.
fn translations(spec: &SequenceSpec) -> [String; 3] {
    [1, 2, 3].map(|f| spec.frame_translation(f))
}

impl Render for SequenceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let spec = Rc::clone(&self.spec);
        let rows = self.rows();
        let row_height = self.lines_per_row() as f32 * LINE_HEIGHT;
        let height = (rows as f32 * row_height).clamp(row_height, MAX_HEIGHT);
        let map = (self.show_map && spec.is_circular() && spec.alphabet.is_nucleic()).then(|| {
            let data = self.map_image(cx);
            match super::svg::use_svg(&data, 1.0, window, cx) {
                SvgState::Ready(raster) => {
                    let (w, h) = raster.size;
                    img(ImageSource::Render(raster.image.clone()))
                        .debug_selector(|| "sequence-map".to_owned())
                        .w(px(MAP_WIDTH))
                        .max_w_full()
                        .aspect_ratio(w / h)
                        .object_fit(ObjectFit::Contain)
                        .into_any_element()
                }
                SvgState::Failed(err) => div()
                    .text_color(cx.theme().danger)
                    .child(SharedString::from(err))
                    .into_any_element(),
                SvgState::Loading => div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("drawing the map…")
                    .into_any_element(),
            }
        });
        let toolbar = self.toolbar(cx);
        let features = self.feature_list(cx);
        let list = uniform_list(
            "sequence-rows",
            rows,
            cx.processor(|this, range, _, cx| this.render_rows(range, cx)),
        )
        .track_scroll(&self.scroll)
        .size_full();
        let theme = cx.theme();
        let (muted, warning, border, radius) =
            (theme.muted_foreground, theme.warning, theme.border, theme.radius);
        let cutters = spec.unique_cutters();
        let cutters_text = (!cutters.is_empty()).then(|| {
            let listed: Vec<String> = cutters
                .iter()
                .take(40)
                .map(|e| {
                    let cut = spec
                        .cut_sites
                        .iter()
                        .find(|c| c.enzyme == *e)
                        .map_or(0, |c| c.cut);
                    format!("{e} ({cut})")
                })
                .collect();
            format!("Single cutters: {}", listed.join(", "))
        });
        let issues = spec.cds_issues();
        let mut notes: Vec<String> = spec.warnings.clone();
        if self.left_out > 0 {
            notes.push(format!(
                "{} features are not drawn in the lanes (more than {MAX_LANES} overlap)",
                self.left_out
            ));
        }
        let header = [
            spec.description.clone(),
            spec.organism.clone(),
            spec.accession.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "sequence".to_owned())
            // A press in the viewer is not a click on the timeline row.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(spec.display_title())),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .debug_selector(|| "sequence-summary".to_owned())
                    .child(SharedString::from(summary_text(&spec))),
            )
            .when(!header.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(SharedString::from(header)),
                )
            })
            .child(toolbar)
            .children(map.map(|m| h_flex().justify_center().child(m)))
            .child(
                div()
                    .id("sequence-body")
                    .debug_selector(|| "sequence-body".to_owned())
                    .w_full()
                    .h(px(height + 4.0))
                    .px_1()
                    .rounded(radius)
                    .border_1()
                    .border_color(border)
                    .child(list),
            )
            .when(!spec.features.is_empty(), |d| d.child(features))
            .when_some(cutters_text, |d, text| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .debug_selector(|| "sequence-cutters".to_owned())
                        .child(SharedString::from(text)),
                )
            })
            .when_some(spec.enzyme_source.clone(), |d, source| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(SharedString::from(format!("Enzyme table: {source}"))),
                )
            })
            .when(!issues.is_empty(), |d| {
                d.child(
                    v_flex()
                        .text_xs()
                        .text_color(warning)
                        .debug_selector(|| "sequence-issues".to_owned())
                        .children(issues.into_iter().take(NOTES).map(|i| div().child(i))),
                )
            })
            .when(!notes.is_empty(), |d| {
                d.child(
                    v_flex()
                        .text_xs()
                        .text_color(muted)
                        .children(notes.into_iter().take(NOTES).map(|n| div().child(n))),
                )
            })
    }
}

/// The viewer entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &SequenceSpec,
    window: &mut Window,
    cx: &mut App,
) -> Entity<SequenceView> {
    let view = super::block_state(id, window, cx, |window, cx| {
        SequenceView::new(spec, window, cx)
    });
    view.update(cx, |view, cx| view.sync(spec, cx));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Sequence(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::science::sequence::{CutSite, SeqFeature, SeqSpan, SequenceFormat, Topology};
    use gpui::{Modifiers, TestAppContext, point, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> SequenceSpec {
        SequenceSpec {
            title: None,
            path: Some("dna/p.gb".into()),
            format: SequenceFormat::Genbank,
            name: Some("pTest".into()),
            description: Some("a test plasmid".into()),
            accession: None,
            organism: None,
            alphabet: Alphabet::Dna,
            topology: Topology::Circular,
            sequence: format!("ATGAAATAAGAATTC{}", "ACGT".repeat(30)),
            features: vec![
                SeqFeature {
                    kind: "CDS".into(),
                    label: Some("orf".into()),
                    segments: vec![SeqSpan { start: 0, end: 9 }],
                    strand: Strand::Forward,
                    qualifiers: Vec::new(),
                },
                SeqFeature {
                    kind: "misc_feature".into(),
                    label: Some("wrap".into()),
                    segments: vec![SeqSpan { start: 130, end: 135 }, SeqSpan { start: 0, end: 4 }],
                    strand: Strand::Reverse,
                    qualifiers: Vec::new(),
                },
            ],
            cut_sites: vec![CutSite {
                enzyme: "EcoRI".into(),
                site: "GAATTC".into(),
                start: 9,
                cut: 10,
                strand: Strand::Forward,
            }],
            enzyme_source: Some("REBASE".into()),
            records: 1,
            record: 0,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn rows_lanes_and_texts() {
        assert_eq!(column(9), 9);
        assert_eq!(column(10), 11);
        assert_eq!(row_parts(5, 70, false, 135, 60..120), vec![0..10]);
        assert_eq!(row_parts(130, 4, true, 135, 120..135), vec![10..15]);
        assert_eq!(row_parts(130, 4, true, 135, 0..60), vec![0..4]);
        let (lanes, left_out) = lanes(&spec());
        // The wrapping feature and the CDS overlap at 0..4: two lanes.
        assert_eq!((lanes.len(), left_out), (2, 0));
        assert_eq!(
            summary_text(&spec()),
            "circular DNA · 135 bp · GC 46.7% · 2 features · 1 cut sites (1 single cutters)"
        );
        assert_eq!(selection_text(&spec(), 0, 9), "1..9 · 9 bp · GC 11.1%");
        assert_eq!(selection_text(&spec(), 130, 4).split(" · ").nth(1), Some("9 bp"));
    }

    #[gpui::test]
    fn draws_the_viewer_searches_and_selects(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Sequence(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(point(px(0.), px(0.)), size(px(1000.), px(1400.)), |_, _| {
                div().size_full().child(view.clone())
            });
            cx.run_until_parked();
        };
        draw(cx);
        draw(cx);
        for selector in [
            "sequence",
            "sequence-summary",
            "sequence-search",
            "sequence-map",
            "sequence-body",
            "sequence-row-0",
            "sequence-features",
            "sequence-cutters",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let seq = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        seq.update(cx, |v, cx| v.set_query("GAATTC", cx));
        assert_eq!(seq.read_with(cx, |v, _| v.hits().len()), 1);
        assert_eq!(seq.read_with(cx, |v, _| v.selection()), Some((9, 15)));
        assert_eq!(
            seq.read_with(cx, |v, _| v.selected_residues(true)),
            Some("GAATTC".to_owned())
        );
        let row = cx.debug_bounds("sequence-feature-0").unwrap().center();
        cx.simulate_click(row, Modifiers::none());
        assert_eq!(seq.read_with(cx, |v, _| v.selection()), Some((0, 9)));
        seq.update(cx, |v, cx| {
            v.set_frames(true, cx);
            v.click(3, false, cx);
            v.click(7, true, cx);
        });
        assert_eq!(seq.read_with(cx, |v, _| v.selection()), Some((3, 8)));
        draw(cx);
        assert!(cx.debug_bounds("sequence-selection").is_some());
        seq.update(cx, |v, cx| v.set_query("AZ", cx));
        assert!(seq.read_with(cx, |v, _| v.hits().is_empty()));
    }
}
