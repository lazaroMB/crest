//! The phase-2 compute tools: `stats_distribution`, and what the model is
//! told about each card beyond its slots ([`notes`]).
//!
//! Tools that read data are [`StatsKind`]s (their `data` is a
//! [`super::DataInput`]); tools that take only parameters are
//! [`UnitsKind`]s with a check before the engine runs.

use std::fmt::Write as _;

use domain::computed::ComputedBody;

use super::{StatsKind, StatsTool};
use crate::ports::compute::{ComputeRequest, DistributionRequest};

/// `stats_distribution`.
#[derive(Debug, Clone, Copy)]
pub struct Distribution;

impl StatsKind for Distribution {
    type Request = DistributionRequest;
    const NAME: &'static str = "stats_distribution";
    const DESCRIPTION: &'static str = "\
Shows the distribution of one numeric variable with crest's deterministic engine: a histogram \
(binning rule and width stated), a Gaussian kernel density (bandwidth rule stated), box and \
violin plots with 1.5×IQR outliers, a normal QQ plot with a pointwise band and the ECDF with \
its DKW band; one per group of `by`. The user sees the plots; you get n, mean, SD, quartiles, \
outlier count, bin width and bandwidth as slots to cite.";
    fn wrap(request: DistributionRequest) -> ComputeRequest {
        ComputeRequest::Distribution(request)
    }
    fn subject(request: &DistributionRequest) -> String {
        match &request.by {
            Some(by) => format!("{} by {by}", request.variable),
            None => request.variable.clone(),
        }
    }
}

/// `stats_distribution`.
pub type StatsDistributionTool = StatsTool<Distribution>;

/// Lines for the model about a phase-2 card that the slots do not say.
pub(super) fn notes(body: &ComputedBody, out: &mut String) {
    match body {
        ComputedBody::Distribution(d) if d.groups.len() > 1 => {
            let names: Vec<&str> = d.groups.iter().filter_map(|g| g.name.as_deref()).collect();
            let _ = writeln!(
                out,
                "Groups of {}: {} (value keys are prefixed `<group>.`).",
                d.by.as_deref().unwrap_or("by"),
                names.join(", ")
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::block::BlockKind;
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::json;

    use super::*;
    use crate::testing::compute::FakeCompute;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::tools::compute::ComputeToolPorts;
    use crate::tools::registry::{Tool as _, ToolContext, Typed};
    use crate::usecases::compute::inline_offload;

    fn ports(engine: Arc<FakeCompute>) -> ComputeToolPorts {
        let fs = InMemoryFs::new(test_root()).with_file("data/cars.csv", "mpg,am\n21,1\n22.8,0\n");
        ComputeToolPorts {
            engine,
            fs: Arc::new(fs),
            offload: inline_offload(),
        }
    }

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_c"))
    }

    #[tokio::test]
    async fn a_distribution_reads_its_file_and_shows_a_card() {
        let engine = Arc::new(FakeCompute::new());
        let tool = Typed(StatsDistributionTool::new(ports(Arc::clone(&engine))));
        let input = json!({ "data": { "path": "data/cars.csv" }, "variable": "mpg", "by": "am" });
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        assert_eq!(prepared.subject, "mpg by am · data/cars.csv");
        let out = prepared.run.await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert!(matches!(
            out.block_update,
            Some(BlockKind::Computed { ref body, .. }) if matches!(**body, ComputedBody::Distribution(_))
        ));
        let ComputeRequest::Distribution(request) = &engine.requests()[0] else {
            panic!("not a distribution request");
        };
        assert_eq!(request.data.source.path.as_deref(), Some("data/cars.csv"));
        let _ = tool.name();
    }
}
