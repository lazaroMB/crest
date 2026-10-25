# Chat blocks for technical work: research

This note records which new chat-block kinds crest should render so that developers and SREs, mechanical, civil and electrical engineers, architects, chemists and life scientists, physicists, mathematicians and data scientists can stop copying agent output into another tool. It collects six research notes (developers and ops, engineering and architecture, chemistry and life sciences, physics/math/data, statistics and verified computation, protocol landscape), all dated 2026-10-24. The report gives a design principle for blocks that carry numbers, the blocks every persona shares, a statistics catalogue, per-domain tables, interop formats, the widgets the blocks have in common, licensing flags and a roadmap.

Scope: new block kinds only. The existing blocks (markdown, text, tool call, diff, shell, table, chart, Mermaid, JSON tree, image, sources, thinking, stage header, `Custom` Lua node tree, 3D model view) are not proposed again. PDF, Jupyter, 3D and terminal are already on the [modal roadmap](rich-content-modals.md#roadmap). For what GPUI can and cannot draw (no webview, no native surfaces on Linux, atlas limits, the `RenderImage` path), see [rich-content-modals.md#gpui-constraints](rich-content-modals.md#gpui-constraints).

Versions and licenses are copied from the notes. Statistics and solver crates were read from the crates.io API on 2026-10-24. Everything else was checked on docs.rs or lib.rs where a source is cited. **[unverified]** marks a fact that came from model knowledge or a source that disagrees with another. Run `cargo deny` before adding any of these crates.

## Summary

The research supports one rule: **the model decides what to compute; a deterministic engine computes it; the block shows only engine numbers, each with its provenance.** Arithmetic is where LLMs fail most visibly. GPT-4 multiplies 3×3 digits correctly about 60% of the time and 5×5 digits 0% of the time ([arXiv 2407.15360](https://arxiv.org/pdf/2407.15360)). Its wrong answers keep the leading and trailing digits right, so they look plausible. Program-aided approaches gain 12–15 points ([PAL](https://arxiv.org/abs/2211.10435), [PoT](https://arxiv.org/abs/2211.12588)), but choosing the method is still weak (GPT-4o reaches 64.83% on StatQA test applicability). Running code also does not make an inference valid (P-Bench). None of the commercial tools surveyed (ChatGPT, Julius, Hex, Excel Copilot, Gemini in Sheets, Wolfram|Alpha) has per-number provenance or recompute-and-diff. That gap is where crest can stand out.

Top picks, in build order:

1. **`Computed` envelope + provenance chip + recompute/diff.** This is the base layer, not a visible block. Every numeric block carries one JSON envelope, and prose refers to its values only through slots. A lint flags any number the model wrote outside a slot. Cost M.
2. **LaTeX math via RaTeX** (inline and display, including mhchem) inside the existing markdown. This is the biggest gap for every persona. gpui-component and Zed both lack it, and inline math is a recurring Claude Code bug report. Cost M.
3. **Unit-checked calc sheet** (Mathcad style, evaluated by `numbat` 1.24.0). The thing engineers distrust most is LLM unit arithmetic (the PTC Mathcad test). Cost M.
4. **Statistics cards**: hypothesis test, regression/ANOVA and descriptive summary, laid out like JASP and `report`, with assumption checks as data, an APA line built from a template, and misuse warnings. Cost M per family, on top of 1.
5. **Form / approval block**: a native take on MCP Apps and elicitation, which crest cannot host as HTML on Wayland. It replaces free-text Q&A with typed inputs. Cost M.
6. **Function plot with sliders** painted natively, Desmos style. It replaces the "write matplotlib, run it, get a static PNG" loop. Cost M.
7. **Developer quick wins**: test report (JUnit XML), SARIF findings and ANSI colours in the shell block (cost S each), then trace waterfall, flame graph and structured log viewer (cost M each, sharing a timeline and a list widget).
8. **Validated molecule 2D block** (`chematic` 1.2.1). Parsing the SMILES and showing formula, MW and charge catches the SMILES errors LLMs make at a high rate (generation 43.0% on MolLangBench for GPT-5). Cost S–M.

Domain viewers (STEP, IFC, VCD waveform, DXF, plasmid map, dataset inspector) follow in phases 2–3. They are cheap once the shared widgets in [Reuse](#reuse) exist.

## Principle: computation you can verify

### Engine-computed numbers

The model writes a structured call (a Lua tool such as `stats.ttest{...}`, a calc-sheet DSL, a solver spec) or a script for a bridged engine. The engine runs it off the UI thread and returns an envelope. The renderer formats values from the envelope only. Narrative text reaches values through **number slots** (`⟦calc:7f3a.p_value⟧`), which the renderer replaces with the engine value at the envelope's precision. Plain-language sentences come from templates (as `report` and gtsummary do), never from the model.

Engines are layered:
- **Rust-native core** for what Rust covers well: distributions (`statrs` 0.19.1, MIT), descriptives and correlation (`ndarray-stats` 0.7.0, `polars` 0.55.2), OLS (`linregress` 0.5.4), linear algebra (`nalgebra` 0.35.0, `faer` 0.24.4), optimisation (`argmin` 0.11.0), LP/MILP (`highs` 2.4.0, `microlp` 0.6.0, `good_lp` 1.15.3), conic (`clarabel` 0.11.1), time series (`augurs` 0.10.2, MSTL with intervals), units (`numbat` 1.24.0, `fend` 1.5.9), money (`rust_decimal` 1.43.0), seeded RNG (`rand_chacha` 0.10.0), hashing (`blake3` 1.8.7). Welch t, Mann-Whitney, χ², Fisher, Kaplan-Meier, OLS with VIF, bootstrap and power are realistic to write on top of statrs. **Validate them against scipy and R reference values in a golden-file suite.** That matters more for credibility than which engine is used.
- **Subprocess bridge** to a Python or R the user has installed, for what Rust lacks: mixed models, emmeans/Tukey, sandwich SEs, full GLM diagnostics, Cox PH with Schoenfeld tests, validated MCMC (Stan, PyMC, brms), SARIMAX, non-central effect-size CIs. A Lua tool writes the inputs (Arrow IPC/Parquet for tables, JSON for parameters) to a temp dir and spawns `python -I -c <runner>` or `Rscript --vanilla runner.R` with a timeout and no shell interpolation. A fixed runner shipped by crest sets the seed, runs the script, and requires it to call `emit(envelope)`. The runner writes one JSON envelope to a result file and records engine versions, captured warnings, stderr and a `pip freeze`/`sessionInfo()` hash. Rust validates the result against the schema; if validation fails, the block is an error block showing raw stdout. A subprocess is preferred over `pyo3` 0.30.1 (crash and GIL isolation, no tie to a Python version). R is not bundled, because of GPL obligations.

### The `Computed` envelope

The envelope is versioned, defined with serde and checked with JSON Schema. It is a proposal: no existing standard covers per-number provenance for LLM statistics.

```json
{
  "crest_computed": "1.0",
  "id": "calc_7f3a",
  "kind": "stats.ttest_welch",
  "status": "ok",
  "values": {
    "t":         {"v": -3.767, "display": "−3.77", "sig": 3},
    "df":        {"v": 18.33, "display": "18.33"},
    "p_value":   {"v": 0.001374, "display": ".001", "fmt": "apa"},
    "mean_diff": {"v": -7.245, "unit": "mi/gal",
                  "ci": {"level": 0.95, "lo": -11.28, "hi": -3.21, "method": "welch_t"}},
    "cohens_d":  {"v": -1.48, "ci": {"level": 0.95, "lo": -2.27, "hi": -0.67, "method": "noncentral_t"},
                  "label": "large", "label_rule": "Cohen 1988"}
  },
  "uncertainty_kind": "frequentist_ci",
  "method": {"name": "Welch two-sample t-test", "why": "Levene p = .02 → unequal variances",
             "alternatives": ["student_t", "mann_whitney"], "params": {"alternative": "two-sided"}},
  "assumptions": [
    {"id": "normality_g1", "check": "Shapiro-Wilk", "stat": 0.94, "p": 0.38, "status": "pass"},
    {"id": "equal_var", "check": "Levene", "stat": 6.2, "p": 0.02, "status": "fail",
     "consequence": "used Welch correction"},
    {"id": "independence", "status": "not_checked", "note": "design property; confirm"}
  ],
  "warnings": [{"code": "small_n", "severity": "warn", "text": "n < 20 per group"}],
  "inputs": [{"name": "mtcars.csv", "role": "data", "hash": "blake3:9c1e…", "rows": 32,
              "columns": ["mpg", "am"], "filter": "none"}],
  "engine": {"name": "crest-stats", "version": "0.15.0", "backend": "statrs 0.19.1",
             "platform": "x86_64-linux"},
  "seed": null, "rng": null,
  "environment_hash": "blake3:…",
  "reproduce": {"lua": "stats.ttest{data='mtcars.csv', y='mpg', group='am', var_equal=false}",
                "python": "scipy.stats.ttest_ind(a, b, equal_var=False)",
                "r": "t.test(mpg ~ am, data = mtcars)"},
  "narrative": {"template": "apa_ttest", "text_slots": ["t", "df", "p_value", "cohens_d"]},
  "provenance": {"activity_started": "2026-10-24T16:40:00Z", "duration_ms": 3,
                 "agent": "assistant-turn-42", "requested_by": "user"},
  "verification": {"recomputed_at": null, "match": null, "diff": null}
}
```

- `values.*.v` stores the raw number. `display`, `sig` and `fmt` affect presentation only, so formatting never changes the stored value. Units come from the engine (`numbat`/`fend` parsing or `uom`), never from the model.
- `seed` and `rng` are required whenever `kind` involves resampling (bootstrap, Monte Carlo, MCMC, k-fold). The card also shows the Monte Carlo SE, so the user can tell sampling noise from a real difference.
- Exports: the envelope maps onto W3C PROV (Entity; the Activity `used` the inputs; the engine is the `SoftwareAgent`; `method` + `reproduce` form the `prov:Plan`; a rerun `wasDerivedFrom` the original). It also maps onto a Jupyter `display_data` bundle (`application/vnd.crest.computed+json` + `text/plain` APA line + `image/png`) and onto an RO-Crate Process Run Crate (one `CreateAction` per envelope). Crest exports to these formats and does not use them internally.

### Provenance chip

Every engine value shows a small chip with the engine and version, the BLAKE3 input hash and the elapsed time. Clicking it expands to the exact call (Lua tool and arguments, or the script) and a **Copy reproduce command** button (Lua, Python or R line). Blocks keep a reference to their data source (path + hash). When the file changes, the block is marked **stale** and offers recompute. This is the opposite of Gemini in Sheets, whose charts are static images that do not update.

### Recompute and diff

**Recompute** re-runs the same envelope. Each `values.*.v` is compared exactly for closed-form kinds and within k·MCSE for Monte Carlo kinds, and the result goes into `verification`. A match shows "Verified, reproduced at <time>". A mismatch opens a field diff (old against new, changed fields highlighted) built on the existing diff renderer. Because method choice is the weak step, the **why this test** trail (data types, number of groups, paired or not, assumption results) lists alternatives. Switching to one runs the engine at once and shows both results side by side.

### Assumption checks

Assumptions are part of the result schema, not prose. Each has a status (`pass`, `warn`, `fail`, `not_checked`) and a `consequence`. `not_checked` is shown on purpose. Independence, for example, is a design property only the user can confirm. This answers the P-Bench finding that agents report p-values without checking assumptions.

### Misuse lint

Deterministic rules, written in Lua so plugins can add more, attach `warnings` to the envelope:
- many tests in one session without correction (suggest Holm or BH and show the count);
- n < 30 together with a non-normality flag; an expected χ² count below 5;
- VIF above 10 or a large condition number (statsmodels' numbered notes are the model);
- p between .04 and .05 after several reruns with changed parameters (a p-hacking flag from session history);
- a sign flip between aggregate and stratified effects (Simpson's paradox);
- forecasts extrapolated beyond the training range; sample-ratio mismatch in A/B tests.

A second lint scans the model's free text for numeric literals that are not slots, quoted user input or whitelisted (years, ordinals). It underlines them in amber as **unverified number**. A regex plus a whitelist makes this the cheapest strong guard.

### "Model said" vs "engine computed"

When the model wrote an interpretation, the card offers a two-column toggle: the narrative with its slots filled on the left, the raw engine output (envelope JSON, or statsmodels-style text) on the right. Any number the model restated outside a slot is compared with the engine value, and mismatches are highlighted. The same split works outside statistics. Chemistry blocks say "formula computed from SMILES" or "stated by model". A material card shows its source ("from MMPDS / datasheet X") or "unverified". A safety card renders only fetched PubChem data.

There is no user study yet showing that "verified" badges make trust better calibrated rather than causing overtrust. Badges should state what was checked, not just "verified".

## Cross-cutting blocks

| Content | Options considered | Chosen | Phase | Cost |
| --- | --- | --- | --- | --- |
| LaTeX math (inline, display, mhchem) | RaTeX (MIT, KaTeX-compatible, >99.5% syntax claimed, mhchem; crates.io version [unverified]); `latex-rust` 3.0.1 (MIT OR Apache-2.0, STIX Two OFL-1.1, MSRV 1.99, yanked 1.0.x); `katex-rs` 0.2.4 (HTML+MathML output, unusable); ReX (stale); MathJax in QuickJS (no crate) | RaTeX through a gpui-component `MarkdownPlugin` pre-pass, rendered to `RenderImage` off-thread; latex-rust as fallback; parse errors show a monospace chip with the error and "Copy LaTeX" | 1 | M (2–3 weeks; baseline alignment is the hard part) |
| Math painted as glyphs | RaTeX display list drawn with GPUI text + quads | later, for crisp zoom and theme colour; needs KaTeX fonts (MIT [unverified]) registered | 3 | L |
| Typst document / diagrams | `typst` 0.15.1 + `typst-render` 0.15.0 / `typst-svg` (license Apache-2.0 [unverified]); tectonic for full LaTeX (not researched) | Typst for ```` ```typst ```` and `.typ` previews (pages as images, errors mapped to spans); `@preview` packages behind a setting; also covers cetz, quill, Feynman diagrams | 3 | M |
| Unit-checked calc sheet | `numbat` 1.24.0 (MIT OR Apache-2.0); `fend` 1.5.9 (MIT); `kalk` 3.2.3 (MIT); `rink-core` 0.9.0 (MPL-2.0); `uom` 0.38.0 (compile-time only) | named lines `name = expr → value unit` evaluated by numbat as a reactive DAG; dimension errors inline; Wolfram-style "interpreting 'ton' as metric tonne" assumption chips; LaTeX via RaTeX; `Computed` envelope per sheet | 1 | M |
| Unit conversion / quantity check | same engines; CODATA constants (crate not researched) | card with factor chain, exact vs rounded flag, dimension vector, uncertainty (linear propagation or MC) | 1 | S |
| Forms and approvals | MCP Apps (HTML, impossible on Wayland); A2UI; Adaptive Cards; MCP elicitation `requestedSchema`; Lua DSL | native form from a JSON Schema subset (gpui-component inputs, selects, checkboxes; `jsonschema` [unverified]); submit becomes a tool call or user turn; approval variant with diff preview, approve / deny / edit args / "always allow" scope | 1 | M |
| Plan / checklist with live progress | AG-UI state snapshot/delta events as reference | checklist driven by block update events | 2 | S |
| Function plots with sliders | `plotters` 0.3.7 (MIT, raster, version [unverified] for 2026); own GPUI painter; evaluators `meval`/`evalexpr`/`exmex` [unverified] | own painter for explicit, parametric and implicit (marching squares) curves; sliders re-evaluate at 60 fps; contour/heatmap rasterised off-thread; vector fields as paths; z=f(x,y) as a mesh in the 3D view; plotters only for static export | 1 | M (S each for vector field, surface) |
| ODE / phase portrait | own RK4/RK45; `ode_solvers` [unverified]; `peroxide` 0.43.1 | expressions only (no code), background thread, click to seed trajectories; 1D/2D heat/wave as heatmap | 3 | M |
| Dataframe / Parquet / Arrow / CSV preview | `arrow`/`parquet` (Apache-2.0, versions [unverified]); `polars` 0.55.2 (MIT, heavy build) | arrow + parquet: schema, row-group stats, first N rows in the table block, per-column type / null % / distinct / histogram sparkline; polars behind a feature | 1 | S–M |
| Large-array summary (npy, tensors) | `ndarray-npy` [unverified] | shape, dtype, min/max/mean/NaN count, sparkline or thumbnail | 2 | S |
| Maps | georust `geo`, `geojson`, `shapefile`, `kml`, `geozero` (MIT/Apache [unverified]); `proj4rs` 0.1.10 (pure Rust, 2D only, no WKT; license [unverified]); `proj` (C FFI) | vector layers over XYZ raster tiles fetched and decoded off-thread; EPSG coordinate readout via proj4rs, proj optional; OSM tile usage policy must be respected | 2 | M |
| Gantt | Mermaid Gantt (static, trusts LLM dates); own CPM | own Gantt that computes CPM and slack from durations, critical path highlighted; MS Project XML via `quick-xml` [unverified] | 2 | S–M |
| Decision matrix | own | editable normalised weights, weighted totals, rank, weight-sensitivity ("how far before rank 1 changes"), tie warning; envelope-backed | 2 | S |
| Export / copy-as | — | every block: copy as TSV/CSV (tables), SVG/PNG/Vega-Lite JSON (charts), source (SMILES, LaTeX, DOT), "ask about selected row" | 1 | S |
| Promote to artifact | Claude's inline-visual vs Artifacts split | transient flag on blocks; "promote" makes a versioned, diffable artifact in a side panel | 3 | M |

Notes:
- **Math integration.** Pull `$..$`, `$$..$$`, `\(..\)`, `\[..\]` and ```` ```math ```` fences out *before* CommonMark parsing, so `_` and `\` survive. That delimiter mangling is the common failure in GitHub, GitLab, Cursor and Gemini. Cache by (source, font size, colour, scale). Inline math needs RaTeX's box height and depth to sit on the text baseline. gpui-component's maintainer declined built-in math (#2424) and pointed apps to `MarkdownPlugin`, so crest owns this code.
- **Forms** close the loop that MCP Apps, Adaptive Cards `Action.Execute` and A2UI actions all use: inputs → action → refreshed block. They also answer Nielsen's "articulation barrier" critique, along with targeted edits ("revise this" on a selected span or block).

## Statistics blocks

Every statistics block wraps a `Computed` envelope. All cards share one anatomy, taken from JASP/jamovi (GUI), `report`/gtsummary (text and tables) and the statsmodels `summary()` footer:

1. **Header**: test or model name and why it was chosen ("Welch t, because Levene p = .02"), plus the provenance chip and a stale/verified badge.
2. **Assumption checklist**: one row per check with statistic, p and a pass/warn/fail/not-checked chip. Expandable, open when anything fails.
3. **Results table**: statistic, df, p, effect size with CI, CI of the difference. Numbers are right-aligned in a tabular-figure font.
4. **Plain-language sentence** from a template, with values in slots.
5. **APA line** with a copy button. The APA 7 formatter applies the rules: italic symbols, no leading zero for statistics bounded by 1, exact p to 2–3 decimals and "p < .001" below that, one decimal for M/SD on integer scales, two for others, no thousands separator in df.
6. **Misuse warnings** and numbered footnotes in the statsmodels style ("[1] Standard errors assume…").
7. **Sensitivity row**: the fallback test (Mann-Whitney, Wilcoxon) when an assumption fails, plus one-click alternatives.
8. Footer: **model said / engine computed** toggle, recompute, copy reproduce command, export (CSV, PNG, Jupyter bundle).

APA also says not to repeat statistics in both the text and a table. The narrative slots should therefore refer to the card instead of restating the whole table.

| Content | Options considered | Chosen | Phase | Cost |
| --- | --- | --- | --- | --- |
| Descriptive summary | `polars` 0.55.2, `ndarray-stats` 0.7.0; gtsummary layout | per-variable n, missing, mean, SD, median, IQR, min/max, skew, kurtosis, sparkline; counts and % for categoricals; `by=` columns | 1 | S |
| Distribution plot | own painter; existing chart block | histogram with binning rule and width stated (Freedman-Diaconis/Sturges), KDE with bandwidth method, box/violin with 1.5×IQR outliers, QQ with band, ECDF with DKW band | 2 | M |
| Hypothesis test card | `statrs` 0.19.1 (a `stats_tests` module [unverified]); `hypors` 0.4.1 (license "non-standard", inspect before use); bridge to scipy | native Welch/Student t, paired t, Mann-Whitney, Wilcoxon, χ², Fisher 2×2 on statrs; others via bridge | 1 | M |
| Regression / ANOVA / GLM | `linregress` 0.5.4; `linfa` 0.8.1; `smartcore` 0.6.16; statsmodels via bridge | coefficient table (estimate, SE, t/z, p, CI, standardised β), fit block (R², adj. R², F/LR χ², AIC, BIC, N), diagnostics footer (Durbin-Watson, Jarque-Bera, Breusch-Pagan, condition number, max VIF, max Cook's D), linked residual plots; ANOVA states SS type I/II/III; GLM headers say OR/HR by link; OLS native, GLM via bridge | 1 (OLS), 2 (GLM) | M |
| Correlation matrix / confusion matrix | own heatmap | diverging palette, r in cells, method stated, Holm/BH stated, pairwise n when data is missing | 1 | S |
| Contingency table | own | observed + expected counts, row/col %, standardised-residual heatmap, χ² or automatic Fisher (stated) when an expected count < 5, Cramér's V or OR with CI | 2 | S |
| Power calculator | statrs non-central t/F | solve for one of n / power / effect / α; editable inputs recompute; power curve | 2 | S |
| A/B test card | own | per-arm n and rate, absolute and relative lift with CI, z or Fisher, achieved power, SRM χ², peeking warning, optional P(B > A) | 2 | S |
| Bootstrap result | `rand_chacha` 0.10.0 | distribution histogram with observed statistic, B, seed, CI method (percentile/BCa/basic), MCSE | 2 | S |
| Bayesian posterior | `rv` 0.20.1; `bayes-rs` 0.3.0 (too young); Stan/PyMC/brms via bridge | SEXIT layout: median, 95% HDI/ETI, probability of direction, ROPE %, BF if any, Rhat/ESS, prior-sensitivity strip | 3 | M |
| Time-series decomposition / forecast | `augurs` 0.10.2 (MSTL with intervals; `ets`, `prophet`, `changepoint`); SARIMAX via bridge | stacked trend/season/remainder panels, 80/95% fan, hold-out MAPE/sMAPE, extrapolation warning | 2 | M |
| Survival (Kaplan-Meier) | native KM; lifelines/R survival via bridge | step curves with CI bands and censor ticks, number-at-risk table, median with CI, log-rank, Cox HR from bridge | 3 | M |
| ML training curves | own painter | multi-series loss/metric vs step, log axes, EMA smoothing, live tail from file/TensorBoard/W&B CSV | 2 | S–M |
| Equation / system solver | `peroxide` 0.43.1; `argmin` 0.11.0; `roots` 0.0.8 (stale) | solution, ‖F(x)‖ and per-equation residuals, method, iterations, tolerance, condition number, plug-back row (lhs, rhs, difference) | 2 | S–M |
| Optimisation result (LP/MILP/QP) | `highs` 2.4.0 (duals; ranging in the Rust wrapper [unverified], may need `highs-sys`); `microlp` 0.6.0; `good_lp` 1.15.3; `clarabel` 0.11.1; `russcip` 0.11.0 | status, objective, MIP gap and bound, variables with reduced cost, constraints with slack/binding/dual, ranging where available, template line ("'labor' is binding; one more hour is worth $12.50"); IIS not available in Rust crates checked | 2 | M |
| Monte Carlo / tornado / Fermi estimate | own on `rand_chacha` | input distributions, P5/P50/P95, mean ± MCSE, N, seed, convergence trace; tornado sorted by swing; Fermi tree of lognormal ranges with per-leaf sources | 2 | S–M |
| Cash flow / engineering economics | `rust_decimal` 1.43.0 | period table, NPV, IRR (warning for multiple sign changes), MIRR, payback, NPV-vs-rate curve with IRR crossing | 2 | S |
| Derivation / step-by-step | RaTeX; SymPy via bridge; Wolfram Show Steps API; `symbolica` (commercial license, flag) | ordered steps (LaTeX + justification + "verified by SymPy ✓/✗" badge), collapsible, copy as LaTeX/SymPy | 2 | S–M |

## Per-domain blocks

Most domain blocks should be **Lua-plugin `Custom` blocks** built from the shared widgets in [Reuse](#reuse), plus a parser. Only the cross-cutting blocks above need new `BlockKind` variants. Output can be recognised automatically by sniffing: JUnit `<testsuites>`, SARIF `$schema`, folded-stack lines, `"Plan":` in EXPLAIN JSON, HAR `log.entries`, `digraph`, a SMILES-shaped fence. With a sniffer, the agent gets a rich block without calling a special tool. Lua plugins register more sniffers.

### Developers & ops

The evidence is copy-out workflows. Users ask for ER/UML diagrams (Claude Code #13600, #14375) and upload plans to explain.dalibo.com, and profiles, traces and HAR files go to pprof, speedscope, Jaeger and DevTools. Each block gets a one-line summary header (status, counts, duration), collapses to it, and acts on the whole block, as Warp's blocks do.

| Content | Options considered | Chosen | Phase | Cost |
| --- | --- | --- | --- | --- |
| ANSI colours in shell/tool output | `vte` (Apache/MIT [unverified]); `anstyle-parse`; `alacritty_terminal` | SGR + OSC 8 into styled runs, shared with the planned log viewer and terminal | 1 | S |
| Test report | `quick-junit` 0.7.0 (now deserialises; MIT/Apache [unverified]); `junit-parser` 1.6.0 | summary bar, suite → case tree, failure with clickable stack frames, duration ranking, "re-run failed" / "ask agent to fix"; JUnit XML, TAP, Go `test2json` | 1 | S |
| SARIF / lint findings | `serde-sarif` 0.8.0 (MIT [unverified]); `sarif_rust` | grouped by rule and file, severity badges, highlighted snippet, `fixes` as applicable diffs via the diff block; also clippy/eslint JSON | 1 | S |
| Trace waterfall | `opentelemetry-proto` (Apache-2.0; version 0.30–0.32 depending on source [unverified]) + `prost`/`serde_json` | span bars on a time axis, nested, critical path and errors highlighted, attribute panel; OTLP, Jaeger, Zipkin; also crest's own agent turns (tools, durations, tokens) | 2 | M |
| Flame graph / profile | `inferno` 0.12.8 collapse (CAL-1.0 [unverified]); own folded-stack parser; pprof via `prost-build` (no parser crate) | native icicle/flame with zoom, search, differential mode; reuses the phase-3 modal flame painter; folded, perf, pprof, speedscope, Firefox Profiler (custom serde) | 2 | M |
| Structured log viewer | `serde_json`, `logfmt` [unverified], `regex`, `jiff` | virtual list, level colours, JSON field columns, filter chips, time histogram, follow mode, pattern grouping; shares the modal log viewer | 2 | M |
| SQL plan tree | hand-written serde per engine (no maintained crate) | PEV2-style tree, per-node time/rows bars, estimate-vs-actual flags, seq-scan and sort-spill warnings; Postgres JSON first, MySQL/SQLite/DuckDB later | 2 | M |
| HTTP / HAR inspector | `har` 0.9.0 (MIT; lib.rs shows 0.8.1), `http`, `httparse` | request list + waterfall, headers/body (JSON tree)/timing/cookies tabs, copy as curl, replay | 2 | S–M |
| Terraform / IaC plan | `plan -json` with serde | create/update/destroy counts plus attribute diffs | 2 | S–M |
| Graphviz / DOT and ER/schema | `layout-rs` 0.1.3 on docs.rs vs 0.1.2 on lib.rs (no clusters, no HTML labels; MIT [unverified]); `graphviz-rust` (shells out); libgraphviz (EPL-1.0); `petgraph` + `rust-sugiyama` [unverified]; `sqlparser` 0.61.0 (Apache-2.0) for DDL | layout-rs for layout data, painted natively with pan/zoom/focus; ER from DDL, Prisma, dbt manifest, with migration diff | 2 | M (L for big interactive graphs) |
| Benchmark comparison | `serde_json` (criterion `estimates.json`, hyperfine, benchstat, pytest-benchmark) | baseline vs new, % change with CI, regression flags, box/violin via chart; envelope-backed | 2 | S |
| Coverage | `lcov` 0.8.0; `quick-xml` (Cobertura); `lcov-parser` 3.2.2 (stale) | per-file bars, patch coverage, source gutters | 3 | S–M |
| Git graph / blame | `gix` (MIT/Apache [unverified]); `git-graph` lanes (MIT [unverified]); gitoxide `tix` as reference | lane-drawn history, commit click → diff block, blame with age colours | 3 | M |
| CI pipeline, Kubernetes, OpenAPI | `octocrab`, `kube`/`k8s-openapi`, `openapiv3`/`oas3`, `serde_yml` (all [unverified]; `serde_yaml` deprecated) | Lua-plugin adapters on the graph and tree widgets: CI DAG with job logs, k8s resource tree with events, endpoint cards with "try it" into HAR and spec diff | 3 | M each |
| Regex, cron, file tree, hex | `regex-syntax`/`fancy-regex`, `railroad`, `croner`/`cron`, `chrono-tz`/`jiff`, `ignore`/`walkdir`, `goblin` (all [unverified]) | regex AST + live highlights; cron next-N fires in several zones with DST warnings; tree with git badges and "touched this turn"; hex reuses the modal viewer | 3 | S each |

### Engineering & architecture

The closed loop matters most: the model writes code (CadQuery/build123d, SKiDL, SPICE, a calc script), a tool runs it, and crest renders the **real artifact** for the engineer to check by eye. A picture the model invented is worth much less than one rendered from a real file or solver output.

| Content | Options considered | Chosen | Phase | Cost |
| --- | --- | --- | --- | --- |
| Beam / frame diagram | own 2D direct-stiffness solver on `nalgebra` 0.35.0; SkyCiv/ClearCalcs round-trip | structure, supports, loads, SFD/BMD/deflection with max values; envelope-backed | 2 | M |
| Tolerance stack-up | own + chart | ± chain, worst-case / RSS / Monte Carlo histogram, ranked contributors | 2 | S |
| Material data card | own (no curated permissive DB found) | E, ν, σy, σu, ρ, CTE, temperature curve, **mandatory source field** or "unverified" | 2 | S |
| Digital waveform (VCD/FST/GHW) | `wellen` 0.20.1 (BSD-3, Surfer's backend); Surfer (EUPL-1.2, reference only) | native signal lanes on the timeline widget, cursor, hex bus values, model-set markers | 2 | M |
| Timing diagram (WaveDrom) | `wavedrom` 0.1.0 (quiet since June 2023; license check) | WaveJSON → SVG → `RenderImage` | 1 | S |
| Netlist / connectivity | `kiutils_kicad` (license not confirmed); own SPICE parser | net table + component↔net graph, unconnected pins flagged; addresses "LLMs can't trace wires" | 2 | S–M |
| Bode / Nyquist / root locus | own evaluation of a transfer function; plot painter | log-frequency axes, dB/deg dual axes, gain and phase margins annotated | 2 | S |
| Truth table / K-map | own Quine-McCluskey | grouped implicants and minimised expression computed, not recalled | 2 | S |
| Schematic sheet, PCB layer view | `kiutils_kicad`, `kiutils-rs`, `kiparse`; Gerber crates [unverified] | later; symbol-library resolution and faithful look are the cost | 3 | L |
| SPICE result | own ngspice `.raw` parser; ngspice via subprocess (license [unverified]) | `.tran`/`.ac` traces on the plot painter | 3 | M |
| STEP/IGES part (3D view extension) | `truck` (Apache-2.0: `truck-stepio`, `truck-meshalgo`; STEP coverage undocumented); foxtrot (PoC, license [unverified]); OCCT via `cadrum` 0.4.4 / opencascade-rs (LGPL-2.1) | truck in-process; OCCT only as an optional out-of-process plugin; face/edge/solid tree, bbox, mass/volume, face picking | 2 | M (real-file spike first) |
| Section / clip plane + measure | own | clip plane and point-to-point / thickness measure in the 3D view | 2 | M |
| FEM result field | `vtkio` 0.6.3 (MIT/Apache, low maintenance); CalculiX `.frd` [unverified] | scalar colouring, colour bar, deformation scale, min/max probes | 3 | M |
| 2D drawing (DXF/DWG) | `dxf` 0.6.0 (license [unverified] MIT); `acadrust` 0.4.1 (MPL-2.0, DWG R13–R2018, tests synthetic); LibreDWG (GPLv3+) | `dxf` first with layers, pan/zoom, entity hover; acadrust for DWG; LibreDWG rejected; revision overlay diff later | 2 (DXF), 3 (DWG) | M / M–L |
| IFC model + room schedule + storey plan | `ifc-lite-core` (MPL-2.0; native Rust use needs a spike; `ifc-lite-wasm` 2.3.0, `ifc-lite-ffi`); `ifc_rs` 0.1.0-alpha.9; IfcOpenShell (LGPL-3 [unverified]) | IFClite: spatial tree, IfcClass filter, property sets; IfcSpace schedule from the parser (no LLM areas); storey plan by mesh slicing | 2 (schedule), 3 (model, plan) | S / M |
| BCF issue card, IDS report | `zip` + `quick-xml`; IFClite | issue with viewpoint restoring the IFC camera; pass/fail per requirement | 3 | S–M / M |
| Map extras: CRS card, terrain profile, point cloud, LandXML | `proj4rs`/`proj`; `tiff`/`geotiff` [unverified]; `las` 0.7.4 + `laz`; `e57` [unverified] | CRS card in phase 2 (S); profile with grade and cut/fill; points rasterised with LOD into the 3D view | 2–3 | S–M |
| BOM with costing, traceability matrix | own tree-table; ReqIF crate not found | qty × cost roll-ups with missing-MPN flags; requirement × test grid with gaps | 2 | S / S–M |
| P&ID | DEXPI (no Rust crate) | deferred | — | L |

### Chemistry & life sciences

Validation is the value and the drawing comes second. Models misread SMILES they are given and output invalid or unreasonable ones (Guo et al.; MolErr2Fix). ChemCrow found that expert graders caught GPT-4's factual errors while an LLM judge preferred its fluent answers. A picture with a computed formula shows errors that fluent text hides. Structure *editing* (Ketcher is a web editor) is out of scope. Blocks copy as SMILES/MOL for ChemDraw or Ketcher.

| Content | Options considered | Chosen | Phase | Cost |
| --- | --- | --- | --- | --- |
| Molecule 2D + validation strip | `chematic` 1.2.1 (MIT OR Apache-2.0, pure Rust, `depict_svg`, very frequent releases); `cosmolkit` 0.5.0 (MIT, pre-1.0, InChI possibly via C FFI); `rdkit`/`rdkit-sys` 0.4.8 (RDKit BSD-3 [unverified], heavy C++ build); `smiles-rs`, `opensmiles` (parse only); OpenBabel (GPL-2 [unverified]) | `chematic` behind a `Depictor` trait (cosmolkit/RDKit swappable); parse error with column, formula, MW, exact mass, charge, R/S, InChIKey, canonical SMILES; name beside drawing; SDF grid. Benchmark ~50 hard molecules against RDKit first | 1 | S–M |
| Molecule grid / SAR table | chematic descriptors | depiction cells + MW, cLogP, TPSA, HBD/HBA, Lipinski flags, substructure highlight | 2 | S |
| Reaction scheme | chematic reaction SMILES / SMIRKS | reactants → conditions → products, per-element and charge balance, missing byproduct flags | 2 | M |
| mhchem equation | RaTeX mhchem (`\ce`, `\pu`); no mhchem crate | RaTeX plus an element-tally balance check | 1 | S |
| Units / concentration calculator | numbat; calc sheet | mass ↔ mol ↔ molarity ↔ volume, C1V1 = C2V2, serial dilution, buffer; MW taken from the parsed structure | 1 | S–M |
| Well-plate map | own (JSON/CSV) | 24/96/384/1536 grid, labels, concentration heatmap, legend, copy as CSV | 1 | S |
| Sequence | `gb-io` (MIT); `rust-bio`, `noodles` (MIT [unverified]) | ruler, frames, GenBank features, GC%, MW/pI, revcomp, restriction sites; flags non-IUPAC chars, frame shifts, premature stops | 2 | S–M |
| Plasmid map | gb-io + GPUI paths | circular and linear map, feature arcs, unique cutters highlighted | 2 | M |
| Spectrum (NMR/IR/UV-Vis/MS/XRD) | `mzdata` 0.67.4 (license not shown, Apache-2.0 [unverified]; Thermo RAW may need .NET); `openmassspec-core` 1.2.0 (Apache-2.0); own JCAMP-DX (~500 lines; jcampconverter license CC BY-NC-SA vs MIT conflict, do not copy) | plot painter with reversed ppm axis, min/max decimation, peak picking, 1H integration, predicted vs experimental overlay | 2 | M |
| 3D molecule / protein | `pdbtbx` (MIT, maintainer wanted); `pdbrust`; cosmolkit/chematic coords; gemmi (MPL-2.0 [unverified]) | ball-and-stick as instanced spheres and cylinders in the 3D view, pLDDT/B-factor/chain colouring; cartoon later | 2 (ball-and-stick), 3 (cartoon) | M / L |
| Safety / GHS card | PubChem PUG-REST via Lua tool; no crate | pictograms, signal word, H/P statements from embedded fixed texts; source and date always shown; **never model-written**, otherwise "unverified" | 2 | M |
| Primer / oligo, titration, element card | own SantaLucia 1998 NN Tm (Primer3 GPL-2 [unverified]); closed-form pH solver; `mendeleev`, `elements_rs` | engine-computed Tm/ΔG, pH curve and speciation, element tile and formula highlight | 3 | S each |
| Crystal / CIF, MSA, phase diagram, gel | own CIF/STAR parser + symmetry table; own MSA parser; polygon regions | unit cell with supercell and density check; virtual MSA grid; T–x regions; simulated gel | 3 | M / M / M / S |

### Physics, math & data

| Content | Options considered | Chosen | Phase | Cost |
| --- | --- | --- | --- | --- |
| HDF5 / NetCDF inspector | `hdf5-pure` 0.9.0 (pure Rust); `hdf5-core`/`hdf5-reader` 0.9.0 (HDF5 and NetCDF, MSRV 1.81); `hdf5-metno` 0.14.1 (C libhdf5) | pure-Rust readers: group/variable tree, shapes, dtypes, attrs, summary stats, slice heatmap/line | 2 | M |
| FITS image + header | `fitsrs` 0.4.1 (read-only); `fitrs` 0.5.0 (experimental) | fitsrs | 3 | S–M |
| ROOT TTree browser | `oxyroot` 0.1.25 (slow maintenance) | deferred until there is demand | — | M |
| Lean proof state | Lean LSP `$/lean/plainGoal`; Pantograph/REPL | goal cards `h : P ⊢ Q` in Unicode, step diff, diagnostics coloured as in Lean Copilot | 3 | M |
| Network graph | `petgraph` + `fdg`/`forceatlas2` [unverified]; GraphML, DOT, NetworkX JSON | force layout on the graph widget with pan, zoom, drag | 2 | M |
| Quantum circuit | `openqasm` (2.0, no visualisation); Qiskit `openqasm3_parser`; `sirraya-qutub-transpiler` (immature) | parse OpenQASM 2/3, draw wire grid natively; Bloch sphere in 3D view (S); Feynman via Typst | 3 | M |
| Model architecture graph | ONNX via `prost`; rusty-mermaid / layout-rs layered layout | graph widget | 3 | M–L |
| Geometry construction | own (GeoGebra-like) | deferred; Typst/cetz for static figures | — | L |

## Interop

The industry's rich layer is web-first: Claude Artifacts and inline visuals, ChatGPT Canvas and Apps SDK, Gemini Canvas and MCP Apps all render HTML in a sandboxed iframe. Crest should be the native host that renders the **declarative** formats faithfully, and should degrade cleanly for HTML-only content.

- **A2UI** (Google, Apache 2.0; v0.9.1 standardised `application/a2ui+json`, v1.0 candidate adds `actionResponse` RPC; sources disagree on whether v0.8 or v0.9.1 is current). Agents may only use components from a catalog the client approves, which matches crest's no-webview stance. Mapping: the `Custom` `CustomNode` tree (`Text`, `Row`, `Col`, `List`, `Table`, `Chart`, `Code`, `Markdown`, `Image`, `Html`, `Json`, `Mermaid`, `Link`) is in effect a private A2UI. Align its vocabulary and data binding with A2UI v0.9/v1.0, add the form inputs (TextField, ChoiceSet, Button) as new `CustomNode` variants, and accept `application/a2ui+json` as a `BlockKind::Custom` producer. Its full basic catalog and binding syntax were not retrieved, and no Rust renderer exists.
- **Adaptive Cards** (schema ≥1.4, `Action.Execute`). A second inbound format for the form/approval block: inputs plus `data` merged into a tool call, the reply replaces the card. Maps to the form `BlockKind`. Whether Microsoft 365 Copilot renders agent-emitted cards in chat is unconfirmed.
- **Vega-Lite via `vl-convert-rs`** (BSD-3). It runs the official Vega/Vega-Lite JS in an embedded V8 and outputs SVG, PNG or the Vega scenegraph with no browser. Mapping: `application/vnd.vegalite.v5+json` becomes a `BlockKind::Chart` when the spec fits the existing chart renderer. Otherwise it becomes an image rendered off-thread, and later the scenegraph could drive native hover and tooltips. V8 is a large dependency, so put it behind a feature (binary size not measured).
- **Jupyter MIME bundles.** The inbound format for tool results: pick the richest type crest renders (`application/vnd.crest.computed+json` → stats/calc block, Vega-Lite → chart, `image/png`/`image/svg+xml` → image, `text/markdown`/`text/latex` → text with math, `text/plain` fallback). `application/vnd.plotly.v1+json` and ipywidgets are not covered. The same mapping is the export path, and serves the planned Jupyter modal.
- **MCP Apps** (SEP-1865, Final; production since 2026-01-26). The UI is a predeclared `ui://` resource of MIME `text/html;profile=mcp-app` referenced by `_meta.ui.resourceUri`; other content types are "reserved for future use". **It is HTML only, so crest cannot run it on Wayland.** Crest can still be a partial host. It renders the tool's `structuredContent` with native blocks, honours `_meta.ui.visibility: "app"`, and offers "open in browser": a local server serves the `ui://` HTML with a JSON-RPC bridge over WebSocket. A headless screenshot is a later option. The "future content types" slot is where crest could propose a declarative profile such as A2UI.
- **`structuredContent` / `_meta` split.** The model reads `structuredContent` verbatim, while `_meta` is UI-only and never sent to the model. Crest Lua tools should return the same split: a compact model-facing summary (envelope `values` with `display`, the APA line, counts) and a UI-only payload (full tables, series, meshes). Large outputs then cost no tokens, and the model can still cite slots.
- **AG-UI** is a transport and event layer, not a UI format. Its taxonomy (run lifecycle, text deltas, tool-call streaming, state snapshot/delta) is a reference for crest's block-update events and live checklists.

## Reuse

Most of the cost is in a few widgets. Build them once as GPUI components, expose them as `CustomNode` variants so Lua plugins can compose domain blocks, and each block becomes mostly a parser plus a mapping.

| Widget | Used by | Notes |
| --- | --- | --- |
| Virtual timeline / bar canvas | trace waterfall, flame graph, HAR waterfall, waveform, Gantt, cron timeline, benchmark and CI durations, KM number-at-risk axis | rows virtualised like `DataTable`; time axis with zoom and pan; quads via `paint_quad`; shared with the phase-3 modal flame graph |
| Filterable virtual list | logs, test cases, SARIF findings, HAR requests, MSA rows | `uniform_list`, filter chips, regex, follow mode; shares the modal log viewer's SGR parser |
| Node-link graph | DOT, ER, lineage, CI DAG, git lanes, k8s tree, netlist, network, ONNX | layout from `layout-rs` / Sugiyama / force crates as data, painted natively; pan, zoom, focus neighbourhood |
| Extended JSON tree | plan nodes, OpenAPI schemas, span attributes, k8s objects, envelope raw view, IFC property sets | existing collapsible tree plus typed columns, badges and inline bars |
| Plot painter | function plots, distribution/QQ/ECDF, regression diagnostics, Bode, spectra, training curves, titration, forecasts | linear/log/reversed axes, dual axes, CI bands, annotations, min/max decimation; envelope-aware tooltips |
| SVG → `RenderImage` via resvg | WaveDrom, molecule SVG, RaTeX/latex-rust SVG, layout-rs SVG, Typst pages, Vega-Lite | resvg/usvg 0.46 is already in GPUI's tree; render off-thread, cache by (source, scale, theme) |
| Provenance chip + envelope inspector | every numeric block | one component: chip, recompute, diff, model-said toggle |

## Licensing

Crest is GPL-3.0-or-later (see `Cargo.toml`), so copyleft alone is not a blocker. GPL-3.0 and GPL-2.0-or-later code can be linked, MPL-2.0 dependencies are fine (modified files stay MPL), and so is LGPL (keep its notice and relink terms). Still flagged: AGPL, whose network-use terms would then apply to crest as a whole and need an explicit decision; GPL-2.0-only code, which cannot be combined with GPL-3.0; non-commercial and source-available licenses; and any license not yet verified. Where the table below still says "avoid" or "out-of-process" for a GPL or LGPL item, the reason is now build weight (C/C++ toolchains), crash isolation or maintenance, not the license.

| Item | License | Treatment |
| --- | --- | --- |
| JASP | Engine GPLv2, Desktop AGPLv3 | design reference only |
| jamovi | AGPL + GPL | design reference only |
| R | GPL-2 \| GPL-3 [unverified] | call the user's install via subprocess; do not bundle |
| LibreDWG | GPLv3+ | rejected; use `acadrust` |
| OpenBabel, Primer3 | GPL-2 [unverified] | avoid; external process at most |
| OCCT (via `cadrum`, opencascade-rs, `occt-wasm`) | LGPL-2.1 with exception (notice and relink obligations) | optional out-of-process plugin only |
| IfcOpenShell | LGPL-3 [unverified] | avoid; use IFClite |
| `acadrust`, `ifc-lite-*`, `rink-core`, gemmi [unverified] | MPL-2.0 (file-level copyleft) | acceptable unmodified; modified files stay MPL |
| Surfer | EUPL-1.2 (per FreshPorts) | design reference; use `wellen` (BSD-3) |
| `inferno` | CAL-1.0 [unverified] | use only for collapse, or write the folded parser; verify before depending |
| Graphviz C | EPL-1.0 | avoid; `layout-rs` |
| `symbolica` | source-available / commercial | flag; prefer SymPy via bridge |
| `hypors` 0.4.1 | "non-standard" on crates.io | read its LICENSE before use |
| npm `jcampconverter` | CC BY-NC-SA vs MIT (conflicting) | do not copy code |
| Fonts | KaTeX fonts MIT [unverified]; STIX Two OFL-1.1 | ship license texts |

Still unverified or unknown: licenses of `dxf`, `serde-sarif`, `quick-junit`, `wavedrom`, `kiutils_kicad`, `foxtrot`, opencascade-rs, `mzdata`, `proj4rs`, georust crates, `layout-rs`, `vte`, `gix`, `git-graph`, `jsonschema`, Typst (believed Apache-2.0), RDKit (believed BSD-3), ngspice. Also unknown: the crates.io version of RaTeX; the current versions of `plotters`, `arrow`/`parquet` and `opentelemetry-proto`; `layout-rs` 0.1.2 vs 0.1.3; `har` 0.8.1 vs 0.9.0; ranging and IIS support in the `highs` Rust wrapper; the `statrs` `stats_tests` module; truck-stepio's coverage of real SolidWorks/Fusion exports; IFClite's native (non-WASM) crate set; how chematic and cosmolkit depictions compare with RDKit.

## Roadmap

Costs follow the modal note: S = a few days, M = 1–3 weeks, L = more than 3 weeks.

### Phase 1: verifiable numbers and the everyday gaps (about 10–13 weeks)

**Status: implemented (2026-10-24).** The Status column says what was built and where it departs from the approach above. User documentation: [usage.md](../usage.md#statistics-and-calculations) and [plugins.md](../plugins.md#appcompute-engine-computed-cards).

| Item | Approach | Estimate | Status |
| --- | --- | --- | --- |
| `Computed` envelope, slots, numeric-literal lint, provenance chip, recompute/diff, stale marking | serde + JSON Schema, BLAKE3 hashes, diff renderer reuse | 2 weeks | Done: `domain::computed` (envelope, `⟦id.key⟧` slots with an ASCII `[[calc:id.key]]` fallback, lint), recompute through `ComputePort` back to a `BlockUpdated` with `verification`; stale = a recompute found the input file's hash changed (no file watching). |
| Rust stats engine core + golden-file suite vs scipy/R | statrs, polars/ndarray-stats, linregress | 2 weeks | Done in `adapter-compute` on statrs alone: descriptives, quantiles, ranks and OLS (Householder QR, statsmodels-style diagnostics) written by hand instead of polars/ndarray-stats/linregress; Brown-Forsythe as the equal-variance check. Reference tests use NIST StRD (Norris, Longley, Michelson, NumAcc) and R documentation examples. |
| Descriptive, hypothesis test, OLS regression, correlation cards | card anatomy, APA formatter, Lua misuse rules | 2 weeks | Done: tools `stats_describe/test/regress/correlate`, the 8-part card, template sentences and the APA line in `domain`; the misuse rules are in the Rust engine, not Lua. `app.compute.*` in Lua returns the same card. |
| LaTeX math (inline, display, mhchem) | RaTeX via `MarkdownPlugin` pre-pass → `RenderImage` | 2–3 weeks | Done with the RaTeX crates (=0.1.15) directly rather than `ratex-gpui`; inline math sits on the baseline as an inline object, so no image fallback was needed. |
| Unit-checked calc sheet + quantity card | numbat, reactive DAG | 1–2 weeks | Done (`calc`, `convert`) on numbat 1.24.0 without default features; lines are evaluated in order, not as a reactive DAG. |
| Form / approval block | JSON Schema subset, gpui-component inputs | 1–2 weeks | Done (`ask_user`, `FormPort`); headless asks on stdin. A cancel is an `is_error` result (not `ToolError::Cancelled`) so the block keeps its state. |
| Function plot with sliders (plot painter) | own painter + evaluator | 2 weeks | Done (`show_plot`, `app.ui.plot`) with an own expression parser in `domain::plot::expr` instead of exmex; the regression card reuses it for residuals and QQ. |
| Dataframe/Parquet preview, test report, SARIF, ANSI, WaveDrom, molecule 2D, plate map, export/copy-as | existing parsers onto existing table/tree/SVG paths | 2–3 weeks | Done in `adapter-formats` and `ui`: JUnit with quick-xml directly (quick-junit needs a `<testsuites>` root), hand-written SARIF 2.1.0 structs instead of serde-sarif, wavedrom 0.1.0, the parquet record API without arrow, chematic 1.2.1, ANSI with anstyle-parse, SVG with resvg; a copy-as row on the selected block. |

Carried over to phase 2 (gaps left by phase 1):

- Statistics: no session-wide multiple-testing rule (only within one correlation matrix); no Lua API for forms, datasets, molecules or plates.
- Plots: linear axes only, no equal-aspect option, a fixed 160×120 grid for implicit curves (thin features can be missed); hover dots are not coloured per layer.
- Formats: no nextest/libtest JSON reader; SARIF `fixes` are not shown; WaveDrom covers part of WaveJSON and draws on white with approximate text widths; Arrow IPC unsupported; CSV row count unknown beyond 32 MiB; Parquet nested columns shown as text, INT96 timestamps as `int96`.
- Rendering: the molecule SVG and the WaveDrom diagram are not theme-tinted (light card) and molecules have no stereo wedges; the plate scale is sequential only (no diverging or log scale) and labels fit only in 96-well cells; the SVG raster cache never evicts.
- Interaction: copy-as only on the selected block; in a form the Submit click moves focus to the timeline, and a form whose request was withdrawn stays pending in the store.

### Phase 2: shared widgets and domain viewers (about 12–16 weeks)

| Item | Approach | Estimate |
| --- | --- | --- |
| Timeline canvas, filterable list, node-link graph, extended tree | GPUI components exposed as `CustomNode` variants | 3–4 weeks |
| Python/R subprocess bridge | fixed runner, `emit(envelope)`, env hashing | 1–2 weeks |
| Remaining stats and solver cards (distribution, contingency, power, A/B, bootstrap, forecast, LP, Monte Carlo, cash flow, GLM via bridge, derivation) | envelope + plot painter | 3–4 weeks |
| Trace waterfall, flame graph, log viewer, SQL plan, HAR, IaC plan, DOT/ER, benchmark | widgets + parsers, sniffers | 3–4 weeks |
| Waveform, netlist, beam/frame, Bode, K-map, tolerance, material, BOM, STEP via truck, clip/measure, DXF, IFC schedule | wellen, truck, dxf, ifc-lite-core spikes first | 3–4 weeks |
| Sequence, plasmid, reaction, SAR grid, spectrum, ball-and-stick, GHS card, map + CRS card, Gantt, decision matrix, HDF5/NetCDF, network graph, training curves | gb-io, chematic, mzdata, georust, hdf5-pure | 3–4 weeks |
| A2UI and Adaptive Cards inbound, Jupyter MIME mapping, MCP Apps `structuredContent` rendering | node vocabulary alignment | 1–2 weeks |

### Phase 3: heavy and niche (about 10–14 weeks)

| Item | Approach | Estimate |
| --- | --- | --- |
| Typst document/diagram preview | typst + typst-render | 1–2 weeks |
| Vega-Lite via vl-convert-rs (feature-gated) | off-thread render; scenegraph tooltips later | 1 week |
| Bayesian posterior, survival, ODE / phase portrait | bridge + plot painter | 2–3 weeks |
| IFC model + storey plan, FEM field, DWG via acadrust, BCF/IDS, point cloud | 3D view extensions | 3–4 weeks |
| Schematic/PCB, SPICE, cartoon proteins, CIF, MSA, Lean proof state, quantum circuit, ONNX graph, git/CI/k8s/OpenAPI adapters, coverage | Lua plugins on shared widgets where possible | 3–4 weeks |
| Math painted as glyphs, promote-to-artifact, MCP Apps open-in-browser bridge | — | 2 weeks |

Before phase 2, run short spikes on the facts most likely to change the plan: truck-stepio on real exports, IFClite's native crates, chematic depictions vs RDKit, `highs` ranging, and the vl-convert binary size.
