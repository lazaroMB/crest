//! Turns the citations of a model answer into research evidence.
//!
//! Web search (and other provider tools) return [`Citation`]s with the
//! source URL and the quoted passage. Each distinct passage becomes one
//! [`Evidence`](domain::research::Evidence) whose claim is the quoted text
//! and whose source is a [`SourceRef::Url`], so the Spec can cite it by id.

use domain::block::SourceRef;
use domain::research::{EvidenceId, ResearchState};

use crate::ports::llm::Citation;

/// Records `citations` as evidence in `state` and returns their ids.
///
/// Citations are deduplicated by `(url, cited_text)`: within the slice and
/// against evidence already in `state` (a URL source whose claim is the
/// same text). The result has one id per distinct citation, in first-seen
/// order; a citation already recorded returns its existing id instead of a
/// new one. Each new record is stamped with `now_ms` (Unix milliseconds).
pub fn citations_to_evidence(
    citations: &[Citation],
    state: &mut ResearchState,
    now_ms: u64,
) -> Vec<EvidenceId> {
    let mut ids: Vec<EvidenceId> = Vec::new();
    for citation in citations {
        let id = existing(state, citation).unwrap_or_else(|| {
            state.add_evidence(
                citation.cited_text.clone(),
                SourceRef::Url {
                    url: citation.url.clone(),
                    title: citation.title.clone(),
                },
                now_ms,
            )
        });
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// Id of the evidence that already records `citation`, if any.
fn existing(state: &ResearchState, citation: &Citation) -> Option<EvidenceId> {
    state
        .evidence
        .iter()
        .find_map(|evidence| match &evidence.source {
            SourceRef::Url { url, .. }
                if *url == citation.url && evidence.claim == citation.cited_text =>
            {
                Some(evidence.id)
            }
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use domain::research::Evidence;

    use super::*;

    fn citation(url: &str, text: &str) -> Citation {
        Citation {
            url: url.into(),
            title: Some(format!("Title of {url}")),
            cited_text: text.into(),
        }
    }

    #[test]
    fn each_citation_becomes_url_evidence() {
        let mut state = ResearchState::default();
        let ids = citations_to_evidence(
            &[
                citation("https://a.dev", "a is fast"),
                citation("https://b.dev", "b is safe"),
            ],
            &mut state,
            42,
        );
        assert_eq!(
            ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["E1", "E2"]
        );
        assert_eq!(
            state.evidence[0],
            Evidence {
                id: ids[0],
                claim: "a is fast".into(),
                source: SourceRef::Url {
                    url: "https://a.dev".into(),
                    title: Some("Title of https://a.dev".into()),
                },
                collected_at: 42,
            }
        );
    }

    #[test]
    fn duplicates_are_recorded_once() {
        let mut state = ResearchState::default();
        let ids = citations_to_evidence(
            &[
                citation("https://a.dev", "a is fast"),
                citation("https://a.dev", "a is small"),
                citation("https://a.dev", "a is fast"),
            ],
            &mut state,
            1,
        );
        assert_eq!(ids.len(), 2);
        assert_eq!(state.evidence.len(), 2);

        // A later answer citing the same passage reuses its id.
        let again = citations_to_evidence(
            &[
                citation("https://b.dev", "a is fast"),
                citation("https://a.dev", "a is fast"),
            ],
            &mut state,
            2,
        );
        assert_eq!(again[1], ids[0]);
        assert_eq!(state.evidence.len(), 3);
        assert_eq!(state.evidence[2].collected_at, 2);
    }

    #[test]
    fn no_citations_no_evidence() {
        let mut state = ResearchState::default();
        assert!(citations_to_evidence(&[], &mut state, 0).is_empty());
        assert!(state.evidence.is_empty());
    }
}
