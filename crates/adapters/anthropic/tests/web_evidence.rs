//! A recorded web search answer ends up as research evidence: SSE →
//! `LlmEvent`s → `AssembledMessage::citations` → `Evidence`.

use adapter_anthropic::stream::llm_events;
use application::ports::llm::{Citation, LlmEvent, MessageAssembler};
use application::usecases::evidence::citations_to_evidence;
use domain::block::SourceRef;
use domain::model::ModelId;
use domain::research::ResearchState;
use futures::StreamExt;
use futures::executor::block_on;

const WEB_SEARCH: &str = include_str!("fixtures/web_search.sse");
const WEB_SEARCH_CITATIONS: &str = include_str!("fixtures/web_search_citations.sse");

/// Decodes and assembles a recorded stream; returns its citations.
fn citations(sse: &'static str) -> Result<Vec<Citation>, Box<dyn std::error::Error>> {
    let body = futures::stream::iter([Ok::<_, std::io::Error>(sse.as_bytes())]);
    let events: Vec<Result<LlmEvent, _>> =
        block_on(llm_events(body, ModelId::new("claude-opus-5-5")).collect());
    let mut asm = MessageAssembler::new();
    for event in events {
        asm.push(event?)?;
    }
    Ok(asm.finish()?.citations)
}

#[test]
fn web_search_citation_becomes_url_evidence() {
    let mut state = ResearchState::default();
    let ids = citations_to_evidence(
        &citations(WEB_SEARCH).unwrap(),
        &mut state,
        1_700_000_000_000,
    );
    assert_eq!(ids.len(), 1);
    let evidence = state.evidence(ids[0]).unwrap();
    assert_eq!(evidence.claim, "Weather in New York City in May 2025");
    assert_eq!(
        evidence.source,
        SourceRef::Url {
            url: "https://world-weather.info/forecast/usa/new_york/may-2025/".into(),
            title: Some(
                "Weather in New York City in May 2025 (New York) - detailed Weather Forecast for a month"
                    .into()
            ),
        }
    );
    assert_eq!(evidence.collected_at, 1_700_000_000_000);
}

#[test]
fn repeated_citations_are_one_evidence() {
    let cited = citations(WEB_SEARCH_CITATIONS).unwrap();
    assert_eq!(cited.len(), 3, "the stream cites the release notes twice");

    let mut state = ResearchState::default();
    let ids = citations_to_evidence(&cited, &mut state, 5);
    assert_eq!(
        ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["E1", "E2"]
    );
    let claims: Vec<(&str, &SourceRef)> = state
        .evidence
        .iter()
        .map(|e| (e.claim.as_str(), &e.source))
        .collect();
    assert_eq!(
        claims,
        [
            (
                "Tokio v1.50.0 adds task dumps on Linux.",
                &SourceRef::Url {
                    url: "https://github.com/tokio-rs/tokio/releases".into(),
                    title: Some("Tokio releases".into()),
                }
            ),
            (
                "The new runtime metrics are stable.",
                &SourceRef::Url {
                    url: "https://tokio.rs/blog".into(),
                    title: Some("Tokio blog".into()),
                }
            ),
        ]
    );

    // The same answer again (e.g. replayed after a checkpoint) adds nothing.
    assert_eq!(citations_to_evidence(&cited, &mut state, 6), ids);
    assert_eq!(state.evidence.len(), 2);
}
