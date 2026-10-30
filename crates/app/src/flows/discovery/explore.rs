//! Opening the problem gate of a deep discovery: the explorer reads the
//! repository and the web, the engine stores every source's text, checks
//! each claim's span against it, confirms the structural map and the
//! variables through an isolated checker, computes the causal loop
//! diagram and appends one `RecordChecked` per record.
//!
//! Records are reconciled, never duplicated: every exploration re-checks
//! the records already stored together with the new ones, and a gate
//! closed by a reopen is first tried with the stored records alone, so a
//! `/reopen scope` costs two checker calls and no fetch when the sources
//! still support the claims.

use std::collections::BTreeMap;
use std::str::FromStr;

use domain::conversation::Session;
use domain::discovery::claim::normalize;
use domain::discovery::{
    Actor, Claim, ClaimCheck, ClaimId, Cld, Direction, DiscoveryEvent, RecordRef, RecordVerdict,
    Section, Sign, SourceId, SourceKind, check_claim, check_claim_with,
};
use domain::model::ModelRole;
use domain::workspace::WorkspacePath;
use serde::{Deserialize, Serialize};

use super::engine::{DiscoveryEngine, Loaded, StepOutcome};
use super::error::DiscoveryError;
use super::records::{self, ClaimRecord, EvidenceRecord, variable_names};
use crate::flows::research::json::{final_text, parse_valid, retry_message};
use crate::ports::discovery::{DiscoveryId, SourceText, StoredSource, text_hash};
use crate::ports::events::NoticeLevel;
use crate::ports::fs::ReadFileRequest;
use crate::usecases::run_turn::TurnOptions;

/// Most variables the explorer may name.
pub const MAX_VARIABLES: usize = 8;
/// Most sources the engine fetches per exploration.
pub const MAX_SOURCES: usize = 12;
/// Most characters of a source kept.
pub const MAX_SOURCE_CHARS: usize = 60_000;

/// A source the explorer relied on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerSource {
    /// URL or workspace path.
    pub location: String,
    /// `study`, `book`, `standard`, `encyclopedia`, `vendor`, `article`,
    /// `repo` or `other`.
    #[serde(default)]
    pub kind: String,
    /// Title, if known.
    #[serde(default)]
    pub title: Option<String>,
}

/// A claim as the explorer states it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerClaim {
    /// The cause.
    pub origin: String,
    /// The effect.
    pub destination: String,
    /// One of the four canonical phrases.
    pub direction: String,
    /// `+` or `-`.
    pub stated_sign: String,
    /// A verbatim span of the source.
    pub span: String,
    /// 1-based index into `sources`.
    pub source: usize,
}

/// Evidence as the explorer states it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerEvidence {
    /// The statement.
    pub claim: String,
    /// A verbatim span of the source.
    pub span: String,
    /// 1-based index into `sources`.
    pub source: usize,
}

/// What the explorer answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exploration {
    /// The sources, in the order the indexes refer to.
    #[serde(default)]
    pub sources: Vec<ExplorerSource>,
    /// The structural map, Markdown.
    #[serde(default)]
    pub structure: String,
    /// The variables, `name: how it moves`.
    #[serde(default)]
    pub variables: Vec<String>,
    /// The causal claims.
    #[serde(default)]
    pub claims: Vec<ExplorerClaim>,
    /// The evidence.
    #[serde(default)]
    pub evidence: Vec<ExplorerEvidence>,
}

fn validate(e: &Exploration) -> Result<(), String> {
    if e.sources.is_empty() {
        return Err("at least one source is required".into());
    }
    if e.sources.len() > MAX_SOURCES {
        return Err(format!("at most {MAX_SOURCES} sources"));
    }
    if e.structure.trim().is_empty() {
        return Err("`structure` cannot be empty".into());
    }
    if e.variables.is_empty() || e.variables.len() > MAX_VARIABLES {
        return Err(format!("between 1 and {MAX_VARIABLES} variables"));
    }
    if e.claims.is_empty() {
        return Err("at least one claim is required".into());
    }
    for c in &e.claims {
        if c.source == 0 || c.source > e.sources.len() {
            return Err(format!("claim source index {} is out of range", c.source));
        }
        if Direction::from_str(&c.direction).is_err() {
            return Err(format!(
                "direction \"{}\" is not one of: {}",
                c.direction,
                [
                    Direction::IncreasingIncreases,
                    Direction::DecreasingDecreases,
                    Direction::IncreasingDecreases,
                    Direction::DecreasingIncreases,
                ]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
            ));
        }
        if Sign::parse(&c.stated_sign).is_none() {
            return Err(format!("stated_sign \"{}\" must be + or -", c.stated_sign));
        }
    }
    for ev in &e.evidence {
        if ev.source == 0 || ev.source > e.sources.len() {
            return Err(format!(
                "evidence source index {} is out of range",
                ev.source
            ));
        }
    }
    Ok(())
}

/// The explorer's instructions.
pub const EXPLORER_TASK: &str = "\
Investigate the problem below for a deep discovery. Read the repository where it applies and \
the open web where the question is one of knowledge: studies, standards, books, encyclopedias, \
documentation. Prefer independent sources over a vendor's own documentation; when you use a \
vendor's, say so in `kind`.\n\
Then answer with only a JSON object:\n\
{\"sources\": [{\"location\": \"<url or workspace path>\", \"kind\": \"study|book|standard|encyclopedia|vendor|article|repo|other\", \"title\": \"...\"}],\n \
 \"structure\": \"<markdown: the parts of the system inside the scope, their connections, and what crosses the boundary>\",\n \
 \"variables\": [\"<name>: <how it moves and what moves it>\"],\n \
 \"claims\": [{\"origin\": \"<variable name>\", \"destination\": \"<variable name>\", \"direction\": \"increasing origin increases destination | decreasing origin decreases destination | increasing origin decreases destination | decreasing origin increases destination\", \"stated_sign\": \"+|-\", \"span\": \"<verbatim words copied from the source>\", \"source\": <1-based index into sources>}],\n \
 \"evidence\": [{\"claim\": \"<a fact>\", \"span\": \"<verbatim words copied from the source>\", \"source\": <index>}]}\n\
Rules: at most 8 variables; every claim's origin and destination must be variable names exactly as listed; every span must be copied verbatim from the page or file you read (it will be checked against the stored text and the claim dropped if it is not found); sources are URLs you fetched or workspace paths you read, nothing else.";

/// Opens the gate of `section` (`Findings` or `Current system`): first by
/// re-checking the records already stored, then, if that is not enough,
/// by exploring again.
pub(crate) async fn open_problem_gate(
    engine: &DiscoveryEngine,
    id: &DiscoveryId,
    loaded: &Loaded,
    section: Section,
    missing: Vec<String>,
) -> Result<StepOutcome, DiscoveryError> {
    let mut produced = Vec::new();
    // Records checked before the current `Scope` verdict (a reopen) are
    // re-checked first; records that already failed to open the gate
    // after it are not worth checking again.
    let scope = loaded.state.approved_at(Section::Scope).unwrap_or(0);
    let stale = loaded.state.records.values().any(|r| r.seq < scope);
    if stale && let Some(rechecked) = recheck_records(engine, id).await? {
        produced = rechecked;
        let reloaded = engine.load(id).await?;
        if reloaded.state.gate_missing(section).is_empty() {
            engine.render(id).await?;
            return Ok(StepOutcome::GateOpened { section, produced });
        }
        engine.notice(
            NoticeLevel::Info,
            format!("the stored records do not open the gate of `{section}`; exploring again"),
        );
    }
    let Some(explorer) = engine.deps().explorer.clone() else {
        return Ok(StepOutcome::GateClosed {
            section,
            missing: with_hint(missing, "no explorer agent is configured"),
            produced,
        });
    };
    let state = &loaded.state;
    let mut prompt = format!("{EXPLORER_TASK}\n\n# Problem\n\n{}\n", state.problem.trim());
    for s in [
        Section::Goal,
        Section::Background,
        Section::Constraints,
        Section::Scope,
    ] {
        if let Some(text) = engine.section_text(id, s).await? {
            prompt.push_str(&format!("\n## {s}\n\n{}\n", text.trim()));
        }
    }
    engine.notice(
        NoticeLevel::Info,
        format!("exploring for the gate of `{section}`"),
    );
    let mut session = Session::new(engine.workspace_root().to_owned(), engine.now());
    let options = TurnOptions::new(ModelRole::Explore);
    explorer
        .run_turn(&mut session, prompt, options.clone())
        .await?;
    let exploration: Exploration = match parse_valid(&final_text(&session), validate) {
        Ok(e) => e,
        Err(error) => {
            explorer
                .run_turn(&mut session, retry_message(&error), options)
                .await?;
            parse_valid(&final_text(&session), validate)
                .map_err(|e| DiscoveryError::Model(format!("exploration: {e}")))?
        }
    };
    produced.extend(reconcile(engine, id, Some(&exploration)).await?);
    let reloaded = engine.load(id).await?;
    let still = reloaded.state.gate_missing(section);
    engine.render(id).await?;
    if still.is_empty() {
        Ok(StepOutcome::GateOpened { section, produced })
    } else {
        Ok(StepOutcome::GateClosed {
            section,
            missing: still,
            produced,
        })
    }
}

fn with_hint(mut missing: Vec<String>, hint: &str) -> Vec<String> {
    missing.push(hint.to_owned());
    missing
}

/// The kind named by the explorer.
#[must_use]
pub fn parse_kind(kind: &str) -> SourceKind {
    match kind.trim().to_lowercase().as_str() {
        "study" | "paper" | "preprint" => SourceKind::Study,
        "book" => SourceKind::Book,
        "standard" | "rfc" | "spec" => SourceKind::Standard,
        "encyclopedia" | "reference" => SourceKind::Encyclopedia,
        "vendor" | "docs" => SourceKind::Vendor,
        "article" | "blog" => SourceKind::Article,
        "repo" | "repository" | "file" | "code" => SourceKind::Repo,
        _ => SourceKind::Other,
    }
}

/// Fetches `source` as text: the web for URLs, the workspace for paths.
async fn fetch(
    engine: &DiscoveryEngine,
    source: &ExplorerSource,
) -> Option<(String, Option<String>)> {
    let location = source.location.trim();
    if location.starts_with("http://") || location.starts_with("https://") {
        let web = engine.deps().web.as_ref()?;
        let page = web.fetch(location).await.ok()?;
        return Some((page.text, page.title.or_else(|| source.title.clone())));
    }
    let fs = engine.deps().fs.as_ref()?;
    let path = WorkspacePath::parse(location).ok()?;
    let content = fs
        .read_file(ReadFileRequest {
            path,
            offset: None,
            limit: Some(20_000),
        })
        .await
        .ok()?;
    Some((content.lines.join("\n"), source.title.clone()))
}

/// Re-checks the stored records against the stored sources without
/// exploring: the structure and the variables through the checker, the
/// spans and the CLD deterministically. `None` when nothing was ever
/// explored.
pub(crate) async fn recheck_records(
    engine: &DiscoveryEngine,
    id: &DiscoveryId,
) -> Result<Option<Vec<String>>, DiscoveryError> {
    let structure = engine
        .deps()
        .log
        .artifact(id, records::names::STRUCTURE)
        .await?;
    let claims: Vec<ClaimRecord> = engine.artifact_json(id, records::names::CLAIMS).await?;
    if structure.is_none() || claims.is_empty() {
        return Ok(None);
    }
    engine.notice(
        NoticeLevel::Info,
        format!(
            "re-checking {} stored claim(s) and the method records before exploring again",
            claims.len()
        ),
    );
    let mut produced = vec!["re-checked the stored records".to_owned()];
    produced.extend(reconcile(engine, id, None).await?);
    Ok(Some(produced))
}

/// How the sources of an exploration were stored.
pub(super) struct Stored {
    /// Per explorer index, the stored id (`None` when unreadable).
    pub(super) ids: Vec<Option<SourceId>>,
    /// Sources fetched and stored anew.
    pub(super) fetched: usize,
    /// Sources already stored with the same location and text.
    pub(super) reused: usize,
}

/// Stores the sources of `exploration` the engine can read, reusing a
/// stored source with the same location and text.
pub(super) async fn store_sources(
    engine: &DiscoveryEngine,
    id: &DiscoveryId,
    sources: &[ExplorerSource],
    existing: &[StoredSource],
    texts: &mut BTreeMap<SourceId, String>,
) -> Result<Stored, DiscoveryError> {
    let mut stored = Vec::with_capacity(sources.len());
    let mut fetched = 0usize;
    let mut reused = 0usize;
    // The explorer cites one page for several claims and may list it more
    // than once: a location is fetched and stored once per exploration, and
    // its claims resolve to that one id.
    let mut seen: BTreeMap<&str, Option<SourceId>> = BTreeMap::new();
    for source in sources {
        if let Some(first) = seen.get(source.location.trim()) {
            stored.push(*first);
            continue;
        }
        let Some((text, title)) = fetch(engine, source).await else {
            engine.notice(
                NoticeLevel::Warning,
                format!(
                    "could not read source {}; its claims become open questions",
                    source.location
                ),
            );
            seen.insert(source.location.trim(), None);
            stored.push(None);
            continue;
        };
        let text: String = text.chars().take(MAX_SOURCE_CHARS).collect();
        let hash = text_hash(&text);
        let location = source.location.trim();
        if let Some(same) = existing
            .iter()
            .find(|s| s.source.location == location && s.hash == hash)
        {
            reused += 1;
            seen.insert(source.location.trim(), Some(same.id));
            stored.push(Some(same.id));
            continue;
        }
        let kind = parse_kind(&source.kind);
        let put = engine
            .deps()
            .sources
            .put(
                id,
                SourceText {
                    location: location.to_owned(),
                    title,
                    kind,
                    text: text.clone(),
                    fetched_at: engine.now(),
                },
            )
            .await?;
        engine
            .append(
                id,
                DiscoveryEvent::SourceFetched {
                    source: put.id,
                    hash: put.hash.clone(),
                    location: put.source.location.clone(),
                    kind,
                },
            )
            .await?;
        if let Some(sink) = &engine.deps().sink {
            let _ = sink
                .write(id.as_str(), &format!("sources/{}.txt", put.id), &text)
                .await;
        }
        texts.insert(put.id, text);
        fetched += 1;
        seen.insert(source.location.trim(), Some(put.id));
        stored.push(Some(put.id));
    }
    Ok(Stored {
        ids: stored,
        fetched,
        reused,
    })
}

/// Stores (or keeps) the structure and the variables and has both
/// confirmed by the isolated checker. Returns the variables record.
async fn confirm_method_records(
    engine: &DiscoveryEngine,
    id: &DiscoveryId,
    exploration: Option<&Exploration>,
    produced: &mut Vec<String>,
) -> Result<String, DiscoveryError> {
    let log = &engine.deps().log;
    let structure = match exploration {
        Some(e) => {
            let text = e.structure.trim().to_owned();
            log.put_artifact(id, records::names::STRUCTURE, &text)
                .await?;
            text
        }
        None => log
            .artifact(id, records::names::STRUCTURE)
            .await?
            .unwrap_or_default(),
    };
    let variables_md = match exploration {
        Some(e) => {
            let md = e
                .variables
                .iter()
                .map(|v| format!("- {}", v.trim()))
                .collect::<Vec<_>>()
                .join("\n");
            log.put_artifact(id, records::names::VARIABLES, &md).await?;
            md
        }
        None => log
            .artifact(id, records::names::VARIABLES)
            .await?
            .unwrap_or_default(),
    };
    let checker = engine.actor_for(ModelRole::Explore);
    for (record, text) in [
        (RecordRef::Structure, structure.as_str()),
        (RecordRef::Variables, variables_md.as_str()),
    ] {
        let verdict = check_record_text(engine, record, text).await?;
        engine
            .append(
                id,
                DiscoveryEvent::RecordChecked {
                    record,
                    verdict,
                    by: checker.clone(),
                },
            )
            .await?;
        produced.push(format!("{record}: {verdict:?}"));
    }
    Ok(variables_md)
}

/// Whether `span` is found verbatim (whitespace collapsed) in `text`.
fn span_found(span: &str, text: Option<&String>) -> bool {
    let span = normalize(span);
    !span.is_empty() && text.is_some_and(|t| normalize(t).contains(&span))
}

/// The stored evidence plus the exploration's, without duplicates (same
/// statement, span and source).
fn merge_evidence(
    mut existing: Vec<EvidenceRecord>,
    exploration: Option<&Exploration>,
    stored: &[Option<SourceId>],
) -> Vec<EvidenceRecord> {
    let Some(e) = exploration else {
        return existing;
    };
    let mut next = existing.iter().map(|r| r.id).max().unwrap_or(0) + 1;
    for ev in &e.evidence {
        let Some(Some(source)) = stored.get(ev.source.wrapping_sub(1)) else {
            continue;
        };
        let claim = ev.claim.trim().to_owned();
        let span = ev.span.trim().to_owned();
        let same = existing.iter().any(|r| {
            r.source == *source && r.claim == claim && normalize(&r.span) == normalize(&span)
        });
        if same {
            continue;
        }
        existing.push(EvidenceRecord {
            id: next,
            claim,
            source: *source,
            span,
        });
        next += 1;
    }
    existing
}

/// The stored claims plus the exploration's, without duplicates (same
/// origin, destination, direction, span and source).
fn merge_claims(
    mut existing: Vec<ClaimRecord>,
    exploration: Option<&Exploration>,
    stored: &[Option<SourceId>],
) -> Vec<ClaimRecord> {
    let Some(e) = exploration else {
        return existing;
    };
    let mut next = existing.iter().map(|c| c.claim.id.0).max().unwrap_or(0) + 1;
    for c in &e.claims {
        let (Ok(direction), Some(sign)) = (
            Direction::from_str(&c.direction),
            Sign::parse(&c.stated_sign),
        ) else {
            continue;
        };
        // A source the engine could not read still yields a claim: an
        // open question with `S0`, so the diagram shows the hole.
        let source = stored
            .get(c.source.wrapping_sub(1))
            .copied()
            .flatten()
            .unwrap_or(SourceId(0));
        let claim = Claim {
            id: ClaimId(next),
            origin: c.origin.trim().to_owned(),
            destination: c.destination.trim().to_owned(),
            direction,
            stated_sign: sign,
            span: c.span.trim().to_owned(),
            source,
        };
        let same = existing.iter().any(|r| {
            r.claim.source == claim.source
                && r.claim.direction == claim.direction
                && normalize(&r.claim.origin).eq_ignore_ascii_case(&normalize(&claim.origin))
                && normalize(&r.claim.destination)
                    .eq_ignore_ascii_case(&normalize(&claim.destination))
                && normalize(&r.claim.span) == normalize(&claim.span)
        });
        if same {
            continue;
        }
        existing.push(ClaimRecord { claim });
        next += 1;
    }
    existing
}

/// Reconciles the records with `exploration` (or with nothing new, on a
/// re-check): stores the sources, merges and checks the evidence and the
/// claims against the stored texts, confirms the method records and
/// rebuilds the CLD. Every record gets a fresh `RecordChecked`.
async fn reconcile(
    engine: &DiscoveryEngine,
    id: &DiscoveryId,
    exploration: Option<&Exploration>,
) -> Result<Vec<String>, DiscoveryError> {
    let mut produced = Vec::new();
    let existing: Vec<StoredSource> = engine.deps().sources.sources(id).await?;
    let mut texts: BTreeMap<SourceId, String> = existing
        .iter()
        .map(|s| (s.id, s.source.text.clone()))
        .collect();
    let stored = match exploration {
        Some(e) => {
            let stored = store_sources(engine, id, &e.sources, &existing, &mut texts).await?;
            produced.push(match stored.reused {
                0 => format!("{} source(s)", stored.fetched),
                reused => format!("{} source(s), {reused} reused", stored.fetched + reused),
            });
            stored.ids
        }
        None => Vec::new(),
    };

    let variables_md = confirm_method_records(engine, id, exploration, &mut produced).await?;

    // Evidence: every span, old and new, checked against the stored text.
    let process = Actor::Process("span-check".into());
    let evidence = merge_evidence(
        engine.artifact_json(id, records::names::EVIDENCE).await?,
        exploration,
        &stored,
    );
    for record in &evidence {
        let found = span_found(&record.span, texts.get(&record.source));
        engine
            .append(
                id,
                DiscoveryEvent::RecordChecked {
                    record: RecordRef::Evidence { id: record.id },
                    verdict: if found {
                        RecordVerdict::Confirmed
                    } else {
                        RecordVerdict::Unverifiable
                    },
                    by: process.clone(),
                },
            )
            .await?;
    }
    engine
        .put_json(id, records::names::EVIDENCE, &evidence)
        .await?;
    produced.push(format!("{} evidence record(s)", evidence.len()));

    // Claims: against the stored text and the canonical variables.
    let names = variable_names(&variables_md);
    let claims = merge_claims(
        engine.artifact_json(id, records::names::CLAIMS).await?,
        exploration,
        &stored,
    );
    let mut checked: Vec<(Claim, ClaimCheck)> = Vec::with_capacity(claims.len());
    for record in &claims {
        let text = texts.get(&record.claim.source).map(String::as_str);
        let check = if names.is_empty() {
            check_claim(&record.claim, text)
        } else {
            check_claim_with(&record.claim, text, &names)
        };
        let verdict = match &check {
            ClaimCheck::Keep { .. } => RecordVerdict::Confirmed,
            ClaimCheck::Open { .. } => RecordVerdict::Inconclusive,
            ClaimCheck::Reject { .. } => RecordVerdict::Refuted,
        };
        engine
            .append(
                id,
                DiscoveryEvent::RecordChecked {
                    record: RecordRef::Claim {
                        id: record.claim.id.0,
                    },
                    verdict,
                    by: process.clone(),
                },
            )
            .await?;
        checked.push((record.claim.clone(), check));
    }
    engine.put_json(id, records::names::CLAIMS, &claims).await?;
    let kept = checked
        .iter()
        .filter(|(_, c)| matches!(c, ClaimCheck::Keep { .. }))
        .count();
    produced.push(format!("{kept} of {} claim(s) kept", checked.len()));

    // The CLD, computed, never written by a model.
    let cld = Cld::build(&checked);
    let cld_md = format!(
        "{}\n\n{} node(s), {} edge(s), {} loop(s); {} open, {} rejected.\n",
        cld.to_mermaid(),
        cld.nodes.len(),
        cld.edges.len(),
        cld.loops.len(),
        cld.open.len(),
        cld.rejected.len()
    );
    engine
        .deps()
        .log
        .put_artifact(id, records::names::CLD, &cld_md)
        .await?;
    engine
        .append(
            id,
            DiscoveryEvent::RecordChecked {
                record: RecordRef::Cld,
                verdict: if cld.edges.is_empty() {
                    RecordVerdict::Inconclusive
                } else {
                    RecordVerdict::Confirmed
                },
                by: Actor::Process("cld".into()),
            },
        )
        .await?;
    produced.push(format!(
        "cld: {} edge(s), {} loop(s)",
        cld.edges.len(),
        cld.loops.len()
    ));
    Ok(produced)
}

/// What the isolated checker answers about a record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordCheck {
    /// Whether the record stands.
    pub ok: bool,
    /// Why not, when it does not.
    #[serde(default)]
    pub why: String,
}

/// The checker's instructions.
pub const CHECKER_SYSTEM: &str = "\
You check one record of a deep discovery. Answer with only a JSON object {\"ok\": true|false, \
\"why\": \"...\"}. A `structure` record is ok when it names the parts inside the scope, their \
connections and what crosses the boundary, without proposing anything. A `variables` record is ok \
when it lists at most 8 directional variables, each one a quantity that can go up or down. A \
`risks` record is ok when every risk names the loop or pathway it comes from and says what would \
make it worse. Say exactly what is wrong when it is not ok.";

/// Checks `text` as `record` through the isolated checker.
pub(crate) async fn check_record_text(
    engine: &DiscoveryEngine,
    record: RecordRef,
    text: &str,
) -> Result<RecordVerdict, DiscoveryError> {
    let check = engine
        .model()
        .ask_json::<RecordCheck>(
            ModelRole::Explore,
            CHECKER_SYSTEM,
            &format!("# Record `{record}`\n\n{}\n", text.trim()),
            &format!("checking {record}"),
            |c| {
                if !c.ok && c.why.trim().is_empty() {
                    return Err("say why it is not ok".into());
                }
                Ok(())
            },
        )
        .await?;
    if check.ok {
        Ok(RecordVerdict::Confirmed)
    } else {
        engine.notice(
            NoticeLevel::Warning,
            format!("record `{record}` not confirmed: {}", check.why.trim()),
        );
        Ok(RecordVerdict::Inconclusive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_parse_loosely() {
        assert_eq!(parse_kind("Study"), SourceKind::Study);
        assert_eq!(parse_kind("docs"), SourceKind::Vendor);
        assert_eq!(parse_kind("repo"), SourceKind::Repo);
        assert_eq!(parse_kind("??"), SourceKind::Other);
    }

    #[test]
    fn exploration_validation_checks_indexes_and_phrases() {
        let mut e = Exploration {
            sources: vec![ExplorerSource {
                location: "https://x".into(),
                kind: "study".into(),
                title: None,
            }],
            structure: "parts".into(),
            variables: vec!["a: up".into(), "b: down".into()],
            claims: vec![ExplorerClaim {
                origin: "a".into(),
                destination: "b".into(),
                direction: "increasing origin decreases destination".into(),
                stated_sign: "-".into(),
                span: "s".into(),
                source: 1,
            }],
            evidence: vec![],
        };
        assert_eq!(validate(&e), Ok(()));
        e.claims[0].source = 2;
        assert!(validate(&e).unwrap_err().contains("out of range"));
        e.claims[0].source = 1;
        e.claims[0].direction = "more a more b".into();
        assert!(validate(&e).unwrap_err().contains("not one of"));
    }
}
