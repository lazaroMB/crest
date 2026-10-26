//! The engineering compute tools: `bode`, `karnaugh`, `beam`, and what the model is told
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

/// `karnaugh`.
#[derive(Debug, Clone, Copy)]
pub struct Karnaugh;

impl UnitsKind for Karnaugh {
    type Request = KarnaughRequest;
    const NAME: &'static str = "karnaugh";
    const DESCRIPTION: &'static str = "\
Minimises a Boolean function of up to six variables with crest's deterministic engine \
(Quine–McCluskey prime implicants, essential primes, then Petrick's method): give `minterms`, \
`maxterms` or an `expression` (`AB' + C`, `!a & b | c`, `A ^ B`), with optional `dont_cares` \
and `variables` (most significant first). The user sees the Karnaugh map with the chosen \
groups, the minimal sum of products and product of sums, the prime implicants and the truth \
table. You get both expressions and their term and literal counts; quote the expressions as \
given, never simplify by hand.";
    fn wrap(request: KarnaughRequest) -> ComputeRequest {
        ComputeRequest::Karnaugh(request)
    }
    fn subject(request: &KarnaughRequest) -> String {
        match &request.expression {
            Some(e) => e.clone(),
            None if !request.maxterms.is_empty() => format!("ΠM({} terms)", request.maxterms.len()),
            None => format!("Σm({} terms)", request.minterms.len()),
        }
    }
    fn check(request: &KarnaughRequest) -> Result<(), String> {
        if request.minterms.is_empty()
            && request.maxterms.is_empty()
            && request
                .expression
                .as_deref()
                .is_none_or(|e| e.trim().is_empty())
        {
            return Err("give `minterms`, `maxterms` or an `expression`".into());
        }
        Ok(())
    }
}

/// `karnaugh`.
pub type KarnaughTool = UnitsTool<Karnaugh>;

/// `beam`.
#[derive(Debug, Clone, Copy)]
pub struct Beam;

impl UnitsKind for Beam {
    type Request = BeamRequest;
    const NAME: &'static str = "beam";
    const DESCRIPTION: &'static str = "\
Analyses a straight beam with crest's deterministic engine (Euler–Bernoulli, direct stiffness \
method, so continuous and propped beams work too): `length`, `supports` (`pin`, `roller`, \
`fixed` at positions), `loads` (point and distributed loads positive downward, moments positive \
counterclockwise), units (`length_unit`, `force_unit`) and optionally `e` and `i` with their \
units for the deflection. The user sees the reactions, the shear, bending-moment and deflection \
diagrams with their extremes; you get the reactions, the largest moments, shear and deflection \
with their positions as slots to cite. State the assumptions (small deflections, uniform E·I).";
    fn wrap(request: BeamRequest) -> ComputeRequest {
        ComputeRequest::Beam(request)
    }
    fn subject(request: &BeamRequest) -> String {
        format!(
            "{} {} span, {} supports, {} loads",
            request.length,
            request.length_unit.symbol(),
            request.supports.len(),
            request.loads.len()
        )
    }
    fn check(request: &BeamRequest) -> Result<(), String> {
        if request.supports.is_empty() {
            return Err("give the `supports` (a cantilever is one `fixed` support)".into());
        }
        if request.e.is_some() != request.i.is_some() {
            return Err("give both `e` and `i` for the deflection, or neither".into());
        }
        Ok(())
    }
}

/// `beam`.
pub type BeamTool = UnitsTool<Beam>;

/// What the model is told about an engineering card beyond its values.
pub(super) fn notes(body: &ComputedBody, out: &mut String) {
    if let ComputedBody::Karnaugh(k) = body {
        let _ = writeln!(
            out,
            "Variables (most significant first): {}.",
            k.variables.join(", ")
        );
        let _ = writeln!(out, "Minimal SOP: F = {}", k.sop_expression);
        let _ = writeln!(out, "Minimal POS: F = {}", k.pos_expression);
        if k.method == domain::computed::CoverMethod::Greedy {
            let _ = writeln!(
                out,
                "The cover was completed greedily and may not be minimal."
            );
        }
    }
    if let ComputedBody::Beam(b) = body {
        let _ = writeln!(
            out,
            "Units: positions in {}, forces in {}, moments in {}, deflection in {} (negative = \
             downward). Support names A, B… run from the left.",
            b.length_unit.symbol(),
            b.force_unit.symbol(),
            b.moment_unit(),
            b.length_unit.deflection_unit().symbol()
        );
        if b.indeterminacy > 0 {
            let _ = writeln!(
                out,
                "Statically indeterminate to degree {}: the reactions assume a uniform E·I.",
                b.indeterminacy
            );
        }
    }
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

    #[tokio::test]
    async fn karnaugh_returns_the_expressions() {
        let engine = Arc::new(FakeCompute::new());
        let tool = Typed(KarnaughTool::new(ports(Arc::clone(&engine))));
        let prepared = tool
            .prepare(
                json!({ "minterms": [2, 3], "variables": ["A", "B"] }),
                &ctx(),
            )
            .await
            .unwrap();
        assert_eq!(prepared.subject, "Σm(2 terms)");
        let text = prepared.run.await.unwrap().text_content();
        assert!(text.contains("Minimal SOP: F = A\n"), "{text}");
        assert!(
            text.contains("Variables (most significant first): A, B."),
            "{text}"
        );
        assert!(
            tool.prepare(json!({ "expression": " " }), &ctx())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn beam_needs_supports_and_both_stiffness_terms() {
        let engine = Arc::new(FakeCompute::new());
        let tool = Typed(BeamTool::new(ports(Arc::clone(&engine))));
        let input = json!({
            "length": 6, "supports": [{ "at": 0 }, { "at": 6, "kind": "roller" }],
            "loads": [{ "kind": "distributed", "at": 0, "value": 10 }]
        });
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        assert_eq!(prepared.subject, "6 m span, 2 supports, 1 loads");
        let text = prepared.run.await.unwrap().text_content();
        assert!(text.contains("moments in kN·m"), "{text}");
        let half = json!({ "length": 6, "supports": [{ "at": 0, "kind": "fixed" }], "loads": [], "e": 200 });
        assert!(tool.prepare(half, &ctx()).await.is_err());
    }
}
