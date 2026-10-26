//! Renderer of the `computed` block kind: the statistics card.
//!
//! Every card wraps a [`Computed`] envelope and shares one anatomy (see
//! `docs/research/chat-blocks-for-technical-work.md`, "Statistics blocks"):
//!
//! 1. header: the method and why it was chosen, the provenance chip
//!    (engine, input hash, time; click for the inputs and reproduce lines)
//!    and the verified / mismatch / stale badge;
//! 2. the assumption checklist, open when a check fails (a calc sheet
//!    shows its unit readings as chips on their lines instead);
//! 3. the results, drawn per body ([`test`], [`descriptive`],
//!    [`regression`], [`correlation`], [`calc`] for the calc sheet and
//!    the quantity card, and the phase-2 cards: [`distribution`], [`contingency`], [`power`], [`ab_test`], [`bootstrap`], [`forecast`], [`optimization`], [`equations`], [`simulation`], [`cash_flow`], [`decision_matrix`], and the engineering cards: [`bode`], [`karnaugh`], [`beam`], [`tolerance`]),
//!    numbers right-aligned in the monospace font;
//! 4. the plain-language sentence and
//! 5. the APA line (italic symbols) with a copy button, both filled from
//!    the envelope by [`narrative`];
//! 6. warnings (a calc sheet shows line errors on their lines instead);
//! 7. alternatives to the chosen method;
//! 8. the footer: engine output (the raw envelope) / card toggle,
//!    Recompute, copy the reproduce line (Lua, Python, R) and copy the
//!    envelope JSON.
//!
//! Recompute emits [`BlockEvent::Recompute`]; the session asks the
//! controller to re-run the envelope, which answers with a `BlockUpdated`
//! whose `verification` the badge shows. A mismatch lists the changed
//! values as a diff.
//!
//! The card is an entity kept per block ([`super::block_state`]), so its
//! toggles survive scrolling.

pub mod ab_test;
pub mod beam;
pub mod bode;
pub mod bootstrap;
pub mod calc;
pub mod cash_flow;
pub mod contingency;
pub mod correlation;
pub mod decision_matrix;
pub mod descriptive;
pub mod distribution;
pub mod equations;
pub mod forecast;
pub mod grid;
pub mod karnaugh;
pub mod optimization;
pub mod power;
pub mod regression;
pub mod simulation;
pub mod test;
pub mod tolerance;

use domain::block::{BlockId, BlockKind};
use domain::computed::apa::{self, ApaToken};
use domain::computed::{
    Assumption, AssumptionStatus, Computed, ComputedBody, ComputedStatus, ValueFormat,
    WarningSeverity, narrative,
};
use gpui::{
    AnyElement, App, ClipboardItem, Context, ElementId, Entity, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use serde_json::Value;

use self::grid::Grid;
use super::sources::events::{BlockEvent, emit};
use crate::timeline::TimelineEntry;

/// Code of the warning a recompute adds when the data file changed.
pub const INPUT_CHANGED: &str = application::usecases::compute::INPUT_CHANGED;

/// `x` with `decimals` decimals and a true minus sign.
pub fn number(x: f64, decimals: u32) -> String {
    apa::number(x, decimals)
}

/// Decimals that show numbers of about `scale` with three significant
/// digits, as the engine does.
pub fn auto_decimals(scale: f64) -> u32 {
    if !(scale.is_finite() && scale > 0.0) {
        return 2;
    }
    #[allow(clippy::cast_possible_truncation)]
    let exponent = scale.log10().floor() as i32;
    (2 - exponent).clamp(0, 6).unsigned_abs()
}

/// A results grid of the headline `keys` (with their labels) that the
/// envelope has: value, interval and the effect-size label.
pub fn headline_grid(computed: &Computed, keys: &[(&str, &str)]) -> Grid {
    let mut grid = Grid::new(["Result", "Value", "CI low", "CI high"]);
    for (key, label) in keys {
        let Some(value) = computed.value(key) else {
            continue;
        };
        let label = match &value.label {
            Some(word) => format!("{label} ({word})"),
            None => (*label).to_owned(),
        };
        let (lo, hi) = match &value.ci {
            Some(ci) => {
                let decimals = value.decimals.unwrap_or(2);
                let fmt = |x| {
                    if value.fmt == Some(ValueFormat::Bounded) {
                        apa::bounded(x, decimals)
                    } else {
                        apa::number(x, decimals)
                    }
                };
                (fmt(ci.lo), fmt(ci.hi))
            }
            None => (String::new(), String::new()),
        };
        grid.push([label, value.display_text(), lo, hi]);
    }
    grid
}

/// What the verification badge says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Badge {
    /// Recomputed and every value matched.
    Verified(String),
    /// Recomputed and some values changed.
    Mismatch(usize),
}

/// The badge of `computed`, if it was recomputed.
pub fn badge(computed: &Computed) -> Option<Badge> {
    let at = computed
        .verification
        .recomputed_at
        .clone()
        .unwrap_or_default();
    match computed.verification.matched? {
        true => Some(Badge::Verified(at)),
        false => Some(Badge::Mismatch(computed.verification.diff.len())),
    }
}

/// Whether a recompute found the data file changed.
pub fn is_stale(computed: &Computed) -> bool {
    computed.warnings.iter().any(|w| w.code == INPUT_CHANGED)
}

/// The changed values as a unified diff, for [`super::diff::render_unified`].
pub fn diff_text(computed: &Computed) -> String {
    let mut out = String::from("--- stored\n+++ recomputed\n@@ values @@\n");
    let show = |v: Option<f64>| v.map_or_else(|| "(none)".to_owned(), |v| format!("{v}"));
    for d in &computed.verification.diff {
        out.push_str(&format!("-{} = {}\n", d.key, show(d.old)));
        out.push_str(&format!("+{} = {}\n", d.key, show(d.new)));
    }
    out
}

/// The provenance chip text: `crest-stats 0.15.0 · blake3:9c1e3a4b · 3 ms`.
pub fn provenance_text(computed: &Computed) -> String {
    let mut parts = vec![format!(
        "{} {}",
        computed.engine.name, computed.engine.version
    )];
    if let Some(hash) = computed.inputs.first().and_then(|i| i.hash.as_deref()) {
        let (prefix, hex) = hash.split_once(':').unwrap_or(("", hash));
        let short = &hex[..hex.len().min(8)];
        parts.push(if prefix.is_empty() {
            short.to_owned()
        } else {
            format!("{prefix}:{short}")
        });
    }
    if let Some(ms) = computed.provenance.duration_ms {
        parts.push(format!("{ms} ms"));
    }
    parts.join(" · ")
}

/// Text of an assumption's statistic and p, e.g. `0.94, p = .38`.
fn assumption_numbers(a: &Assumption) -> String {
    let stat = a.stat.map(|s| apa::number(s, 2));
    let p = a.p.map(|p| {
        let (relation, value) = apa::p_value_parts(p);
        format!("p {relation} {value}")
    });
    [stat, p]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ")
}

fn status_label(status: AssumptionStatus) -> &'static str {
    match status {
        AssumptionStatus::Pass => "pass",
        AssumptionStatus::Warn => "warn",
        AssumptionStatus::Fail => "fail",
        AssumptionStatus::NotChecked => "not checked",
    }
}

/// The card of one block.
pub struct ComputedCard {
    id: BlockId,
    envelope: Computed,
    body: ComputedBody,
    /// Shows the raw envelope instead of the card.
    raw: bool,
    /// The provenance chip is open.
    provenance: bool,
    /// The user's choice for the assumption list; `None` follows the rule
    /// (open when one fails).
    assumptions: Option<bool>,
    /// A recompute was asked for and has not answered yet.
    recomputing: bool,
    /// Why the last recompute failed.
    error: Option<String>,
    /// The envelope as JSON, built when first shown.
    json: Option<Value>,
}

impl ComputedCard {
    fn new(id: BlockId, envelope: Computed, body: ComputedBody) -> Self {
        Self {
            id,
            envelope,
            body,
            raw: false,
            provenance: false,
            assumptions: None,
            recomputing: false,
            error: None,
            json: None,
        }
    }

    /// Takes the latest version of the block.
    fn sync(&mut self, envelope: &Computed, body: &ComputedBody, cx: &mut Context<Self>) {
        if &self.envelope != envelope || &self.body != body {
            self.envelope = envelope.clone();
            self.body = body.clone();
            self.recomputing = false;
            self.error = None;
            self.json = None;
            cx.notify();
        }
    }

    /// Whether a recompute is on its way.
    pub fn is_recomputing(&self) -> bool {
        self.recomputing
    }

    /// Records that the recompute failed with `message`.
    pub fn recompute_failed(&mut self, message: String, cx: &mut Context<Self>) {
        self.recomputing = false;
        self.error = Some(message);
        cx.notify();
    }

    fn element_id(&self, name: &str) -> ElementId {
        ElementId::Name(SharedString::from(format!("computed-{name}-{}", self.id)))
    }

    fn key(&self, name: &str) -> String {
        format!("computed-{name}-{}", self.id)
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let computed = &self.envelope;
        let mut title = h_flex().gap_2().flex_wrap().items_center().child(
            div()
                .font_weight(FontWeight::BOLD)
                .child(SharedString::from(computed.method.name.clone())),
        );
        let chip = div()
            .id(self.element_id("provenance"))
            .debug_selector(|| "computed-provenance".to_owned())
            .px_1()
            .rounded_sm()
            .border_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .cursor_pointer()
            .hover(|s| s.bg(theme.muted))
            .child(SharedString::from(provenance_text(computed)))
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.provenance = !this.provenance;
                cx.notify();
            }));
        title = title.child(chip);
        let pill = |text: String, color: Hsla| {
            div()
                .px_1()
                .rounded_sm()
                .text_xs()
                .bg(color.opacity(0.15))
                .text_color(color)
                .child(SharedString::from(text))
        };
        match badge(computed) {
            Some(Badge::Verified(at)) => {
                let text = if at.is_empty() {
                    "✓ Verified".to_owned()
                } else {
                    format!("✓ Verified · reproduced {at}")
                };
                title = title.child(pill(text, theme.success));
            }
            Some(Badge::Mismatch(n)) => {
                title = title.child(pill(
                    format!(
                        "✗ Mismatch · {n} value{} changed",
                        if n == 1 { "" } else { "s" }
                    ),
                    theme.danger,
                ));
            }
            None => {}
        }
        if is_stale(computed) {
            title = title.child(pill("Stale input".into(), theme.warning));
        }
        if self.recomputing {
            title = title.child(pill("Recomputing…".into(), theme.info));
        }
        let mut header = v_flex().gap_0p5().child(title);
        if let Some(why) = &computed.method.why {
            header = header.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(format!("Why: {why}"))),
            );
        }
        header.into_any_element()
    }

    fn provenance_details(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let computed = &self.envelope;
        let mut lines: Vec<String> = Vec::new();
        for input in &computed.inputs {
            let mut line = format!("input {}", input.path.as_deref().unwrap_or(&input.name));
            if let Some(rows) = input.rows {
                line.push_str(&format!(" · {rows} rows"));
            }
            if !input.columns.is_empty() {
                line.push_str(&format!(" · {}", input.columns.join(", ")));
            }
            if let Some(hash) = &input.hash {
                line.push_str(&format!(" · {hash}"));
            }
            lines.push(line);
        }
        if let Some(backend) = &computed.engine.backend {
            lines.push(format!(
                "engine {} {} on {backend}{}",
                computed.engine.name,
                computed.engine.version,
                computed
                    .engine
                    .platform
                    .as_deref()
                    .map(|p| format!(" ({p})"))
                    .unwrap_or_default()
            ));
        }
        if let Some(started) = &computed.provenance.activity_started {
            lines.push(format!(
                "ran {started}{}",
                computed
                    .provenance
                    .agent
                    .as_deref()
                    .map(|a| format!(" for {a}"))
                    .unwrap_or_default()
            ));
        }
        let reproduce = &computed.reproduce;
        for (lang, line) in [
            ("lua", &reproduce.lua),
            ("python", &reproduce.python),
            ("r", &reproduce.r),
            ("shell", &reproduce.shell),
        ] {
            if let Some(line) = line {
                lines.push(format!("{lang}: {line}"));
            }
        }
        v_flex()
            .pl_2()
            .border_l_2()
            .border_color(theme.border)
            .text_xs()
            .font_family(theme.mono_font_family.clone())
            .text_color(theme.muted_foreground)
            .children(
                lines
                    .into_iter()
                    .map(|l| div().child(SharedString::from(l))),
            )
            .into_any_element()
    }

    fn assumptions(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let items = &self.envelope.assumptions;
        if items.is_empty() || matches!(self.body, ComputedBody::CalcSheet(_)) {
            return None;
        }
        let theme = cx.theme();
        let count = |status| items.iter().filter(|a| a.status == status).count();
        let failing = count(AssumptionStatus::Fail);
        let open = self.assumptions.unwrap_or(failing > 0);
        let summary = [
            AssumptionStatus::Pass,
            AssumptionStatus::Warn,
            AssumptionStatus::Fail,
            AssumptionStatus::NotChecked,
        ]
        .into_iter()
        .filter_map(|s| {
            let n = count(s);
            (n > 0).then(|| format!("{n} {}", status_label(s)))
        })
        .collect::<Vec<_>>()
        .join(" · ");
        let toggle = div()
            .id(self.element_id("assumptions"))
            .debug_selector(|| "computed-assumptions".to_owned())
            .cursor_pointer()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(SharedString::from(format!(
                "{} Assumptions: {summary}",
                if open { "▾" } else { "▸" }
            )))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.assumptions = Some(!open);
                cx.notify();
            }));
        let mut list = v_flex().gap_0p5().child(toggle);
        if open {
            let color = |status| match status {
                AssumptionStatus::Pass => theme.success,
                AssumptionStatus::Warn => theme.warning,
                AssumptionStatus::Fail => theme.danger,
                AssumptionStatus::NotChecked => theme.muted_foreground,
            };
            list = list.children(items.iter().map(|a| {
                let tone = color(a.status);
                let mut row = h_flex().gap_2().pl_4().text_sm().items_start().child(
                    div()
                        .flex_none()
                        .px_1()
                        .rounded_sm()
                        .text_xs()
                        .bg(tone.opacity(0.15))
                        .text_color(tone)
                        .child(status_label(a.status)),
                );
                let name = match &a.check {
                    Some(check) => format!("{} ({check})", a.id),
                    None => a.id.clone(),
                };
                row = row.child(div().child(SharedString::from(name)));
                let numbers = assumption_numbers(a);
                if !numbers.is_empty() {
                    row = row.child(
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .child(SharedString::from(numbers)),
                    );
                }
                let note = [a.consequence.as_deref(), a.note.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("; ");
                if !note.is_empty() {
                    row = row.child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(format!("→ {note}"))),
                    );
                }
                row
            }));
        }
        Some(list.into_any_element())
    }

    fn results(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        let id = self.key("body");
        match &self.body {
            ComputedBody::Test(body) => test::render(&id, &self.envelope, body, cx),
            ComputedBody::Descriptive(body) => descriptive::render(&id, body, cx),
            ComputedBody::Regression(body) => regression::render(&id, body, window, cx),
            ComputedBody::Correlation(body) => correlation::render(&id, body, cx),
            ComputedBody::CalcSheet(body) => calc::calc_sheet(body, window, cx),
            ComputedBody::Distribution(body) => distribution::render(&id, body, window, cx),
            ComputedBody::Contingency(body) => contingency::render(&id, body, cx),
            ComputedBody::Power(body) => power::render(&id, &self.envelope, body, window, cx),
            ComputedBody::AbTest(body) => ab_test::render(&id, &self.envelope, body, window, cx),
            ComputedBody::Forecast(body) => forecast::render(&id, &self.envelope, body, window, cx),
            ComputedBody::Optimization(body) => optimization::render(&id, body, window, cx),
            ComputedBody::Equations(body) => equations::render(&id, body, window, cx),
            ComputedBody::CashFlow(body) => {
                cash_flow::render(&id, &self.envelope, body, window, cx)
            }
            ComputedBody::DecisionMatrix(body) => decision_matrix::render(&id, body, cx),
            ComputedBody::Bode(body) => bode::render(&id, &self.envelope, body, window, cx),
            ComputedBody::Karnaugh(body) => karnaugh::render(&id, body, cx),
            ComputedBody::Beam(body) => beam::render(&id, &self.envelope, body, window, cx),
            ComputedBody::Tolerance(body) => {
                tolerance::render(&id, &self.envelope, body, window, cx)
            }
            ComputedBody::Simulation(body) => {
                simulation::render(&id, &self.envelope, body, window, cx)
            }
            ComputedBody::Bootstrap(body) => {
                bootstrap::render(&id, &self.envelope, body, window, cx)
            }
            ComputedBody::Quantity(body) => calc::quantity(
                body,
                self.envelope
                    .value("result")
                    .map(domain::computed::ComputedValue::display_text),
                cx,
            ),
        }
    }

    fn apa(&self) -> Option<AnyElement> {
        let line = narrative::apa_line(&self.envelope, &self.body)?;
        let plain = line.plain();
        let tokens = line.tokens().into_iter().map(|token| match token {
            ApaToken::Plain(text) => div().child(SharedString::from(text)),
            ApaToken::Italic(text) => div().italic().child(SharedString::from(text)),
        });
        let copy = Button::new(self.element_id("copy-apa"))
            .ghost()
            .xsmall()
            .label("Copy")
            .tooltip("Copy the APA line")
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                cx.write_to_clipboard(ClipboardItem::new_string(plain.clone()));
            });
        Some(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    h_flex()
                        .debug_selector(|| "computed-apa".to_owned())
                        .flex_wrap()
                        .children(tokens),
                )
                .child(copy)
                .into_any_element(),
        )
    }

    fn warnings(&self, cx: &App) -> Option<AnyElement> {
        let on_lines = matches!(self.body, ComputedBody::CalcSheet(_));
        let warnings: Vec<_> = self
            .envelope
            .warnings
            .iter()
            .filter(|w| !(on_lines && w.code == calc::LINE_ERROR))
            .collect();
        if warnings.is_empty() {
            return None;
        }
        let theme = cx.theme();
        Some(
            v_flex()
                .gap_0p5()
                .text_sm()
                .children(warnings.iter().enumerate().map(|(ix, w)| {
                    let color = match w.severity {
                        WarningSeverity::Info => theme.muted_foreground,
                        WarningSeverity::Warn => theme.warning,
                        WarningSeverity::Error => theme.danger,
                    };
                    div().text_color(color).child(SharedString::from(format!(
                        "[{}] {}",
                        ix + 1,
                        w.text
                    )))
                }))
                .into_any_element(),
        )
    }

    fn alternatives(&self, cx: &App) -> Option<AnyElement> {
        let alternatives = &self.envelope.method.alternatives;
        if alternatives.is_empty() {
            return None;
        }
        let theme = cx.theme();
        Some(
            h_flex()
                .gap_1()
                .flex_wrap()
                .text_xs()
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child("Alternatives:"),
                )
                .children(alternatives.iter().map(|alt| {
                    div()
                        .px_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(theme.border)
                        .child(SharedString::from(alt.replace('_', " ")))
                }))
                .into_any_element(),
        )
    }

    fn footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let id = self.id;
        let raw_label = if self.raw { "Card" } else { "Engine output" };
        let mut row = h_flex()
            .gap_1()
            .flex_wrap()
            .child(
                Button::new(self.element_id("raw"))
                    .ghost()
                    .xsmall()
                    .label(raw_label)
                    .tooltip("Model said / engine computed: the raw envelope")
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.raw = !this.raw;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .debug_selector(|| "computed-recompute".to_owned())
                    .child(
                        Button::new(self.element_id("recompute"))
                            .ghost()
                            .xsmall()
                            .label("Recompute")
                            .tooltip("Run the same request again and compare every value")
                            .disabled(self.recomputing)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.recomputing = true;
                                this.error = None;
                                cx.notify();
                                emit(BlockEvent::Recompute { block: id }, cx);
                            })),
                    ),
            );
        // The same forms the copy-as row offers for other blocks; the APA
        // line has its own button next to it.
        let formats = super::copy::computed(&self.envelope, &self.body);
        for format in formats.into_iter().filter(|f| f.key != "apa") {
            let tooltip = match format.key {
                "json" => "Copy the envelope".to_owned(),
                "lua" | "python" | "r" | "shell" => "Copy the reproduce command".to_owned(),
                _ => format!("Copy the {}", format.label),
            };
            let text = format.text;
            row = row.child(
                Button::new(self.element_id(&format!("copy-{}", format.key)))
                    .ghost()
                    .xsmall()
                    .label(format!("Copy {}", format.label))
                    .tooltip(tooltip)
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    }),
            );
        }
        row.into_any_element()
    }
}

impl Render for ComputedCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut card = v_flex()
            .w_full()
            .gap_2()
            .debug_selector(|| "computed-card".to_owned())
            .child(self.header(cx));
        if self.provenance {
            card = card.child(self.provenance_details(cx));
        }
        let failed = self.envelope.status == ComputedStatus::Error;
        if failed {
            let message = self
                .envelope
                .error
                .clone()
                .unwrap_or_else(|| "the computation failed".into());
            card = card.child(
                div()
                    .text_color(cx.theme().danger)
                    .child(SharedString::from(message)),
            );
        }
        if self.raw {
            let json = self
                .json
                .get_or_insert_with(|| serde_json::to_value(&self.envelope).unwrap_or_default())
                .clone();
            let view = super::json::json_view(self.id, &json, 2, window, cx);
            card = card.child(view);
        } else if !failed {
            card = card.children(self.assumptions(cx));
            card = card.child(self.results(window, cx));
            if let Some(sentence) = narrative::sentence(&self.envelope, &self.body) {
                card = card.child(
                    div()
                        .debug_selector(|| "computed-sentence".to_owned())
                        .child(SharedString::from(sentence)),
                );
            }
            card = card.children(self.apa());
            card = card.children(self.warnings(cx));
            card = card.children(self.alternatives(cx));
        }
        if self.envelope.verification.matched == Some(false)
            && !self.envelope.verification.diff.is_empty()
        {
            card = card.child(
                div()
                    .border_l_2()
                    .border_color(cx.theme().danger)
                    .child(super::diff::render_unified(&diff_text(&self.envelope), cx)),
            );
        }
        if let Some(error) = &self.error {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(SharedString::from(format!("Recompute failed: {error}"))),
            );
        }
        card.child(self.footer(cx))
    }
}

/// The card entity of block `id`, in sync with its envelope and body.
pub fn card(
    id: BlockId,
    envelope: &Computed,
    body: &ComputedBody,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ComputedCard> {
    let card = super::block_state(id, window, cx, |_, _| {
        ComputedCard::new(id, envelope.clone(), body.clone())
    });
    card.update(cx, |card, cx| card.sync(envelope, body, cx));
    card
}

/// Tells the card of block `id`, if it was drawn, that its recompute
/// failed.
pub fn recompute_failed(id: BlockId, message: String, cx: &mut App) {
    if let Some(card) = super::existing_block_state::<ComputedCard>(id, cx) {
        card.update(cx, |card, cx| card.recompute_failed(message, cx));
    }
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Computed { envelope, body } = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    card(entry.block.id, envelope, body, window, cx).into_any_element()
}

#[cfg(test)]
mod tests;
