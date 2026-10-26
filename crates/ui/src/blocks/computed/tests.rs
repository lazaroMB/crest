use std::cell::RefCell;
use std::rc::Rc;

use application::ports::events::DomainEvent;
use domain::block::Block;
use domain::computed::{
    Alternative, CalcLine, CalcSheetBody, CoefficientRow, ComputedValue, ComputedWarning,
    CorrelationBody, CorrelationMethod, DescriptiveBody, Diagnostics, Engine, FitStats,
    GroupSummary, Interval, Narrative, PAdjust, Quantity, QuantityBody, RegressionBody, TestBody,
    TestKind, ValueDiff, VariableKind, VariableSummary,
};
use gpui::{Modifiers, TestAppContext, VisualTestContext, px};

use super::*;
use crate::blocks::sources::{BlockEvent, block_events};
use crate::timeline::TimelineView;

fn engine() -> Engine {
    Engine {
        name: "crest-stats".into(),
        version: "0.15.0".into(),
        backend: Some("statrs 0.19.1".into()),
        platform: Some("x86_64-linux".into()),
    }
}

fn shown(v: f64, display: &str) -> ComputedValue {
    ComputedValue {
        display: Some(display.into()),
        ..ComputedValue::new(v)
    }
}

fn welch() -> (Computed, ComputedBody) {
    let mut c = Computed::new(
        "st_abc",
        "stats.ttest_welch",
        "Welch two-sample t-test",
        engine(),
    );
    c.method.why = Some("Levene p = .02 → unequal variances".into());
    c.method.alternatives = vec!["student_t".into(), "mann_whitney".into()];
    c.values.insert("statistic".into(), shown(-3.767, "−3.77"));
    c.values.insert("df".into(), shown(18.33, "18.33"));
    c.values.insert("p_value".into(), shown(0.001_374, ".001"));
    c.values.insert(
        "cohens_d".into(),
        ComputedValue {
            ci: Some(Interval {
                level: 0.95,
                lo: -2.27,
                hi: -0.67,
                method: None,
            }),
            label: Some("large".into()),
            ..shown(-1.48, "−1.48")
        },
    );
    c.values.insert("effect".into(), shown(-1.48, "−1.48"));
    c.assumptions.push(Assumption {
        id: "equal_var".into(),
        check: Some("Levene".into()),
        stat: Some(6.2),
        p: Some(0.02),
        status: AssumptionStatus::Fail,
        consequence: Some("used Welch correction".into()),
        note: None,
    });
    c.warnings.push(ComputedWarning {
        code: "small_n".into(),
        severity: WarningSeverity::Warn,
        text: "n < 20 per group".into(),
    });
    c.inputs.push(domain::computed::InputRef {
        name: "mtcars.csv".into(),
        role: Some("data".into()),
        path: Some("mtcars.csv".into()),
        hash: Some("blake3:9c1e3a4b5d6e7f".into()),
        rows: Some(32),
        columns: vec!["mpg".into(), "am".into()],
        filter: None,
    });
    c.provenance.duration_ms = Some(3);
    c.reproduce.lua = Some("app.compute.test{data = 'mtcars.csv'}".into());
    c.narrative = Some(Narrative {
        template: "apa_ttest".into(),
        text_slots: vec!["statistic".into()],
    });
    let group = |name: &str, mean| GroupSummary {
        name: name.into(),
        n: 13,
        mean: Some(mean),
        sd: Some(3.8),
        median: None,
    };
    let body = ComputedBody::Test(TestBody {
        test: TestKind::WelchT,
        alternative: Alternative::TwoSided,
        groups: vec![group("auto", 17.15), group("manual", 24.39)],
        observed: None,
        expected: None,
    });
    (c, body)
}

fn other_cards() -> Vec<(Computed, ComputedBody)> {
    let summary = VariableSummary {
        name: "mpg".into(),
        group: None,
        kind: VariableKind::Numeric,
        unit: None,
        n: 32,
        missing: 0,
        mean: Some(20.09),
        sd: Some(6.03),
        median: Some(19.2),
        q1: Some(15.4),
        q3: Some(22.8),
        min: Some(10.4),
        max: Some(33.9),
        skew: Some(0.61),
        kurtosis: Some(-0.37),
        histogram: Some(domain::computed::Histogram {
            edges: vec![10.0, 20.0, 30.0, 34.0],
            counts: vec![14, 16, 2],
        }),
        levels: Vec::new(),
    };
    let describe = ComputedBody::Descriptive(DescriptiveBody {
        by: None,
        variables: vec![summary],
    });
    let regression = ComputedBody::Regression(RegressionBody {
        dependent: "mpg".into(),
        ci_level: 0.95,
        coefficients: vec![CoefficientRow {
            term: "wt".into(),
            estimate: -5.34,
            se: 0.56,
            t: -9.56,
            p: 1.3e-10,
            ci_lo: -6.49,
            ci_hi: -4.20,
            std_beta: Some(-0.87),
            vif: None,
        }],
        fit: FitStats {
            n: 32,
            r_squared: 0.75,
            adj_r_squared: 0.74,
            f: 91.4,
            df_model: 1.0,
            df_resid: 30.0,
            f_p: 1.3e-10,
            sigma: Some(3.05),
            aic: Some(166.0),
            bic: Some(170.4),
        },
        diagnostics: Diagnostics {
            durbin_watson: Some(1.25),
            ..Diagnostics::default()
        },
        fitted: (0..32).map(|i| 10.0 + f64::from(i) * 0.7).collect(),
        residuals: (0..32).map(|i| (f64::from(i) * 1.3).sin() * 3.0).collect(),
    });
    let correlation = ComputedBody::Correlation(CorrelationBody {
        method: CorrelationMethod::Pearson,
        adjustment: PAdjust::Holm,
        variables: vec!["a".into(), "b".into()],
        r: vec![vec![Some(1.0), Some(-0.6)], vec![Some(-0.6), Some(1.0)]],
        p: vec![vec![None, Some(0.004)], vec![Some(0.004), None]],
        n: vec![vec![20, 20], vec![20, 20]],
    });
    let sheet = ComputedBody::CalcSheet(CalcSheetBody {
        lines: vec![CalcLine {
            name: Some("g".into()),
            expr: "9.81 m/s^2".into(),
            value: Some(9.81),
            display: None,
            unit: Some("m/s²".into()),
            latex: None,
            dimension: None,
            error: None,
            notes: Vec::new(),
        }],
    });
    let quantity = ComputedBody::Quantity(QuantityBody {
        from: Quantity {
            value: 1.0,
            unit: "in".into(),
            uncertainty: None,
        },
        to: Quantity {
            value: 25.4,
            unit: "mm".into(),
            uncertainty: None,
        },
        factor: 25.4,
        exact: true,
        dimension: "length".into(),
        chain: Vec::new(),
    });
    [describe, regression, correlation, sheet, quantity]
        .into_iter()
        .map(|body| (Computed::new("st_x", "stats.x", "Card", engine()), body))
        .collect()
}

/// What [`window`] sets up.
type Window<'a> = (
    &'a mut VisualTestContext,
    gpui::Entity<TimelineView>,
    Vec<Block>,
    Rc<RefCell<Vec<BlockEvent>>>,
);

/// A window showing `cards`, plus every block event emitted.
fn window(cx: &mut TestAppContext, cards: Vec<(Computed, ComputedBody)>) -> Window<'_> {
    cx.update(gpui_component::init);
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    let blocks: Vec<Block> = cards
        .into_iter()
        .map(|(envelope, body)| {
            Block::new(BlockKind::Computed {
                envelope: Box::new(envelope),
                body: Box::new(body),
            })
        })
        .collect();
    view.update(cx, |view, cx| {
        for block in &blocks {
            view.apply(
                &DomainEvent::BlockAdded {
                    block: block.clone(),
                },
                cx,
            );
        }
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
    draw(cx, &view);
    (cx, view, blocks, events)
}

fn draw(cx: &mut VisualTestContext, view: &gpui::Entity<TimelineView>) {
    let view = view.clone();
    cx.draw(
        gpui::point(px(0.), px(0.)),
        gpui::size(px(1000.), px(2400.)),
        move |_, _| div().size_full().child(view.clone()),
    );
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} not drawn"));
    cx.simulate_click(bounds.center(), Modifiers::none());
}

#[test]
fn badges_provenance_and_diffs_come_from_the_envelope() {
    let (mut c, _) = welch();
    assert_eq!(badge(&c), None);
    assert_eq!(
        provenance_text(&c),
        "crest-stats 0.15.0 · blake3:9c1e3a4b · 3 ms"
    );
    c.verification.matched = Some(true);
    c.verification.recomputed_at = Some("2026-10-24T10:00:00Z".into());
    assert_eq!(
        badge(&c),
        Some(Badge::Verified("2026-10-24T10:00:00Z".into()))
    );
    c.verification.matched = Some(false);
    c.verification.diff = vec![ValueDiff {
        key: "statistic".into(),
        old: Some(-3.767),
        new: Some(-3.5),
    }];
    assert_eq!(badge(&c), Some(Badge::Mismatch(1)));
    assert_eq!(
        diff_text(&c),
        "--- stored\n+++ recomputed\n@@ values @@\n-statistic = -3.767\n+statistic = -3.5\n"
    );
    assert!(!is_stale(&c));
    c.warnings.push(ComputedWarning {
        code: INPUT_CHANGED.into(),
        severity: WarningSeverity::Warn,
        text: "changed".into(),
    });
    assert!(is_stale(&c));
    assert_eq!(auto_decimals(6.03), 2);
    assert_eq!(auto_decimals(0.056), 4);
    assert_eq!(auto_decimals(1234.0), 0);
}

#[gpui::test]
fn draws_every_card_body(cx: &mut TestAppContext) {
    let mut cards = vec![welch()];
    cards.extend(other_cards());
    let (cx, view, _, _) = window(cx, cards);
    view.read_with(cx, |view, _| assert_eq!(view.state().len(), 6));
    for selector in [
        "computed-card",
        "computed-provenance",
        "computed-apa",
        "computed-sentence",
        "computed-recompute",
        "computed-assumptions",
        "regression-plots",
    ] {
        assert!(cx.debug_bounds(selector).is_some(), "{selector}");
    }
    // Both residual plots are drawn by the plot painter.
    let card = view.read_with(cx, |view, _| view.state().entries()[2].block.id);
    for name in ["residuals", "qq"] {
        let selector = format!("plot-canvas-computed-body-{card}-{name}");
        assert!(
            cx.debug_bounds(selector.clone().leak()).is_some(),
            "{selector}"
        );
    }
}

fn calc_line(name: &str, expr: &str) -> CalcLine {
    CalcLine {
        name: Some(name.into()),
        expr: expr.into(),
        value: None,
        display: None,
        unit: None,
        latex: None,
        dimension: None,
        error: None,
        notes: Vec::new(),
    }
}

fn calc_cards() -> Vec<(Computed, ComputedBody)> {
    let typeset = CalcLine {
        value: Some(462.963),
        display: Some("462.963 kJ".into()),
        unit: Some("kJ".into()),
        latex: Some(
            "E = 0.5 \\cdot m_{\\mathrm{car}} \\cdot v^{2} = 462.963\\,\\mathrm{kJ}".into(),
        ),
        dimension: Some("Energy or Torque".into()),
        ..calc_line("E", "0.5 * m_car * v^2 -> kJ")
    };
    let plain = CalcLine {
        value: Some(4409.25),
        display: Some("4409.25 lb".into()),
        unit: Some("lb".into()),
        dimension: Some("Mass".into()),
        notes: vec!["`ton` read as the metric tonne (1000 kg)".into()],
        ..calc_line("load", "2 ton -> lb")
    };
    let failed = CalcLine {
        error: Some("incompatible dimensions in addition: left hand side is Length, right hand side is Time".into()),
        ..calc_line("bad", "dist + 2 s")
    };
    let mut sheet = Computed::new("ca_sheet", "calc.sheet", "Calc sheet", engine());
    sheet.warnings.push(ComputedWarning {
        code: calc::LINE_ERROR.into(),
        severity: WarningSeverity::Warn,
        text: "line 3 (`bad`): incompatible dimensions".into(),
    });
    let mut converted = Computed::new("ca_conv", "calc.convert", "Unit conversion", engine());
    converted.values.insert(
        "result".into(),
        ComputedValue {
            unit: Some("cm".into()),
            ..shown(160.02, "160.02")
        },
    );
    let quantity = ComputedBody::Quantity(QuantityBody {
        from: Quantity {
            value: 63.0,
            unit: "in".into(),
            uncertainty: None,
        },
        to: Quantity {
            value: 160.02,
            unit: "cm".into(),
            uncertainty: None,
        },
        factor: 2.54,
        exact: true,
        dimension: "Length".into(),
        chain: vec![
            domain::computed::ConversionStep {
                from_unit: "in".into(),
                to_unit: "m".into(),
                factor: 0.0254,
                note: Some("exact".into()),
            },
            domain::computed::ConversionStep {
                from_unit: "m".into(),
                to_unit: "cm".into(),
                factor: 100.0,
                note: Some("exact".into()),
            },
        ],
    });
    vec![
        (
            sheet,
            ComputedBody::CalcSheet(CalcSheetBody {
                lines: vec![typeset, plain, failed],
            }),
        ),
        (converted, quantity),
    ]
}

#[gpui::test]
fn the_calc_sheet_typesets_lines_and_keeps_errors_on_them(cx: &mut TestAppContext) {
    let (cx, view, _, _) = window(cx, calc_cards());
    view.read_with(cx, |view, _| assert_eq!(view.state().len(), 2));
    for selector in [
        "calc-sheet",
        "calc-line",
        "calc-math",
        "calc-error",
        "calc-note",
        "quantity-exact",
        "quantity-chain",
    ] {
        assert!(cx.debug_bounds(selector).is_some(), "{selector}");
    }
    // The line error is shown on its line, not again among the warnings,
    // and unit readings are chips, not an assumption checklist.
    assert!(cx.debug_bounds("computed-assumptions").is_none());
    assert!(cx.debug_bounds("quantity-rounded").is_none());
}

#[gpui::test]
fn recompute_asks_the_session_and_shows_the_answer(cx: &mut TestAppContext) {
    let (cx, view, blocks, events) = window(cx, vec![welch()]);
    let id = blocks[0].id;
    click(cx, "computed-recompute");
    assert_eq!(*events.borrow(), [BlockEvent::Recompute { block: id }]);
    let card = cx
        .update(|_, cx| crate::blocks::existing_block_state::<ComputedCard>(id, cx))
        .unwrap();
    card.read_with(cx, |card, _| assert!(card.is_recomputing()));

    // The controller answers with the recomputed envelope: a mismatch.
    let (mut envelope, body) = welch();
    envelope.verification.matched = Some(false);
    envelope.verification.diff = vec![ValueDiff {
        key: "statistic".into(),
        old: Some(-3.767),
        new: Some(-3.5),
    }];
    let block = Block {
        id,
        kind: BlockKind::Computed {
            envelope: Box::new(envelope),
            body: Box::new(body),
        },
    };
    view.update(cx, |view, cx| {
        view.apply(&DomainEvent::BlockUpdated { block }, cx);
    });
    draw(cx, &view);
    card.read_with(cx, |card, _| {
        assert!(!card.is_recomputing());
        assert_eq!(badge(&card.envelope), Some(Badge::Mismatch(1)));
    });

    // A failure is shown on the card.
    click(cx, "computed-recompute");
    cx.update(|_, cx| recompute_failed(id, "file gone".into(), cx));
    card.read_with(cx, |card, _| {
        assert!(!card.is_recomputing());
        assert_eq!(card.error.as_deref(), Some("file gone"));
    });
}

#[gpui::test]
fn assumptions_open_when_one_fails_and_toggle(cx: &mut TestAppContext) {
    let (cx, _view, blocks, _) = window(cx, vec![welch()]);
    let card = cx
        .update(|_, cx| crate::blocks::existing_block_state::<ComputedCard>(blocks[0].id, cx))
        .unwrap();
    card.read_with(cx, |card, _| assert_eq!(card.assumptions, None));
    click(cx, "computed-assumptions");
    card.read_with(cx, |card, _| assert_eq!(card.assumptions, Some(false)));
}

/// One card per phase-2 body, with the selectors of the plots it draws.
fn phase_two_cards() -> Vec<(Computed, ComputedBody, &'static [&'static str])> {
    let card = |kind: &str, method: &str| Computed::new("st_p2", kind, method, engine());
    vec![
        (
            card("stats.distribution", "Distribution of mpg"),
            ComputedBody::Distribution(super::distribution::tests::body()),
            &["histogram", "box", "qq", "ecdf"],
        ),
        (
            card("stats.contingency_chisq", "Pearson's χ² test"),
            ComputedBody::Contingency(super::contingency::tests::body()),
            &[],
        ),
        (
            card("stats.power", "Power analysis: two-sample t-test"),
            ComputedBody::Power(super::power::tests::body()),
            &["curve"],
        ),
        (
            card("stats.ab_test", "A/B test: two-proportion z-test"),
            ComputedBody::AbTest(super::ab_test::tests::body()),
            &["rates"],
        ),
        (
            {
                let mut c = card("stats.bootstrap", "Bootstrap of the mean");
                c.seed = Some(1);
                c.rng = Some("chacha20".into());
                c
            },
            ComputedBody::Bootstrap(super::bootstrap::tests::body()),
            &["replicates"],
        ),
        (
            card(
                "stats.forecast",
                "Forecast: MSTL (periods 2) + AutoETS trend",
            ),
            ComputedBody::Forecast(super::forecast::tests::body()),
            &["forecast", "trend", "seasonal-2", "remainder"],
        ),
        (
            card("solve.lp", "Linear program"),
            ComputedBody::Optimization(super::optimization::tests::body()),
            &["region"],
        ),
        (
            card(
                "solve.equations",
                "Equation solver: sign-change scan with bisection",
            ),
            ComputedBody::Equations(super::equations::tests::body()),
            &["curve"],
        ),
        (
            {
                let mut c = card("solve.monte_carlo", "Monte Carlo simulation");
                c.seed = Some(7);
                c
            },
            ComputedBody::Simulation(super::simulation::tests::body()),
            &["histogram", "tornado", "convergence"],
        ),
        (
            card("solve.cash_flow", "Cash-flow analysis"),
            ComputedBody::CashFlow(super::cash_flow::tests::body()),
            &["npv"],
        ),
        (
            card("solve.decision_matrix", "Decision matrix"),
            ComputedBody::DecisionMatrix(super::decision_matrix::tests::body()),
            &[],
        ),
        (
            card("solve.bode", "Frequency response"),
            ComputedBody::Bode(super::bode::tests::body()),
            &["magnitude", "phase", "nyquist"],
        ),
        (
            card("solve.karnaugh", "Karnaugh map"),
            ComputedBody::Karnaugh(super::karnaugh::tests::body()),
            &[],
        ),
        (
            card("solve.beam", "Beam analysis"),
            ComputedBody::Beam(super::beam::tests::body()),
            &["shear", "moment", "deflection"],
        ),
    ]
}

#[gpui::test]
fn draws_the_phase_two_cards_with_their_plots(cx: &mut TestAppContext) {
    // One window per card: the timeline follows its end, so a long one
    // would not draw the first cards.
    for (envelope, body, names) in phase_two_cards() {
        let (cx, _view, blocks, _) = window(cx, vec![(envelope, body)]);
        assert!(cx.debug_bounds("computed-card").is_some());
        for name in names {
            let selector = format!("plot-canvas-computed-body-{}-{name}", blocks[0].id);
            assert!(
                cx.debug_bounds(selector.clone().leak()).is_some(),
                "{selector}"
            );
        }
    }
}
