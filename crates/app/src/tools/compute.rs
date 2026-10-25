//! Compute tools, thin [`TypedTool`]s over the [`ComputePort`]: the
//! statistics tools `stats_describe`, `stats_test`, `stats_regress` and
//! `stats_correlate`, and the unit tools `calc` (a calc sheet) and
//! `convert` (one unit conversion), whose input is the port request as is,
//! and the card tools of [`cards`] (distribution, ...).
//! [`register_compute_tools`] adds them and [`COMPUTE_TOOLS_PROMPT`] tells
//! the model to compute with them and to cite numbers through slots.
//!
//! The input of each tool is its port request (so the request's doc
//! comments and defaults are the schema the model sees) with `data`
//! replaced by a [`DataInput`]: a CSV/TSV path inside the workspace, read
//! through the file port and its guard, or inline columns. The engine runs
//! through the [`Offload`] so it never blocks the executor.
//!
//! A good call shows a [`BlockKind::Computed`] card and returns to the
//! model a compact summary: the envelope id, the method, the APA line and
//! the values as slots (`⟦st_….p_value⟧`) it can cite, never the data. A
//! bad request, an unreadable file or a computation that failed comes back
//! as an `is_error` result with the reason.
//!
//! `stats_test` counts the hypothesis tests of each session ([`TestTally`]):
//! from the [`SESSION_TESTS_WARN_FROM`]th on, each new test carries a
//! `multiple_comparisons` warning with the count, suggesting Holm or
//! Benjamini-Hochberg. The p-values are never changed. The tally lives as
//! long as the tool (the app run), so a resumed session counts afresh.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::computed::{
    AssumptionStatus, Computed, ComputedBody, ComputedStatus, ComputedWarning, WarningSeverity,
    narrative, slots,
};
use domain::conversation::SessionId;
use domain::permission::ToolEffect;
use futures::FutureExt;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

mod cards;

pub use self::cards::{Distribution, StatsDistributionTool};

use super::registry::{
    PreparedCall, RegisterError, ToolContext, ToolError, ToolOutput, ToolRegistry, TypedTool,
};
use crate::ports::compute::{
    CalcRequest, ComputePort, ComputeRequest, ConvertRequest, CorrelateRequest, DataColumn,
    DataSet, DescribeRequest, RegressRequest, TestRequest,
};
use crate::ports::fs::FileSystemPort;
use crate::usecases::compute::{
    Offload, SESSION_TESTS_WARNING, load_file, offloaded, set_provenance,
};

/// Who the tools record as having asked for the computation.
pub const REQUESTED_BY: &str = "assistant";

pub use crate::usecases::compute::DELIMITER_PARAM;

/// Most values listed to the model; the card shows all of them.
const MAX_LISTED_VALUES: usize = 40;

/// Where a statistics tool reads its data from.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DataInput {
    /// A CSV or TSV file of the workspace whose first row names the
    /// columns, e.g. `data/cars.csv`. Cells that all read as numbers make a
    /// numeric column; empty cells and `NA` are missing. Give this or
    /// `columns`, not both.
    #[serde(default)]
    pub path: Option<String>,
    /// Field separator of `path`; by default a tab for `.tsv` files and a
    /// comma otherwise.
    #[serde(default)]
    pub delimiter: Option<char>,
    /// Inline columns of equal length: `{"name": "mpg", "values": [21.0,
    /// null, 22.8]}` (numbers or strings; `null` is missing).
    #[serde(default)]
    pub columns: Vec<DataColumn>,
}

/// The input of a statistics tool: the port request `R` with its `data`
/// given as a [`DataInput`].
#[derive(Debug, Clone, PartialEq)]
pub struct StatsInput<R> {
    /// Where the data comes from.
    pub data: DataInput,
    /// The rest of the request; its `data` is a placeholder until loaded.
    pub request: R,
}

impl<'de, R: DeserializeOwned> Deserialize<'de> for StatsInput<R> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let mut value = Value::deserialize(deserializer)?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| D::Error::custom("expected an object"))?;
        let data = match object.remove("data") {
            Some(data) => DataInput::deserialize(data).map_err(D::Error::custom)?,
            None => DataInput::default(),
        };
        object.insert("data".into(), json!({ "columns": [] }));
        let request = R::deserialize(value).map_err(D::Error::custom)?;
        Ok(Self { data, request })
    }
}

impl<R: JsonSchema> JsonSchema for StatsInput<R> {
    fn schema_name() -> Cow<'static, str> {
        R::schema_name()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = R::json_schema(generator);
        let data = generator.subschema_for::<DataInput>();
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.insert("data".into(), data.to_value());
        }
        // Only the replaced `data` field used these.
        for name in ["DataSet", "InputRef"] {
            generator.definitions_mut().remove(name);
        }
        schema
    }
}

/// What the four statistics tools differ in.
pub trait StatsKind: Send + Sync + 'static {
    /// The port request.
    type Request: DeserializeOwned + JsonSchema + Send + Sync + 'static;
    /// Tool name.
    const NAME: &'static str;
    /// Tool description for the model.
    const DESCRIPTION: &'static str;
    /// The request as a [`ComputeRequest`].
    fn wrap(request: Self::Request) -> ComputeRequest;
    /// What the call is about, for the timeline.
    fn subject(request: &Self::Request) -> String;
    /// Whether the request needs `data` (a contingency table given as
    /// counts does not).
    fn needs_data(request: &Self::Request) -> bool {
        let _ = request;
        true
    }
}

/// `stats_describe`.
#[derive(Debug, Clone, Copy)]
pub struct Describe;

impl StatsKind for Describe {
    type Request = DescribeRequest;
    const NAME: &'static str = "stats_describe";
    const DESCRIPTION: &'static str = "\
Computes descriptive statistics with crest's deterministic engine: per variable n, missing, \
mean, SD, median, quartiles, min/max, skew and kurtosis, or counts and percentages for \
categorical columns; per group of `by` when given. The user sees a card; you get the values \
as slots to cite. Use it instead of computing summaries yourself.";
    fn wrap(request: DescribeRequest) -> ComputeRequest {
        ComputeRequest::Describe(request)
    }
    fn subject(request: &DescribeRequest) -> String {
        match &request.by {
            Some(by) => format!("{} by {by}", list_or_all(&request.variables)),
            None => list_or_all(&request.variables),
        }
    }
}

/// `stats_test`.
#[derive(Debug, Clone, Copy)]
pub struct Test;

impl StatsKind for Test {
    type Request = TestRequest;
    const NAME: &'static str = "stats_test";
    const DESCRIPTION: &'static str = "\
Runs a hypothesis test with crest's deterministic engine: Welch, Student, paired or \
one-sample t, Mann-Whitney, Wilcoxon, χ² or Fisher's exact test. With `test: auto` the \
engine picks the test from the data and its assumption checks (normality, equal variances, \
expected counts) and says why. Shapes: numeric `y` split by a two-level `group`; `y` and \
`y2` (with `paired` for repeated measures); `y` alone against `mu`; two categorical columns \
`y` and `group`, or a count `table`, for χ²/Fisher. The user sees a card with assumptions, \
effect size and the APA line; you get the values as slots to cite.";
    fn wrap(request: TestRequest) -> ComputeRequest {
        ComputeRequest::Test(request)
    }
    fn needs_data(request: &TestRequest) -> bool {
        request.table.is_none()
    }
    fn subject(request: &TestRequest) -> String {
        let y = request.y.as_deref().unwrap_or("table");
        match (&request.group, &request.y2) {
            (Some(group), _) => format!("{y} by {group}"),
            (None, Some(y2)) => format!("{y} vs {y2}"),
            (None, None) => y.to_owned(),
        }
    }
}

/// `stats_regress`.
#[derive(Debug, Clone, Copy)]
pub struct Regress;

impl StatsKind for Regress {
    type Request = RegressRequest;
    const NAME: &'static str = "stats_regress";
    const DESCRIPTION: &'static str = "\
Fits an ordinary least squares regression with intercept using crest's deterministic engine: \
coefficients with SE, t, p, confidence intervals, standardised β and VIF; R², adjusted R², \
F, AIC, BIC; Durbin-Watson, Jarque-Bera, Breusch-Pagan and the condition number. Categorical \
predictors are dummy coded. The user sees a card; you get the values as slots to cite.";
    fn wrap(request: RegressRequest) -> ComputeRequest {
        ComputeRequest::Regress(request)
    }
    fn subject(request: &RegressRequest) -> String {
        format!("{} ~ {}", request.y, request.x.join(" + "))
    }
}

/// `stats_correlate`.
#[derive(Debug, Clone, Copy)]
pub struct Correlate;

impl StatsKind for Correlate {
    type Request = CorrelateRequest;
    const NAME: &'static str = "stats_correlate";
    const DESCRIPTION: &'static str = "\
Computes a correlation matrix (Pearson, Spearman or Kendall) with crest's deterministic \
engine, with p-values adjusted for the number of pairs (Holm by default) and pairwise n. For \
two variables it also gives the interval of r. The user sees a heatmap card; you get the \
values as slots to cite.";
    fn wrap(request: CorrelateRequest) -> ComputeRequest {
        ComputeRequest::Correlate(request)
    }
    fn subject(request: &CorrelateRequest) -> String {
        list_or_all(&request.variables)
    }
}

fn list_or_all(names: &[String]) -> String {
    if names.is_empty() {
        "all columns".into()
    } else {
        names.join(", ")
    }
}

/// What the statistics tools run on.
#[derive(Clone)]
pub struct ComputeToolPorts {
    /// The engine.
    pub engine: Arc<dyn ComputePort>,
    /// Reads data files of the workspace (with the guard of `read_file`).
    pub fs: Arc<dyn FileSystemPort>,
    /// Runs the engine off the async executor.
    pub offload: Offload,
}

/// The test of a session from which each new one warns about multiple
/// comparisons.
pub const SESSION_TESTS_WARN_FROM: usize = 3;

/// Code of the warning on tests from the [`SESSION_TESTS_WARN_FROM`]th.
pub const MULTIPLE_COMPARISONS: &str = SESSION_TESTS_WARNING;

/// Hypothesis tests run so far, per session.
#[derive(Debug, Default)]
pub struct TestTally(Mutex<HashMap<SessionId, usize>>);

impl TestTally {
    /// Counts one more test in `session` and returns how many it has run.
    pub fn record(&self, session: SessionId) -> usize {
        let mut counts = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let count = counts.entry(session).or_insert(0);
        *count += 1;
        *count
    }

    /// Tests run so far in `session`.
    #[must_use]
    pub fn count(&self, session: SessionId) -> usize {
        let counts = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        counts.get(&session).copied().unwrap_or(0)
    }
}

/// The warning on the `count`th test of a session, from the
/// [`SESSION_TESTS_WARN_FROM`]th on.
#[must_use]
pub fn session_tests_warning(count: usize) -> Option<ComputedWarning> {
    (count >= SESSION_TESTS_WARN_FROM).then(|| ComputedWarning {
        code: MULTIPLE_COMPARISONS.into(),
        severity: WarningSeverity::Warn,
        text: format!(
            "this is hypothesis test {count} of this session; with several tests some come \
             out significant by chance, so adjust the family of p-values (Holm, or \
             Benjamini-Hochberg for the false discovery rate) before calling any significant"
        ),
    })
}

/// A statistics tool of kind `K`.
pub struct StatsTool<K> {
    ports: ComputeToolPorts,
    tally: Arc<TestTally>,
    kind: PhantomData<K>,
}

impl<K> StatsTool<K> {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: ComputeToolPorts) -> Self {
        Self {
            ports,
            tally: Arc::default(),
            kind: PhantomData,
        }
    }

    /// The per-session test count this tool keeps.
    #[must_use]
    pub fn tally(&self) -> Arc<TestTally> {
        Arc::clone(&self.tally)
    }
}

/// `stats_describe`.
pub type StatsDescribeTool = StatsTool<Describe>;
/// `stats_test`.
pub type StatsTestTool = StatsTool<Test>;
/// `stats_regress`.
pub type StatsRegressTool = StatsTool<Regress>;
/// `stats_correlate`.
pub type StatsCorrelateTool = StatsTool<Correlate>;

fn invalid(tool: &str, message: impl Into<String>) -> ToolError {
    ToolError::InvalidInput {
        tool: tool.to_owned(),
        message: message.into(),
    }
}

#[async_trait]
impl<K: StatsKind> TypedTool for StatsTool<K> {
    type Input = StatsInput<K::Request>;

    fn name(&self) -> &str {
        K::NAME
    }

    fn description(&self) -> &str {
        K::DESCRIPTION
    }

    fn effect(&self, input: &Self::Input) -> ToolEffect {
        // Computing only reads; the file goes through the read guard.
        let _ = input;
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: Self::Input,
        ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let StatsInput { data, request } = input;
        let mut subject = K::subject(&request);
        let needs_data = K::needs_data(&request);
        let mut request = K::wrap(request);
        match (&data.path, data.columns.is_empty()) {
            (Some(_), false) => {
                return Err(invalid(
                    K::NAME,
                    "give either `data.path` or `data.columns`, not both",
                ));
            }
            (None, true) if needs_data => {
                return Err(invalid(
                    K::NAME,
                    "no data: give `data.path` (a CSV/TSV file of the workspace) or \
                     `data.columns`",
                ));
            }
            (Some(path), true) => subject = format!("{subject} · {path}"),
            _ => {}
        }
        let ports = self.ports.clone();
        let agent = format!("assistant-turn-{}", ctx.turn_id);
        let session = Some((Arc::clone(&self.tally), ctx.session_id));
        let run = async move {
            let loaded = match &data.path {
                Some(path) => match load_file(ports.fs.as_ref(), path, data.delimiter).await {
                    Ok(loaded) => loaded,
                    Err(message) => return Ok(ToolOutput::error(message)),
                },
                None => DataSet::inline(data.columns),
            };
            if let Some(slot) = request.data_mut() {
                *slot = loaded;
            }
            Ok(card(&ports, request, &agent, data.delimiter, session).await)
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// Runs `request` on the engine off the executor and turns the envelope
/// into a card plus the model's summary, or an `is_error` output. A
/// hypothesis test counts in `session`'s tally and warns from the
/// [`SESSION_TESTS_WARN_FROM`]th.
async fn card(
    ports: &ComputeToolPorts,
    request: ComputeRequest,
    agent: &str,
    delimiter: Option<char>,
    session: Option<(Arc<TestTally>, SessionId)>,
) -> ToolOutput {
    let engine = Arc::clone(&ports.engine);
    let Some(result) = offloaded(&ports.offload, move || engine.run(&request)).await else {
        return ToolOutput::error("the engine stopped before answering");
    };
    let (mut computed, body) = match result {
        Ok(done) => done,
        Err(err) => return ToolOutput::error(format!("{err}. Fix the call and try again.")),
    };
    if computed.status == ComputedStatus::Error {
        return ToolOutput::error(format!(
            "the engine could not compute it: {}",
            computed.error.as_deref().unwrap_or("unknown error")
        ));
    }
    set_provenance(&mut computed, agent, REQUESTED_BY);
    if let (ComputedBody::Test(_), Some((tally, session))) = (&body, session) {
        let count = tally.record(session);
        let warned = computed
            .warnings
            .iter()
            .any(|w| w.code == MULTIPLE_COMPARISONS);
        if let Some(warning) = session_tests_warning(count).filter(|_| !warned) {
            computed.warnings.push(warning);
        }
    }
    if let Some(delimiter) = delimiter {
        computed
            .method
            .params
            .insert(DELIMITER_PARAM.into(), Value::from(delimiter.to_string()));
    }
    let text = summary(&computed, &body);
    ToolOutput::text(text).with_block_update(BlockKind::Computed {
        envelope: Box::new(computed),
        body: Box::new(body),
    })
}

/// What the two unit tools differ in.
pub trait UnitsKind: Send + Sync + 'static {
    /// The port request, which is also the tool input.
    type Request: DeserializeOwned + JsonSchema + Send + Sync + 'static;
    /// Tool name.
    const NAME: &'static str;
    /// Tool description for the model.
    const DESCRIPTION: &'static str;
    /// The request as a [`ComputeRequest`].
    fn wrap(request: Self::Request) -> ComputeRequest;
    /// What the call is about, for the timeline.
    fn subject(request: &Self::Request) -> String;
    /// Whether the request makes sense before it reaches the engine.
    ///
    /// # Errors
    ///
    /// The reason it does not.
    fn check(request: &Self::Request) -> Result<(), String>;
}

/// `calc`.
#[derive(Debug, Clone, Copy)]
pub struct Calc;

/// Most lines of one calc sheet.
pub const MAX_CALC_LINES: usize = 200;

impl UnitsKind for Calc {
    type Request = CalcRequest;
    const NAME: &'static str = "calc";
    const DESCRIPTION: &'static str = "\
Evaluates a calc sheet with units using crest's deterministic engine (numbat): lines \
`name = expression` or bare expressions, top to bottom; later lines use earlier names. Units \
are checked and converted (`v = 100 km/h`, `t = 90 min`, `d = v * t -> km`, `P = 3 kW * 2 h \
-> kWh`); functions such as sqrt, sin, ln and constants such as pi work. A line with an \
error (a dimension mismatch, an unknown unit, a name used before it is defined) shows the \
error and the others still evaluate. Names must not clash with units or constants: `m`, `g`, \
`h`, `s`, `c` are units, so write `m_car`, `g_0`, `h_1`. The user sees the sheet with each \
result typeset; you get each named result as a slot to cite.";
    fn wrap(request: CalcRequest) -> ComputeRequest {
        ComputeRequest::Calc(request)
    }
    fn subject(request: &CalcRequest) -> String {
        let names: Vec<&str> = request
            .lines
            .iter()
            .filter_map(|l| l.split_once('=').map(|(n, _)| n.trim()))
            .filter(|n| !n.is_empty() && !n.contains(' '))
            .collect();
        match names.as_slice() {
            [] => format!("{} lines", request.lines.len()),
            names => names.join(", "),
        }
    }
    fn check(request: &CalcRequest) -> Result<(), String> {
        if request.lines.iter().all(|l| l.trim().is_empty()) {
            return Err("`lines` is empty: give at least one `name = expression` line".into());
        }
        if request.lines.len() > MAX_CALC_LINES {
            return Err(format!("at most {MAX_CALC_LINES} lines per sheet"));
        }
        if !(1..=15).contains(&request.significant_digits) {
            return Err("`significant_digits` must be between 1 and 15".into());
        }
        Ok(())
    }
}

/// `convert`.
#[derive(Debug, Clone, Copy)]
pub struct Convert;

impl UnitsKind for Convert {
    type Request = ConvertRequest;
    const NAME: &'static str = "convert";
    const DESCRIPTION: &'static str = "\
Converts a value between units with crest's deterministic engine (numbat): the result, the \
conversion factor, whether the factor is exact by definition or rounded, the dimension and \
the chain through SI units. Give `value` and units (`value: 65, from: \"mi/h\", to: \"km/h\"`), \
or a whole quantity in `from` (`from: \"5 ft + 3 in\", to: \"cm\"`). Temperatures \
(`degC`, `degF`, `K`) convert with their offset. An `uncertainty` is propagated linearly. \
The user sees a card; you get the result as a slot to cite.";
    fn wrap(request: ConvertRequest) -> ComputeRequest {
        ComputeRequest::Convert(request)
    }
    fn subject(request: &ConvertRequest) -> String {
        match request.value {
            Some(value) => format!("{value} {} → {}", request.from, request.to),
            None => format!("{} → {}", request.from, request.to),
        }
    }
    fn check(request: &ConvertRequest) -> Result<(), String> {
        if request.from.trim().is_empty() || request.to.trim().is_empty() {
            return Err("give both `from` and `to`".into());
        }
        if request.value.is_some_and(|v| !v.is_finite()) {
            return Err("`value` must be a finite number".into());
        }
        if request
            .uncertainty
            .is_some_and(|u| !(u.is_finite() && u >= 0.0))
        {
            return Err("`uncertainty` must be a finite number ≥ 0".into());
        }
        if !(1..=15).contains(&request.significant_digits) {
            return Err("`significant_digits` must be between 1 and 15".into());
        }
        Ok(())
    }
}

/// A unit tool of kind `K`: `calc` or `convert`.
pub struct UnitsTool<K> {
    ports: ComputeToolPorts,
    kind: PhantomData<K>,
}

impl<K> UnitsTool<K> {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: ComputeToolPorts) -> Self {
        Self {
            ports,
            kind: PhantomData,
        }
    }
}

/// `calc`.
pub type CalcTool = UnitsTool<Calc>;
/// `convert`.
pub type ConvertTool = UnitsTool<Convert>;

#[async_trait]
impl<K: UnitsKind> TypedTool for UnitsTool<K> {
    type Input = K::Request;

    fn name(&self) -> &str {
        K::NAME
    }

    fn description(&self) -> &str {
        K::DESCRIPTION
    }

    fn effect(&self, input: &Self::Input) -> ToolEffect {
        // Pure computation: nothing is read or written.
        let _ = input;
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: Self::Input,
        ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        K::check(&input).map_err(|message| invalid(K::NAME, message))?;
        let subject = K::subject(&input);
        let request = K::wrap(input);
        let ports = self.ports.clone();
        let agent = format!("assistant-turn-{}", ctx.turn_id);
        let run = async move { Ok(card(&ports, request, &agent, None, None).await) };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back: id, method, APA line, the values as slots,
/// failed assumptions and warnings.
#[must_use]
pub fn summary(computed: &Computed, body: &ComputedBody) -> String {
    let id = &computed.id;
    let mut out = format!("Computed {id}: {}", computed.method.name);
    if let Some(why) = &computed.method.why {
        let _ = write!(out, " ({why})");
    }
    out.push_str(". The user sees it as a card; do not repeat its table.\n");
    if let Some(line) = narrative::apa_line(computed, body) {
        let _ = writeln!(out, "APA: {}", line.plain());
    }
    let mut keys: Vec<&str> = computed
        .narrative
        .iter()
        .flat_map(|n| n.text_slots.iter().map(String::as_str))
        .filter(|k| computed.values.contains_key(*k))
        .collect();
    for key in computed.values.keys() {
        if !keys.contains(&key.as_str()) {
            keys.push(key);
        }
    }
    let _ = writeln!(
        out,
        "Values (cite each number by its slot; the timeline fills in the engine's value):"
    );
    for key in keys.iter().take(MAX_LISTED_VALUES) {
        if let Some(value) = computed.values.get(*key) {
            let _ = writeln!(out, "- {} = {}", slots::slot(id, key), value.display_text());
        }
    }
    if keys.len() > MAX_LISTED_VALUES {
        let _ = writeln!(
            out,
            "- … {} more keys of the same shapes",
            keys.len() - MAX_LISTED_VALUES
        );
    }
    let failing: Vec<String> = computed
        .assumptions
        .iter()
        .filter(|a| matches!(a.status, AssumptionStatus::Fail | AssumptionStatus::Warn))
        .map(|a| {
            let status = if a.status == AssumptionStatus::Fail {
                "fails"
            } else {
                "is doubtful"
            };
            match &a.consequence {
                Some(c) => format!("{} {status} ({c})", a.id),
                None => format!("{} {status}", a.id),
            }
        })
        .collect();
    if !failing.is_empty() {
        let _ = writeln!(out, "Assumptions: {}.", failing.join("; "));
    }
    match body {
        ComputedBody::CalcSheet(sheet) => {
            for (i, line) in sheet.lines.iter().enumerate() {
                let label = match &line.name {
                    Some(name) => format!("line {} (`{name}`)", i + 1),
                    None => format!("line {}", i + 1),
                };
                if let Some(error) = &line.error {
                    let _ = writeln!(out, "Error on {label}: {error}");
                }
                for note in &line.notes {
                    let _ = writeln!(out, "Note on {label}: {note}");
                }
            }
        }
        ComputedBody::Quantity(quantity) => {
            let exact = if quantity.exact {
                "exact by definition"
            } else {
                "rounded"
            };
            let _ = writeln!(
                out,
                "Factor {} → {}: {} ({exact}); dimension {}.",
                quantity.from.unit, quantity.to.unit, quantity.factor, quantity.dimension
            );
        }
        other => cards::notes(other, &mut out),
    }
    for warning in &computed.warnings {
        let _ = writeln!(out, "Warning [{}]: {}", warning.code, warning.text);
    }
    if !computed.method.alternatives.is_empty() {
        let _ = writeln!(
            out,
            "Alternatives: {}.",
            computed.method.alternatives.join(", ")
        );
    }
    out.trim_end().to_owned()
}

/// System prompt section on computing numbers.
pub const COMPUTE_TOOLS_PROMPT: &str = "\
# Computing numbers

Never compute statistics or state computed numbers from memory or mental arithmetic. To \
summarise data, test a hypothesis, fit a regression or correlate variables, call \
`stats_describe`, `stats_test`, `stats_regress` or `stats_correlate` with the data (a CSV/TSV \
path of the workspace in `data.path`, or inline `data.columns`). The engine shows the user a \
card with the method and why it was chosen, assumption checks, results, a sentence and the \
APA line, so do not repeat the card.

To look at the shape of a variable (histogram, density, box/violin, QQ, ECDF), call `stats_distribution`.

For any arithmetic with units, or anything beyond trivial arithmetic, call `calc` with the \
steps as named lines (`v = 100 km/h`, `t = 90 min`, `d = v * t -> km`) instead of working it \
out yourself; for a unit conversion call `convert`. Quote results through their slots.

When your reply mentions a computed number, cite it by the slot the tool listed, exactly as \
written, e.g. `⟦st_1a2b3c4d5e6f.p_value⟧`; the timeline replaces it with the engine's value. \
Numbers you type yourself are flagged to the user as not coming from an engine. If a call \
fails, fix the input and call it again.";

/// Registers `stats_describe`, `stats_test`, `stats_regress`,
/// `stats_correlate`, `calc`, `convert` and the phase-2 card tools
/// ([`cards`]) over `ports`.
///
/// # Errors
///
/// [`RegisterError::Duplicate`] when one of them is already registered.
pub fn register_compute_tools(
    registry: &mut ToolRegistry,
    ports: &ComputeToolPorts,
) -> Result<(), RegisterError> {
    registry.register_typed(StatsDescribeTool::new(ports.clone()))?;
    registry.register_typed(StatsTestTool::new(ports.clone()))?;
    registry.register_typed(StatsRegressTool::new(ports.clone()))?;
    registry.register_typed(StatsCorrelateTool::new(ports.clone()))?;
    registry.register_typed(CalcTool::new(ports.clone()))?;
    registry.register_typed(ConvertTool::new(ports.clone()))?;
    registry.register_typed(StatsDistributionTool::new(ports.clone()))
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};

    use super::*;
    use crate::ports::compute::{ColumnValues, ComputeError};
    use crate::testing::compute::FakeCompute;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::tools::registry::{Tool, Typed};
    use crate::usecases::compute::inline_offload;

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn ports(engine: Arc<FakeCompute>) -> ComputeToolPorts {
        let fs = InMemoryFs::new(test_root())
            .with_file(
                "data/cars.csv",
                "mpg,am\n21,manual\n22.8,manual\n18.7,auto\n",
            )
            .with_file(".env", "a\n1\n");
        ComputeToolPorts {
            engine,
            fs: Arc::new(fs),
            offload: inline_offload(),
        }
    }

    async fn call(tool: &dyn Tool, input: Value) -> Result<ToolOutput, ToolError> {
        let prepared = tool.prepare(input, &ctx()).await?;
        prepared.run.await
    }

    fn definition<T: TypedTool + 'static>(tool: T) -> Value {
        let tool = Typed(tool);
        json!({
            "name": tool.name(),
            "description": tool.description(),
            "input_schema": tool.input_schema(),
        })
    }

    #[test]
    fn definitions() {
        let ports = ports(Arc::new(FakeCompute::new()));
        insta::assert_json_snapshot!(
            "stats_describe_definition",
            definition(StatsDescribeTool::new(ports.clone()))
        );
        insta::assert_json_snapshot!(
            "stats_test_definition",
            definition(StatsTestTool::new(ports.clone()))
        );
        insta::assert_json_snapshot!(
            "stats_regress_definition",
            definition(StatsRegressTool::new(ports.clone()))
        );
        insta::assert_json_snapshot!(
            "stats_correlate_definition",
            definition(StatsCorrelateTool::new(ports.clone()))
        );
        insta::assert_json_snapshot!("calc_definition", definition(CalcTool::new(ports.clone())));
        insta::assert_json_snapshot!(
            "convert_definition",
            definition(ConvertTool::new(ports.clone()))
        );
        insta::assert_json_snapshot!(
            "stats_distribution_definition",
            definition(StatsDistributionTool::new(ports))
        );
    }

    #[tokio::test]
    async fn a_calc_sheet_is_a_card_with_a_slot_per_named_line() {
        let engine = Arc::new(FakeCompute::new());
        let tool = Typed(CalcTool::new(ports(Arc::clone(&engine))));
        let input = json!({ "lines": ["h_1 = 5 ft + 3 in", "h_1 -> cm"] });
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        assert_eq!(prepared.subject, "h_1");
        let out = prepared.run.await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        let text = out.text_content();
        let Some(BlockKind::Computed { envelope, body }) = out.block_update else {
            panic!("expected a computed block, got {:?}", out.block_update);
        };
        assert!(matches!(*body, ComputedBody::CalcSheet(_)));
        assert_eq!(
            envelope.provenance.requested_by.as_deref(),
            Some(REQUESTED_BY)
        );
        let slot = slots::slot(&envelope.id, "h_1");
        assert!(text.contains(&format!("- {slot} = 1 m")), "{text}");
        let ComputeRequest::Calc(request) = &engine.requests()[0] else {
            panic!("expected a calc request");
        };
        assert_eq!(request.lines.len(), 2);
        assert_eq!(request.significant_digits, 6);
    }

    #[tokio::test]
    async fn a_conversion_reports_its_factor() {
        let engine = Arc::new(FakeCompute::new());
        let tool = Typed(ConvertTool::new(ports(Arc::clone(&engine))));
        let input = json!({ "value": 2.0, "from": "in", "to": "cm", "uncertainty": 0.1 });
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        assert_eq!(prepared.subject, "2 in → cm");
        let out = prepared.run.await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        let text = out.text_content();
        assert!(
            text.contains("Factor in → cm: 2.54 (exact by definition)"),
            "{text}"
        );
        assert!(text.contains(".result⟧ = 5.08 cm"), "{text}");
        let Some(BlockKind::Computed { body, .. }) = out.block_update else {
            panic!("expected a computed block");
        };
        let ComputedBody::Quantity(quantity) = *body else {
            panic!("expected a quantity body");
        };
        assert_eq!(quantity.to.uncertainty, Some(0.254));
    }

    #[tokio::test]
    async fn bad_unit_requests_never_reach_the_engine() {
        let engine = Arc::new(FakeCompute::new());
        let calc = Typed(CalcTool::new(ports(Arc::clone(&engine))));
        for input in [
            json!({ "lines": [] }),
            json!({ "lines": ["  "] }),
            json!({ "lines": ["1 m"], "significant_digits": 0 }),
            json!({ "expr": "1 m" }),
        ] {
            let err = call(&calc, input.clone()).await.unwrap_err();
            assert!(ToolOutput::from(err).is_error, "{input}");
        }
        let convert = Typed(ConvertTool::new(ports(Arc::clone(&engine))));
        for input in [
            json!({ "from": "m", "to": " " }),
            json!({ "from": "m", "to": "ft", "uncertainty": -1.0 }),
        ] {
            assert!(
                matches!(
                    call(&convert, input.clone()).await,
                    Err(ToolError::InvalidInput { .. })
                ),
                "{input}"
            );
        }
        assert!(engine.requests().is_empty());

        let failing = Arc::new(
            FakeCompute::new().failing(ComputeError::Invalid("unknown unit `furlongz`".into())),
        );
        let convert = Typed(ConvertTool::new(ports(failing)));
        let out = call(
            &convert,
            json!({ "value": 1.0, "from": "furlongz", "to": "m" }),
        )
        .await
        .unwrap();
        assert!(out.is_error);
        assert!(out.text_content().contains("furlongz"));
        assert_eq!(out.block_update, None);
    }

    #[tokio::test]
    async fn a_file_is_read_through_the_guard_and_shown_as_a_card() {
        let engine = Arc::new(FakeCompute::new());
        let tool = Typed(StatsTestTool::new(ports(Arc::clone(&engine))));
        let input = json!({ "data": { "path": "data/cars.csv" }, "y": "mpg", "group": "am" });
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        assert_eq!(prepared.subject, "mpg by am · data/cars.csv");
        let out = prepared.run.await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        let text = out.text_content();
        let Some(BlockKind::Computed { envelope, body }) = out.block_update else {
            panic!("expected a computed block, got {:?}", out.block_update);
        };
        assert!(matches!(*body, ComputedBody::Test(_)));
        assert!(
            envelope
                .provenance
                .agent
                .as_deref()
                .is_some_and(|a| a.starts_with("assistant-turn-"))
        );
        assert_eq!(
            envelope.provenance.requested_by.as_deref(),
            Some(REQUESTED_BY)
        );

        let requests = engine.requests();
        let ComputeRequest::Test(request) = &requests[0] else {
            panic!("expected a test request");
        };
        assert_eq!(request.data.source.path.as_deref(), Some("data/cars.csv"));
        assert_eq!(
            request.data.column("am").map(|c| &c.values),
            Some(&ColumnValues::Categorical(vec![
                Some("manual".into()),
                Some("manual".into()),
                Some("auto".into())
            ]))
        );

        let slot = slots::slot(&envelope.id, "p_value");
        assert!(text.contains(&format!("- {slot} = ")), "{text}");
        assert!(
            text.contains(&format!("Computed {}", envelope.id)),
            "{text}"
        );
        assert!(text.contains("APA: t(10) = 2.50, p = .030"), "{text}");
        assert!(
            !text.contains("manual"),
            "the data must not go back: {text}"
        );
    }

    #[tokio::test]
    async fn inline_columns_go_to_the_engine_as_is() {
        let engine = Arc::new(FakeCompute::new());
        let tool = Typed(StatsDescribeTool::new(ports(Arc::clone(&engine))));
        let input = json!({ "data": { "columns": [
            { "name": "x", "values": [1.0, null, 3.0] }
        ] } });
        let out = call(&tool, input).await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert!(
            out.text_content().contains(".x.mean⟧ = 2.00"),
            "{}",
            out.text_content()
        );
        let ComputeRequest::Describe(request) = &engine.requests()[0] else {
            panic!("expected describe");
        };
        assert_eq!(request.data.source.name, "inline");
    }

    #[tokio::test]
    async fn bad_inputs_are_errors_for_the_model() {
        let engine = Arc::new(FakeCompute::new());
        let tool = Typed(StatsDescribeTool::new(ports(Arc::clone(&engine))));
        let no_data = call(&tool, json!({})).await.unwrap_err();
        assert!(ToolOutput::from(no_data).is_error);
        let both = json!({ "data": { "path": "a.csv", "columns": [
            { "name": "x", "values": [1.0] }
        ] } });
        assert!(matches!(
            call(&tool, both).await,
            Err(ToolError::InvalidInput { .. })
        ));
        let unknown = json!({ "data": { "file": "a.csv" } });
        assert!(matches!(
            call(&tool, unknown).await,
            Err(ToolError::InvalidInput { .. })
        ));
        for path in [".env", "../out.csv", "missing.csv"] {
            let out = call(&tool, json!({ "data": { "path": path } }))
                .await
                .unwrap();
            assert!(out.is_error, "{path}");
            assert_eq!(out.block_update, None);
        }
        assert!(engine.requests().is_empty());

        let test = Typed(StatsTestTool::new(ports(Arc::new(FakeCompute::new()))));
        let table = json!({ "table": [[3, 1], [1, 3]], "test": "fisher" });
        assert!(!call(&test, table).await.unwrap().is_error);

        let failing = Arc::new(
            FakeCompute::new().failing(ComputeError::Invalid("unknown column `mpgg`".into())),
        );
        let tool = Typed(StatsRegressTool::new(ports(failing)));
        let out = call(
            &tool,
            json!({ "data": { "path": "data/cars.csv" }, "y": "mpgg", "x": ["am"] }),
        )
        .await
        .unwrap();
        assert!(out.is_error);
        assert!(
            out.text_content().contains("mpgg"),
            "{}",
            out.text_content()
        );
    }

    #[tokio::test]
    async fn the_third_test_of_a_session_warns_about_multiple_comparisons() {
        let tool = StatsTestTool::new(ports(Arc::new(FakeCompute::new())));
        let tally = tool.tally();
        let tool = Typed(tool);
        let table = json!({ "table": [[3, 1], [1, 3]], "test": "fisher" });
        let session = SessionId::new();
        let mut cards = Vec::new();
        for _ in 0..4 {
            let ctx = ToolContext::new(session, TurnId::new(), ToolUseId::new("toolu_t"));
            let out = tool
                .prepare(table.clone(), &ctx)
                .await
                .unwrap()
                .run
                .await
                .unwrap();
            let Some(BlockKind::Computed { envelope, .. }) = out.block_update.clone() else {
                panic!("a card");
            };
            cards.push((envelope, out.text_content()));
        }
        assert_eq!(tally.count(session), 4);
        let warned = |c: &Computed| {
            c.warnings
                .iter()
                .filter(|w| w.code == MULTIPLE_COMPARISONS)
                .map(|w| w.text.clone())
                .collect::<Vec<_>>()
        };
        assert!(warned(&cards[0].0).is_empty() && warned(&cards[1].0).is_empty());
        let third = warned(&cards[2].0);
        assert_eq!(third.len(), 1);
        assert!(
            third[0].contains("test 3") && third[0].contains("Holm"),
            "{third:?}"
        );
        assert!(warned(&cards[3].0)[0].contains("test 4"));
        // The model sees it; the p-values stay as the engine gave them.
        assert!(cards[2].1.contains("Holm"), "{}", cards[2].1);
        assert_eq!(cards[0].0.values, cards[2].0.values);

        // Another session starts its own count.
        let other = ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_o"));
        let out = tool
            .prepare(table, &other)
            .await
            .unwrap()
            .run
            .await
            .unwrap();
        let Some(BlockKind::Computed { envelope, .. }) = out.block_update else {
            panic!("a card");
        };
        assert!(warned(&envelope).is_empty());
        assert_eq!(session_tests_warning(2), None);
    }

    #[test]
    fn registers_the_tools_named_in_the_prompt() {
        let mut registry = ToolRegistry::new();
        let ports = ports(Arc::new(FakeCompute::new()));
        register_compute_tools(&mut registry, &ports).unwrap();
        assert_eq!(
            registry.names(),
            [
                "calc",
                "convert",
                "stats_correlate",
                "stats_describe",
                "stats_distribution",
                "stats_regress",
                "stats_test"
            ]
        );
        for name in registry.names() {
            assert!(
                COMPUTE_TOOLS_PROMPT.contains(&format!("`{name}`")),
                "{name}"
            );
        }
    }
}
