//! Renderer of the `findings` block kind (`show_findings`): a SARIF log
//! grouped by file, then severity.
//!
//! - The header: the tool and one chip per severity with its count, in
//!   the theme's error, warning and info colors; a click shows only that
//!   severity, a second click shows all again.
//! - One group per file, the most severe files first, with its counts; a
//!   click on the path opens the file (see [`super::sources::action`]),
//!   a click on the group's arrow folds it.
//! - Each finding: severity, line (a link to the file at that line), rule
//!   id, message, the source snippet in the monospace font and a `docs`
//!   link to the rule's help page (http and https only).
//!
//! The view is an entity per block ([`super::block_state`]). At most
//! [`MAX_ROWS`] findings are drawn at once.

use std::collections::{BTreeMap, BTreeSet};

use domain::block::{BlockId, BlockKind, SourceRef};
use domain::report::{Finding, FindingSeverity, FindingsSpec};
use gpui::{
    AnyElement, App, Context, ElementId, Entity, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::sources::{BlockEvent, action, activate};
use crate::timeline::TimelineEntry;

/// Most findings drawn at once; the rest are counted.
pub const MAX_ROWS: usize = 300;

/// Group label of findings without a file.
pub const NO_FILE: &str = "(no location)";

/// The severities, most severe first.
pub const SEVERITIES: [FindingSeverity; 4] = [
    FindingSeverity::Error,
    FindingSeverity::Warning,
    FindingSeverity::Note,
    FindingSeverity::None,
];

/// Lower-case name of a severity.
#[must_use]
pub fn severity_name(severity: FindingSeverity) -> &'static str {
    match severity {
        FindingSeverity::Error => "error",
        FindingSeverity::Warning => "warning",
        FindingSeverity::Note => "note",
        FindingSeverity::None => "none",
    }
}

fn color(severity: FindingSeverity, cx: &App) -> Hsla {
    let theme = cx.theme();
    match severity {
        FindingSeverity::Error => theme.danger,
        FindingSeverity::Warning => theme.warning,
        FindingSeverity::Note => theme.info,
        FindingSeverity::None => theme.muted_foreground,
    }
}

/// The file of a finding, or [`NO_FILE`].
fn file_of(finding: &Finding) -> &str {
    match &finding.location {
        Some(SourceRef::File { path, .. }) => path,
        _ => NO_FILE,
    }
}

fn line_of(finding: &Finding) -> Option<u32> {
    match &finding.location {
        Some(SourceRef::File { line_start, .. }) => *line_start,
        _ => None,
    }
}

/// One file's findings, by index into the spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileGroup {
    /// The path, or [`NO_FILE`].
    pub path: String,
    /// Indexes of its findings, by severity then line.
    pub findings: Vec<usize>,
    /// Count per severity, in [`SEVERITIES`] order.
    pub counts: [usize; 4],
}

/// The findings grouped by file: files with errors first (then by
/// warnings, notes, path), findings by severity then line.
#[must_use]
pub fn groups(spec: &FindingsSpec) -> Vec<FileGroup> {
    let mut by_file: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (ix, finding) in spec.findings.iter().enumerate() {
        by_file.entry(file_of(finding)).or_default().push(ix);
    }
    let mut groups: Vec<FileGroup> = by_file
        .into_iter()
        .map(|(path, mut findings)| {
            findings.sort_by_key(|&ix| {
                let f = &spec.findings[ix];
                (f.severity, line_of(f).unwrap_or(u32::MAX))
            });
            let mut counts = [0; 4];
            for &ix in &findings {
                let rank = SEVERITIES
                    .iter()
                    .position(|s| *s == spec.findings[ix].severity)
                    .unwrap_or(3);
                counts[rank] += 1;
            }
            FileGroup {
                path: path.to_owned(),
                findings,
                counts,
            }
        })
        .collect();
    groups.sort_by(|a, b| {
        b.counts
            .cmp(&a.counts)
            .then_with(|| (a.path == NO_FILE).cmp(&(b.path == NO_FILE)))
            .then_with(|| a.path.cmp(&b.path))
    });
    groups
}

/// Whether `url` may be opened in the browser.
fn is_web(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

/// Per-block state of the findings.
pub struct FindingsView {
    id: BlockId,
    spec: FindingsSpec,
    /// Only this severity is shown, when set.
    filter: Option<FindingSeverity>,
    /// Folded files.
    folded: BTreeSet<String>,
}

impl FindingsView {
    fn new(id: BlockId, spec: &FindingsSpec) -> Self {
        Self {
            id,
            spec: spec.clone(),
            filter: None,
            folded: BTreeSet::new(),
        }
    }

    /// The severity filter.
    pub fn filter(&self) -> Option<FindingSeverity> {
        self.filter
    }

    /// Shows only `severity`, or everything again when it was the filter.
    pub fn toggle_filter(&mut self, severity: FindingSeverity, cx: &mut Context<Self>) {
        self.filter = (self.filter != Some(severity)).then_some(severity);
        cx.notify();
    }

    /// Folds or unfolds the group of `path`.
    pub fn toggle_file(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.folded.remove(path) {
            self.folded.insert(path.to_owned());
        }
        cx.notify();
    }

    fn sync(&mut self, spec: &FindingsSpec, cx: &mut Context<Self>) {
        if &self.spec != spec {
            self.spec = spec.clone();
            cx.notify();
        }
    }

    fn element_id(&self, what: &str) -> ElementId {
        ElementId::Name(SharedString::from(format!("findings-{what}-{}", self.id)))
    }

    fn chip(&self, severity: FindingSeverity, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let tint = color(severity, cx);
        let active = self.filter == Some(severity);
        let name = severity_name(severity);
        let label = if count == 1 || severity == FindingSeverity::None {
            format!("{count} {name}")
        } else {
            format!("{count} {name}s")
        };
        div()
            .id(self.element_id(&format!("chip-{name}")))
            .debug_selector(move || format!("findings-chip-{name}"))
            .px_1p5()
            .rounded_sm()
            .border_1()
            .border_color(if active { tint } else { theme.border })
            .when(active, |chip| chip.bg(tint.opacity(0.15)))
            .text_color(tint)
            .cursor_pointer()
            .child(SharedString::from(label))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_filter(severity, cx);
            }))
            .into_any_element()
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let title = match &self.spec.tool {
            Some(tool) => format!("Findings · {tool}"),
            None => "Findings".to_owned(),
        };
        let mut chips = h_flex().gap_1().flex_wrap().text_xs();
        for severity in SEVERITIES {
            let count = self
                .spec
                .findings
                .iter()
                .filter(|f| f.severity == severity)
                .count();
            if count > 0 {
                chips = chips.child(self.chip(severity, count, cx));
            }
        }
        h_flex()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(title)),
            )
            .child(chips)
            .into_any_element()
    }

    fn file_header(&self, group: &FileGroup, gx: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let folded = self.folded.contains(&group.path);
        let path = group.path.clone();
        let mut row = h_flex().gap_2().text_sm().child(
            div()
                .id(self.element_id(&format!("fold-{gx}")))
                .debug_selector(move || format!("findings-fold-{gx}"))
                .w(px(12.))
                .text_color(theme.muted_foreground)
                .cursor_pointer()
                .child(if folded { "▸" } else { "▾" })
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_file(&path, cx);
                })),
        );
        let label = div()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .font_weight(FontWeight::MEDIUM)
            .child(SharedString::from(group.path.clone()));
        let open = (group.path != NO_FILE)
            .then(|| {
                action(&SourceRef::File {
                    path: group.path.clone(),
                    line_start: None,
                    line_end: None,
                })
            })
            .flatten();
        row = match open {
            Some(event) => row.child(
                label
                    .id(self.element_id(&format!("file-{gx}")))
                    .debug_selector(move || format!("findings-file-{gx}"))
                    .text_color(theme.link)
                    .cursor_pointer()
                    .hover(|style| style.underline())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        activate(&event, cx);
                    }),
            ),
            None => row.child(label),
        };
        let counts = SEVERITIES
            .iter()
            .zip(group.counts)
            .filter(|(_, n)| *n > 0)
            .map(|(severity, n)| {
                div()
                    .text_xs()
                    .text_color(color(*severity, cx))
                    .child(SharedString::from(format!(
                        "{n} {}",
                        severity_name(*severity)
                    )))
            })
            .collect::<Vec<_>>();
        row.children(counts).into_any_element()
    }

    fn finding_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let finding = &self.spec.findings[ix];
        let tint = color(finding.severity, cx);
        let mut line = h_flex().gap_2().text_sm().child(
            div()
                .px_1()
                .rounded_sm()
                .text_xs()
                .text_color(tint)
                .border_1()
                .border_color(tint.opacity(0.5))
                .child(severity_name(finding.severity)),
        );
        if let (Some(line_no), Some(location)) = (line_of(finding), &finding.location) {
            let label = div()
                .text_xs()
                .font_family(theme.mono_font_family.clone())
                .child(SharedString::from(format!("{line_no}")));
            line = match action(location) {
                Some(event) => line.child(
                    label
                        .id(self.element_id(&format!("line-{ix}")))
                        .debug_selector(move || format!("findings-line-{ix}"))
                        .text_color(theme.link)
                        .cursor_pointer()
                        .hover(|style| style.underline())
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            activate(&event, cx);
                        }),
                ),
                None => line.child(label.text_color(theme.muted_foreground)),
            };
        }
        if let Some(rule) = &finding.rule {
            line = line.child(
                div()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(rule.clone())),
            );
        }
        if let Some(url) = finding.help_uri.clone().filter(|u| is_web(u)) {
            let event = BlockEvent::OpenUrl { url };
            line = line.child(
                div()
                    .id(self.element_id(&format!("docs-{ix}")))
                    .debug_selector(move || format!("findings-docs-{ix}"))
                    .text_xs()
                    .text_color(theme.link)
                    .cursor_pointer()
                    .hover(|style| style.underline())
                    .child("docs")
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        activate(&event, cx);
                    }),
            );
        }
        let mut row = v_flex()
            .gap_0p5()
            .pl(px(20.))
            .debug_selector(move || format!("findings-row-{ix}"))
            .child(line)
            .child(
                div()
                    .text_sm()
                    .whitespace_normal()
                    .child(SharedString::from(finding.message.clone())),
            );
        if let Some(snippet) = &finding.snippet {
            row = row.child(
                div()
                    .p_1()
                    .rounded_sm()
                    .bg(theme.muted)
                    .font_family(theme.mono_font_family.clone())
                    .text_xs()
                    .whitespace_normal()
                    .child(SharedString::from(snippet.trim_end().to_owned())),
            );
        }
        row.into_any_element()
    }
}

impl Render for FindingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, radius) = (theme.border, theme.muted_foreground, theme.radius);
        let mut list = v_flex().gap_2();
        let mut drawn = 0;
        let mut left_out = 0;
        for (gx, group) in groups(&self.spec).iter().enumerate() {
            let shown: Vec<usize> = group
                .findings
                .iter()
                .copied()
                .filter(|&ix| {
                    self.filter
                        .is_none_or(|f| self.spec.findings[ix].severity == f)
                })
                .collect();
            if shown.is_empty() {
                continue;
            }
            let mut file = v_flex().gap_1().child(self.file_header(group, gx, cx));
            if !self.folded.contains(&group.path) {
                for ix in shown {
                    if drawn == MAX_ROWS {
                        left_out += 1;
                        continue;
                    }
                    drawn += 1;
                    file = file.child(self.finding_row(ix, cx));
                }
            }
            list = list.child(file);
        }
        if left_out > 0 {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(format!("… {left_out} more findings"))),
            );
        }
        if self.spec.findings.is_empty() {
            list = list.child(div().text_color(muted).child("No findings"));
        }
        v_flex()
            .w_full()
            .gap_2()
            .px_3()
            .py_2()
            .rounded(radius)
            .border_1()
            .border_color(border)
            .debug_selector(|| "findings".to_owned())
            .child(self.header(cx))
            .child(list)
    }
}

/// The view of the findings block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &FindingsSpec,
    window: &mut Window,
    cx: &mut App,
) -> Entity<FindingsView> {
    let view = super::block_state(id, window, cx, |_, _| FindingsView::new(id, spec));
    view.update(cx, |view, cx| view.sync(spec, cx));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Findings(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use gpui::{Modifiers, TestAppContext, point, size};

    use super::*;
    use crate::blocks::sources::block_events;
    use crate::timeline::TimelineView;

    fn finding(path: Option<&str>, line: u32, severity: FindingSeverity) -> Finding {
        Finding {
            rule: Some("clippy::unwrap_used".into()),
            severity,
            message: format!("{} at {line}", severity_name(severity)),
            location: path.map(|p| SourceRef::File {
                path: p.into(),
                line_start: Some(line),
                line_end: None,
            }),
            snippet: Some("let x = f().unwrap();".into()),
            help_uri: Some("https://rust-lang.github.io/rust-clippy/#unwrap_used".into()),
            fix: None,
        }
    }

    fn spec() -> FindingsSpec {
        FindingsSpec {
            tool: Some("clippy 0.1.95".into()),
            findings: vec![
                finding(Some("src/b.rs"), 9, FindingSeverity::Warning),
                finding(Some("src/a.rs"), 20, FindingSeverity::Note),
                finding(Some("src/b.rs"), 2, FindingSeverity::Error),
                finding(None, 1, FindingSeverity::Error),
                finding(Some("src/b.rs"), 1, FindingSeverity::Warning),
            ],
        }
    }

    #[test]
    fn groups_by_file_with_the_worst_first() {
        let groups = groups(&spec());
        let paths: Vec<_> = groups.iter().map(|g| g.path.as_str()).collect();
        assert_eq!(paths, ["src/b.rs", NO_FILE, "src/a.rs"]);
        assert_eq!(groups[0].findings, [2, 4, 0]);
        assert_eq!(groups[0].counts, [1, 2, 0, 0]);
        assert!(is_web("https://x") && !is_web("javascript:alert(1)"));
    }

    #[gpui::test]
    fn draws_groups_filters_and_links(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let block = Block::new(BlockKind::Findings(spec()));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&events);
        cx.update(|_, cx| {
            let emitter = block_events(cx);
            cx.subscribe(&emitter, move |_, event: &BlockEvent, _| {
                recorded.borrow_mut().push(event.clone());
            })
            .detach();
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(point(px(0.), px(0.)), size(px(800.), px(900.)), |_, _| {
                div().size_full().child(view.clone())
            });
            cx.run_until_parked();
        };
        draw(cx);
        for row in [
            "findings-row-0",
            "findings-row-1",
            "findings-row-2",
            "findings-row-3",
            "findings-row-4",
        ] {
            assert!(cx.debug_bounds(row).is_some(), "{row}");
        }
        // The line link opens the file at the line; docs open the browser.
        let line = cx
            .debug_bounds("findings-line-2")
            .unwrap_or_else(|| panic!("no line"));
        cx.simulate_click(line.center(), Modifiers::none());
        assert!(matches!(
            events.borrow().last(),
            Some(BlockEvent::OpenFile { line: Some(2), .. })
        ));
        let docs = cx
            .debug_bounds("findings-docs-2")
            .unwrap_or_else(|| panic!("no docs"));
        cx.simulate_click(docs.center(), Modifiers::none());
        assert!(cx.opened_url().is_some_and(|u| u.ends_with("#unwrap_used")));
        // Only errors.
        let chip = cx
            .debug_bounds("findings-chip-error")
            .unwrap_or_else(|| panic!("no chip"));
        cx.simulate_click(chip.center(), Modifiers::none());
        draw(cx);
        let state = cx
            .update(|_, cx| crate::blocks::existing_block_state::<FindingsView>(id, cx))
            .unwrap_or_else(|| panic!("no state"));
        state.read_with(cx, |state, _| {
            assert_eq!(state.filter(), Some(FindingSeverity::Error))
        });
        assert!(cx.debug_bounds("findings-row-2").is_some());
        assert!(cx.debug_bounds("findings-row-0").is_none());
        // Folding the first file hides its rows.
        let fold = cx
            .debug_bounds("findings-fold-0")
            .unwrap_or_else(|| panic!("no fold"));
        cx.simulate_click(fold.center(), Modifiers::none());
        draw(cx);
        assert!(cx.debug_bounds("findings-row-2").is_none());
        assert!(cx.debug_bounds("findings-row-3").is_some());
    }
}
