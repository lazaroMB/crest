//! Renderer of the `test_report` block kind (`show_test_report`): a JUnit
//! run at a glance.
//!
//! - The header: title, a totals bar split by outcome and one chip per
//!   outcome (passed, failed, errored, skipped) with its count; a click on
//!   a chip shows only that outcome, a second click shows all again. The
//!   summed suite time sits at the end.
//! - Failures and errors first, then skipped tests, each with its name,
//!   class, duration, message and location (a link that opens the file,
//!   see [`super::sources::action`]). A click on a case opens its details
//!   (stack trace, captured output) in the monospace font.
//! - Passed tests are folded into one row until it is clicked or the
//!   `passed` chip is selected.
//!
//! The view is an entity per block ([`super::block_state`]) so the filter
//! and the open cases survive scrolling. At most [`MAX_ROWS`] cases are
//! drawn at once.

use std::collections::BTreeSet;

use domain::block::{BlockId, BlockKind};
use domain::report::{TestCase, TestOutcome, TestReportSpec};
use gpui::{
    AnyElement, App, Context, ElementId, Entity, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _, px, relative,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::sources::{action, activate, entry_label};
use crate::timeline::TimelineEntry;

/// Most cases drawn at once; the rest are counted.
pub const MAX_ROWS: usize = 300;

/// The outcomes in the order the block lists them.
pub const ORDER: [TestOutcome; 4] = [
    TestOutcome::Failed,
    TestOutcome::Errored,
    TestOutcome::Skipped,
    TestOutcome::Passed,
];

/// Lower-case name of an outcome.
#[must_use]
pub fn outcome_name(outcome: TestOutcome) -> &'static str {
    match outcome {
        TestOutcome::Passed => "passed",
        TestOutcome::Failed => "failed",
        TestOutcome::Errored => "errored",
        TestOutcome::Skipped => "skipped",
    }
}

fn glyph(outcome: TestOutcome) -> &'static str {
    match outcome {
        TestOutcome::Passed => "✓",
        TestOutcome::Failed => "✗",
        TestOutcome::Errored => "!",
        TestOutcome::Skipped => "○",
    }
}

fn color(outcome: TestOutcome, cx: &App) -> Hsla {
    let theme = cx.theme();
    match outcome {
        TestOutcome::Passed => theme.success,
        TestOutcome::Failed => theme.danger,
        TestOutcome::Errored => theme.warning,
        TestOutcome::Skipped => theme.muted_foreground,
    }
}

/// `1.5 s`, `120 ms`, `2 min 3 s`.
#[must_use]
pub fn duration(secs: f64) -> String {
    if secs < 1.0 {
        format!("{:.0} ms", secs * 1000.0)
    } else if secs < 60.0 {
        format!("{secs:.2} s")
    } else {
        let minutes = (secs / 60.0).floor();
        format!("{minutes:.0} min {:.0} s", secs - minutes * 60.0)
    }
}

/// The cases of `spec` in display order (failures and errors, skipped,
/// passed; report order within each), with their (suite, case) index.
#[must_use]
pub fn ordered_cases(spec: &TestReportSpec) -> Vec<((usize, usize), &TestCase)> {
    let mut cases: Vec<_> = spec
        .suites
        .iter()
        .enumerate()
        .flat_map(|(s, suite)| {
            suite
                .cases
                .iter()
                .enumerate()
                .map(move |(c, case)| ((s, c), case))
        })
        .collect();
    let rank = |outcome: TestOutcome| match outcome {
        TestOutcome::Failed | TestOutcome::Errored => 0,
        TestOutcome::Skipped => 1,
        TestOutcome::Passed => 2,
    };
    cases.sort_by_key(|(_, case)| rank(case.outcome));
    cases
}

/// Per-block state of the report.
pub struct TestReportView {
    id: BlockId,
    spec: TestReportSpec,
    /// Only this outcome is shown, when set.
    filter: Option<TestOutcome>,
    /// Passed cases are listed (otherwise folded into one row).
    show_passed: bool,
    /// Cases whose details are open, by (suite, case) index.
    open: BTreeSet<(usize, usize)>,
}

impl TestReportView {
    fn new(id: BlockId, spec: &TestReportSpec) -> Self {
        Self {
            id,
            spec: spec.clone(),
            filter: None,
            show_passed: false,
            open: BTreeSet::new(),
        }
    }

    /// The outcome filter.
    pub fn filter(&self) -> Option<TestOutcome> {
        self.filter
    }

    /// Shows only `outcome`, or everything again when it was the filter.
    pub fn toggle_filter(&mut self, outcome: TestOutcome, cx: &mut Context<Self>) {
        self.filter = (self.filter != Some(outcome)).then_some(outcome);
        cx.notify();
    }

    /// Opens or closes the details of case `key`.
    pub fn toggle_case(&mut self, key: (usize, usize), cx: &mut Context<Self>) {
        if !self.open.remove(&key) {
            self.open.insert(key);
        }
        cx.notify();
    }

    /// Whether the details of case `key` are open.
    pub fn is_open(&self, key: (usize, usize)) -> bool {
        self.open.contains(&key)
    }

    fn sync(&mut self, spec: &TestReportSpec, cx: &mut Context<Self>) {
        if &self.spec != spec {
            self.spec = spec.clone();
            self.open.clear();
            cx.notify();
        }
    }

    fn element_id(&self, what: &str) -> ElementId {
        ElementId::Name(SharedString::from(format!(
            "test-report-{what}-{}",
            self.id
        )))
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let totals = self.spec.totals();
        let theme = cx.theme();
        let title = self
            .spec
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Test report".to_owned());
        let time: f64 = self.spec.suites.iter().filter_map(|s| s.time_secs).sum();
        let counts = [
            (TestOutcome::Passed, totals.passed),
            (TestOutcome::Failed, totals.failed),
            (TestOutcome::Errored, totals.errored),
            (TestOutcome::Skipped, totals.skipped),
        ];
        let mut chips = h_flex().gap_1().flex_wrap().text_xs();
        for (outcome, count) in counts {
            if count == 0 && outcome != TestOutcome::Passed {
                continue;
            }
            let active = self.filter == Some(outcome);
            let tint = color(outcome, cx);
            let name = outcome_name(outcome);
            chips = chips.child(
                div()
                    .id(self.element_id(&format!("chip-{name}")))
                    .debug_selector(move || format!("test-report-chip-{name}"))
                    .px_1p5()
                    .rounded_sm()
                    .border_1()
                    .border_color(if active { tint } else { theme.border })
                    .when(active, |chip| chip.bg(tint.opacity(0.15)))
                    .text_color(tint)
                    .cursor_pointer()
                    .child(SharedString::from(format!("{count} {name}")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_filter(outcome, cx);
                    })),
            );
        }
        if time > 0.0 {
            chips = chips.child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(duration(time))),
            );
        }
        // The totals bar: one segment per outcome, as wide as its share.
        let total = totals.total().max(1) as f32;
        let mut bar = h_flex()
            .w_full()
            .h(px(4.))
            .rounded_sm()
            .overflow_hidden()
            .bg(theme.muted);
        for outcome in ORDER {
            let count = match outcome {
                TestOutcome::Passed => totals.passed,
                TestOutcome::Failed => totals.failed,
                TestOutcome::Errored => totals.errored,
                TestOutcome::Skipped => totals.skipped,
            };
            if count > 0 {
                bar = bar.child(
                    div()
                        .h_full()
                        .w(relative(count as f32 / total))
                        .bg(color(outcome, cx)),
                );
            }
        }
        v_flex()
            .gap_1()
            .child(
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
                    .child(chips),
            )
            .child(bar)
            .into_any_element()
    }

    fn case_row(&self, key: (usize, usize), case: &TestCase, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let tint = color(case.outcome, cx);
        let open = self.is_open(key);
        let mut line = h_flex()
            .gap_2()
            .child(div().w(px(12.)).text_color(tint).child(glyph(case.outcome)))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .whitespace_nowrap()
                            .child(SharedString::from(case.name.clone())),
                    )
                    .children(case.classname.as_ref().map(|class| {
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(class.clone()))
                    })),
            );
        if let Some(location) = &case.location
            && let Some(event) = action(location)
        {
            let (s, c) = key;
            line = line.child(
                div()
                    .id(self.element_id(&format!("loc-{s}-{c}")))
                    .debug_selector(move || format!("test-report-location-{s}-{c}"))
                    .text_xs()
                    .text_color(theme.link)
                    .cursor_pointer()
                    .hover(|style| style.underline())
                    .child(SharedString::from(entry_label(location)))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        activate(&event, cx);
                    }),
            );
        }
        if let Some(secs) = case.time_secs {
            line = line.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(duration(secs))),
            );
        }
        let mut row = v_flex().gap_0p5().child(line);
        if let Some(message) = &case.message {
            row = row.child(
                div()
                    .pl(px(20.))
                    .text_sm()
                    .text_color(if case.outcome == TestOutcome::Skipped {
                        theme.muted_foreground
                    } else {
                        tint
                    })
                    .child(SharedString::from(
                        message.lines().next().unwrap_or_default().to_owned(),
                    )),
            );
        }
        let has_details =
            case.details.is_some() || case.message.as_ref().is_some_and(|m| m.contains('\n'));
        if open {
            let details = match (&case.details, &case.message) {
                (Some(details), _) => details.clone(),
                (None, Some(message)) => message.clone(),
                (None, None) => String::new(),
            };
            row = row.child(
                div()
                    .ml(px(20.))
                    .p_2()
                    .rounded_sm()
                    .bg(theme.muted)
                    .font_family(theme.mono_font_family.clone())
                    .text_xs()
                    .whitespace_normal()
                    .debug_selector(|| "test-report-details".to_owned())
                    .child(SharedString::from(details)),
            );
        }
        let (s, c) = key;
        let mut row = div()
            .id(self.element_id(&format!("case-{s}-{c}")))
            .debug_selector(move || format!("test-report-case-{s}-{c}"))
            .child(row);
        if has_details {
            row = row
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_case(key, cx);
                }));
        }
        row.into_any_element()
    }
}

impl Render for TestReportView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, radius) = (theme.border, theme.muted_foreground, theme.radius);
        let spec = self.spec.clone();
        let cases = ordered_cases(&spec);
        let mut list = v_flex().gap_1();
        let mut drawn = 0;
        let mut hidden_passed = 0;
        for (key, case) in &cases {
            let shown = match self.filter {
                Some(filter) => case.outcome == filter,
                None => case.outcome != TestOutcome::Passed || self.show_passed,
            };
            if !shown {
                if case.outcome == TestOutcome::Passed {
                    hidden_passed += 1;
                }
                continue;
            }
            if drawn == MAX_ROWS {
                drawn += 1;
                continue;
            }
            drawn += 1;
            list = list.child(self.case_row(*key, case, cx));
        }
        if drawn > MAX_ROWS {
            list = list.child(div().text_xs().text_color(muted).child(SharedString::from(
                format!("… more cases not drawn (showing {MAX_ROWS})"),
            )));
        }
        if self.filter.is_none() && hidden_passed > 0 {
            list = list.child(
                div()
                    .id(self.element_id("passed"))
                    .debug_selector(|| "test-report-passed".to_owned())
                    .text_sm()
                    .text_color(muted)
                    .cursor_pointer()
                    .hover(|style| style.underline())
                    .child(SharedString::from(format!(
                        "✓ {hidden_passed} passed · show"
                    )))
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.show_passed = true;
                        cx.notify();
                    })),
            );
        }
        if cases.is_empty() {
            list = list.child(div().text_color(muted).child("No test cases"));
        }
        v_flex()
            .w_full()
            .gap_2()
            .px_3()
            .py_2()
            .rounded(radius)
            .border_1()
            .border_color(border)
            .debug_selector(|| "test-report".to_owned())
            .child(self.header(cx))
            .child(list)
    }
}

/// The view of the report block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &TestReportSpec,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TestReportView> {
    let view = super::block_state(id, window, cx, |_, _| TestReportView::new(id, spec));
    view.update(cx, |view, cx| view.sync(spec, cx));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::TestReport(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use application::ports::events::DomainEvent;
    use domain::block::{Block, SourceRef};
    use domain::report::TestSuite;
    use domain::workspace::WorkspacePath;
    use gpui::{Modifiers, TestAppContext, point, size};

    use super::*;
    use crate::blocks::sources::{BlockEvent, block_events};
    use crate::timeline::TimelineView;

    fn case(name: &str, outcome: TestOutcome) -> TestCase {
        TestCase {
            name: name.into(),
            classname: Some("tests::math".into()),
            outcome,
            time_secs: Some(0.25),
            message: (outcome != TestOutcome::Passed).then(|| format!("{name} went wrong")),
            details: (outcome == TestOutcome::Failed).then(|| "stack\n  at a.rs:3".to_owned()),
            location: Some(SourceRef::File {
                path: "tests/math.rs".into(),
                line_start: Some(3),
                line_end: None,
            }),
        }
    }

    fn spec() -> TestReportSpec {
        TestReportSpec {
            title: Some("CI".into()),
            suites: vec![TestSuite {
                name: "unit".into(),
                time_secs: Some(1.5),
                cases: vec![
                    case("adds", TestOutcome::Passed),
                    case("divides", TestOutcome::Failed),
                    case("slow", TestOutcome::Skipped),
                    case("opens", TestOutcome::Errored),
                ],
            }],
        }
    }

    #[test]
    fn failures_come_first_and_durations_read_well() {
        let spec = spec();
        let names: Vec<_> = ordered_cases(&spec)
            .iter()
            .map(|(_, c)| c.name.as_str())
            .collect();
        assert_eq!(names, ["divides", "opens", "slow", "adds"]);
        assert_eq!(duration(0.12), "120 ms");
        assert_eq!(duration(1.5), "1.50 s");
        assert_eq!(duration(125.0), "2 min 5 s");
    }

    #[gpui::test]
    fn draws_failures_first_filters_and_opens_details(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let block = Block::new(BlockKind::TestReport(spec()));
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
            cx.draw(point(px(0.), px(0.)), size(px(800.), px(700.)), |_, _| {
                div().size_full().child(view.clone())
            });
            cx.run_until_parked();
        };
        draw(cx);
        // Passed cases are folded; the failure is listed.
        assert!(cx.debug_bounds("test-report-case-0-1").is_some());
        assert!(cx.debug_bounds("test-report-case-0-0").is_none());
        assert!(cx.debug_bounds("test-report-passed").is_some());
        // A click on the failure opens its details.
        let failure = cx
            .debug_bounds("test-report-case-0-1")
            .unwrap_or_else(|| panic!("no row"));
        cx.simulate_click(failure.center(), Modifiers::none());
        draw(cx);
        assert!(cx.debug_bounds("test-report-details").is_some());
        let state =
            cx.update(|_, cx| crate::blocks::existing_block_state::<TestReportView>(id, cx));
        let state = state.unwrap_or_else(|| panic!("no state"));
        state.read_with(cx, |state, _| assert!(state.is_open((0, 1))));
        // The location opens the file.
        let link = cx
            .debug_bounds("test-report-location-0-1")
            .unwrap_or_else(|| panic!("no location"));
        cx.simulate_click(link.center(), Modifiers::none());
        assert!(matches!(
            events.borrow().last(),
            Some(BlockEvent::OpenFile { path, line: Some(3) })
                if *path == WorkspacePath::parse("tests/math.rs").unwrap_or_else(|e| panic!("{e}"))
        ));
        // The passed chip shows only passed cases.
        let chip = cx
            .debug_bounds("test-report-chip-passed")
            .unwrap_or_else(|| panic!("no chip"));
        cx.simulate_click(chip.center(), Modifiers::none());
        draw(cx);
        state.read_with(cx, |state, _| {
            assert_eq!(state.filter(), Some(TestOutcome::Passed))
        });
        assert!(cx.debug_bounds("test-report-case-0-0").is_some());
        assert!(cx.debug_bounds("test-report-case-0-1").is_none());
    }
}
