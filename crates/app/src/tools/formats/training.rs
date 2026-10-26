//! `show_training_curves`: machine-learning training logs as curves with
//! smoothing and per-run summaries.
//!
//! The model gets the runs, metrics and axes, and per run and metric the
//! count, best (with its step), last, minimum and maximum the engine
//! computed with the envelope id, never the series.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::permission::ToolEffect;
use domain::science::training::{Goal, MAX_RUNS, TrainingAxis, TrainingSpec, value_text};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{FormatToolPorts, invalid, one_line, read};
use crate::ports::formats::{TrainingFile, TrainingRequest};
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Summaries listed to the model.
const LISTED: usize = 40;

/// Input of `show_training_curves`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowTrainingCurves {
    /// A training log of the workspace: CSV/TSV with a header (Keras or
    /// Lightning CSV loggers, wandb or TensorBoard scalar exports) or JSON
    /// lines.
    #[serde(default)]
    pub path: Option<String>,
    /// Several logs, one run each (or several through a `run` column).
    #[serde(default)]
    pub paths: Vec<String>,
    /// The metric shown first (`val_loss`); the first loss-like one when
    /// absent. Every metric can be picked on the plot.
    #[serde(default)]
    pub metric: Option<String>,
    /// The x axis: `step` (default), `epoch` or `time`.
    #[serde(default)]
    pub x: Option<TrainingAxis>,
    /// EMA smoothing weight from 0 (none) to below 1 (default 0.6, as
    /// TensorBoard).
    #[serde(default)]
    pub smoothing: Option<f64>,
    /// Start with a logarithmic y axis.
    #[serde(default)]
    pub log_y: bool,
    /// Whether lower (`minimize`) or higher (`maximize`) is better for
    /// every metric; guessed from each name when absent.
    #[serde(default)]
    pub goal: Option<Goal>,
    /// Title shown above the plot.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_training_curves`: training curves.
#[derive(Clone)]
pub struct ShowTrainingCurvesTool {
    ports: FormatToolPorts,
}

impl ShowTrainingCurvesTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_training_curves`.
pub const SHOW_TRAINING_CURVES: &str = "show_training_curves";

#[async_trait]
impl TypedTool for ShowTrainingCurvesTool {
    type Input = ShowTrainingCurves;

    fn name(&self) -> &str {
        SHOW_TRAINING_CURVES
    }

    fn description(&self) -> &str {
        "Plots machine-learning training logs (CSV or JSON lines; `path` or \
         `paths`, one run each or a `run` column) in the user's timeline: \
         every run's curve of a metric (picked on the plot) against step, \
         epoch or time, smoothed with TensorBoard's EMA over the raw values \
         drawn faint, best values marked, linear or log y. The engine \
         summarises every run and metric (count, min, max, last, best with \
         its step). Returns the runs, metrics and summaries with their \
         envelope id; compare runs from these numbers, not from the shape \
         of the curves."
    }

    fn effect(&self, _input: &ShowTrainingCurves) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowTrainingCurves,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let mut paths: Vec<String> = input.path.into_iter().chain(input.paths).collect();
        paths.retain(|p| !p.trim().is_empty());
        if paths.is_empty() {
            return Err(invalid(SHOW_TRAINING_CURVES, "give `path` or `paths`"));
        }
        if paths.len() > MAX_RUNS {
            return Err(invalid(
                SHOW_TRAINING_CURVES,
                format!("at most {MAX_RUNS} logs"),
            ));
        }
        if input.smoothing.is_some_and(|s| !(0.0..1.0).contains(&s)) {
            return Err(invalid(
                SHOW_TRAINING_CURVES,
                "`smoothing` is from 0 to below 1",
            ));
        }
        let ports = self.ports.clone();
        let subject = paths.join(", ");
        let title = input.title.filter(|t| !t.trim().is_empty());
        let run = async move {
            let mut files = Vec::new();
            for path in &paths {
                match read(ports.fs.as_ref(), path).await {
                    Ok(bytes) => files.push(TrainingFile {
                        path: path.clone(),
                        bytes,
                    }),
                    Err(message) => return Ok(ToolOutput::error(message)),
                }
            }
            let request = TrainingRequest {
                files,
                metric: input.metric.filter(|m| !m.trim().is_empty()),
                axis: input.x,
                smoothing: input.smoothing,
                log_y: input.log_y,
                goal: input.goal,
            };
            let formats = Arc::clone(&ports.formats);
            let read = offloaded(&ports.offload, move || formats.training_curves(&request)).await;
            let mut spec = match read {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the log reader did not run")),
            };
            spec.title = title;
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(err.to_string()));
            }
            let summary = training_summary(&spec);
            Ok(ToolOutput::text(summary)
                .with_block_update(BlockKind::TrainingCurves(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_training_curves`.
#[must_use]
pub fn training_summary(spec: &TrainingSpec) -> String {
    let runs: Vec<&str> = spec.runs.iter().map(|r| r.name.as_str()).collect();
    let axes: Vec<&str> = spec.axes().iter().map(|a| a.name()).collect();
    let mut out = format!(
        "Training curves `{}` shown to the user: {} runs ({}), metrics {}; showing `{}` against \
         {} (axes: {}), EMA smoothing {}.",
        one_line(&spec.display_title(), 80),
        spec.runs.len(),
        runs.join(", "),
        spec.metrics.join(", "),
        spec.metric,
        spec.axis.name(),
        axes.join(", "),
        spec.smoothing
    );
    let _ = write!(
        out,
        "\nSummaries over all logged values (envelope `{}`, keys \
         `<run>.<metric>.n|min|max|last|best|best_step|last_step`):",
        spec.stats.id
    );
    for s in spec.summaries.iter().take(LISTED) {
        let step = |row: usize| {
            spec.x_at(&s.run, row, TrainingAxis::Step)
                .map_or_else(|| "?".to_owned(), value_text)
        };
        let best = s.best();
        let _ = write!(
            out,
            "\n- {} {}: best {} at step {} ({}{}), last {} at step {}, min {}, max {}, n {}",
            one_line(&s.run, 40),
            one_line(&s.metric, 40),
            value_text(best.value),
            step(best.row),
            match s.goal {
                Goal::Minimize => "lowest",
                Goal::Maximize => "highest",
            },
            if s.goal_given { "" } else { ", goal guessed" },
            value_text(s.last.value),
            step(s.last.row),
            value_text(s.min.value),
            value_text(s.max.value),
            s.n
        );
    }
    if spec.summaries.len() > LISTED {
        let _ = write!(out, "\n… {} more", spec.summaries.len() - LISTED);
    }
    for w in spec.warnings.iter().take(5) {
        let _ = write!(out, "\nNote: {}", one_line(w, 200));
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::{Value, json};

    use super::*;
    use crate::testing::formats::FakeFormats;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::tools::registry::{Tool, Typed};
    use crate::usecases::compute::inline_offload;

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn ports() -> FormatToolPorts {
        let fs = InMemoryFs::new(test_root()).with_file("runs/a.csv", "step,loss\n");
        FormatToolPorts {
            formats: Arc::new(FakeFormats::new()),
            fs: Arc::new(fs),
            offload: inline_offload(),
        }
    }

    async fn call(tool: &dyn Tool, input: Value) -> Result<ToolOutput, ToolError> {
        let prepared = tool.prepare(input, &ctx()).await?;
        prepared.run.await
    }

    #[test]
    fn definition() {
        let tool = Typed(ShowTrainingCurvesTool::new(ports()));
        insta::assert_json_snapshot!(
            "show_training_curves_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn the_model_gets_the_summaries() {
        let tool = Typed(ShowTrainingCurvesTool::new(ports()));
        let out = call(&tool, json!({ "path": "runs/a.csv", "smoothing": 0.5 }))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "Training curves `a.csv` shown to the user: 1 runs (a), metrics loss; showing \
             `loss` against step (axes: step), EMA smoothing 0.5.\n\
             Summaries over all logged values (envelope `ml_fake`, keys \
             `<run>.<metric>.n|min|max|last|best|best_step|last_step`):\n\
             - a loss: best 0.25 at step 30 (lowest, goal guessed), last 0.4 at step 40, min \
             0.25, max 1, n 4"
        );
        assert!(matches!(
            out.block_update,
            Some(BlockKind::TrainingCurves(_))
        ));
        assert!(call(&tool, json!({})).await.is_err());
        assert!(
            call(&tool, json!({ "path": "x", "smoothing": 1.0 }))
                .await
                .is_err()
        );
        let missing = call(&tool, json!({ "path": "runs/none.csv" }))
            .await
            .unwrap();
        assert!(missing.is_error);
    }
}
