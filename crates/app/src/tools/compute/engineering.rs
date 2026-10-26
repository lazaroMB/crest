//! The engineering compute tools: `bode`, and what the model is told
//! about each engineering card beyond its slots ([`notes`]).

use std::fmt::Write as _;

use domain::computed::ComputedBody;

use super::{UnitsKind, UnitsTool};
use crate::ports::compute::*;

/// `bode`.
#[derive(Debug, Clone, Copy)]
pub struct Bode;

impl UnitsKind for Bode {
    type Request = BodeRequest;
    const NAME: &'static str = "bode";
    const DESCRIPTION: &'static str = "\
Computes the frequency response of a transfer function with crest's deterministic engine: G(s) \
as `numerator`/`denominator` coefficients in s (highest power first) or as `zeros`, `poles` and \
`gain`, with an optional dead time `delay` (seconds). The user sees Bode magnitude and phase \
plots on log-frequency axes with the crossovers marked, the Nyquist curve around −1, the poles \
and zeros, and the closed-loop poles under unity negative feedback. You get the gain and phase \
margins, the crossover frequencies, the DC gain and the −3 dB bandwidth as slots to cite.";
    fn wrap(request: BodeRequest) -> ComputeRequest {
        ComputeRequest::Bode(request)
    }
    fn subject(request: &BodeRequest) -> String {
        if request.denominator.is_empty() {
            format!(
                "{} zeros, {} poles",
                request.zeros.len(),
                request.poles.len()
            )
        } else {
            format!(
                "order {} over order {}",
                request.numerator.len().saturating_sub(1),
                request.denominator.len().saturating_sub(1)
            )
        }
    }
    fn check(request: &BodeRequest) -> Result<(), String> {
        if request.denominator.is_empty() && request.poles.is_empty() {
            return Err(
                "give the `denominator` coefficients (highest power of s first) or the `poles`"
                    .into(),
            );
        }
        Ok(())
    }
}

/// `bode`.
pub type BodeTool = UnitsTool<Bode>;

/// What the model is told about an engineering card beyond its values.
pub(super) fn notes(body: &ComputedBody, out: &mut String) {
    if let ComputedBody::Bode(b) = body {
        let _ = writeln!(out, "G(s) = {}.", b.transfer_text());
        let roots = |rs: &[domain::computed::Complex]| {
            rs.iter().map(|r| r.text(4)).collect::<Vec<_>>().join(", ")
        };
        if !b.poles.is_empty() {
            let _ = writeln!(out, "Poles: {}.", roots(&b.poles));
        }
        if !b.zeros.is_empty() {
            let _ = writeln!(out, "Zeros: {}.", roots(&b.zeros));
        }
        if !b.closed_loop_poles.is_empty() {
            let _ = writeln!(
                out,
                "Closed-loop poles (unity feedback): {}.",
                roots(&b.closed_loop_poles)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::json;

    use super::*;
    use crate::testing::compute::FakeCompute;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::tools::compute::ComputeToolPorts;
    use crate::tools::registry::{Tool as _, ToolContext, Typed};
    use crate::usecases::compute::inline_offload;

    fn ports(engine: Arc<FakeCompute>) -> ComputeToolPorts {
        ComputeToolPorts {
            engine,
            fs: Arc::new(InMemoryFs::new(test_root())),
            offload: inline_offload(),
        }
    }

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    #[tokio::test]
    async fn bode_takes_polynomials_or_poles_and_lists_them() {
        let engine = Arc::new(FakeCompute::new());
        let tool = Typed(BodeTool::new(ports(Arc::clone(&engine))));
        let prepared = tool
            .prepare(json!({ "numerator": [1], "denominator": [1, 1] }), &ctx())
            .await
            .unwrap();
        assert_eq!(prepared.subject, "order 0 over order 1");
        let out = prepared.run.await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        let text = out.text_content();
        assert!(text.contains("G(s) = (1) / (s + 1)."), "{text}");
        assert!(text.contains("Poles: -1.0000."), "{text}");
        assert!(
            text.contains("Closed-loop poles (unity feedback): -2.0000."),
            "{text}"
        );
        assert!(
            tool.prepare(json!({ "numerator": [1] }), &ctx())
                .await
                .is_err()
        );
        assert_eq!(engine.requests().len(), 1);
    }
}
