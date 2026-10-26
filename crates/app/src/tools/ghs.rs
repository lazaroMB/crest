//! `show_ghs`: a GHS hazard card from PubChem, fetched through the
//! [`WebPort`] (so `app.opt.web`'s switch and domain rules apply).
//!
//! A name or CAS number is resolved to a compound id with PUG REST
//! (`/rest/pug/compound/name/<name>/cids/JSON`), then the compound's "GHS
//! Classification" section is read with PUG View
//! (`/rest/pug_view/data/compound/<cid>/JSON?heading=GHS+Classification`).
//! [`parse_ghs`] keeps, per source (EU CLP, the ECHA C&L inventory...),
//! the pictogram codes, signal word, hazard statements as written and the
//! precautionary statement codes (PubChem gives no P texts there), with
//! the source's name, link and licence note. Nothing is written by crest
//! or the model: an unknown compound or one without a classification is a
//! card that says so, and the summary tells the model not to fill in.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::permission::ToolEffect;
use domain::science::ghs::{GhsClassification, GhsSpec, HazardStatement, Pictogram, utc_date};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::ports::web::{WebError, WebPort};

/// Name of `show_ghs`.
pub const SHOW_GHS: &str = "show_ghs";
/// PubChem's base URL.
pub const PUBCHEM: &str = "https://pubchem.ncbi.nlm.nih.gov";
/// Most classifications kept.
const MAX_SOURCES: usize = 20;
/// Most hazard statements kept per source.
const MAX_HAZARDS: usize = 60;
/// Hazard statements listed to the model per source.
const LISTED_HAZARDS: usize = 20;

/// What the system prompt says about `show_ghs`, when the web is on.
pub const GHS_TOOL_PROMPT: &str = "\
# Chemical safety
- `show_ghs`: when hazards, safe handling or a safety data sheet of a \
chemical come up, look up its GHS classification on PubChem and show the \
card; quote only the pictograms, signal word and statements it returns, \
with its source and date. When it finds nothing, say the hazards could not \
be verified instead of giving them from memory.";

/// Input of `show_ghs`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowGhs {
    /// The chemical: a name (`acetone`) or a CAS number (`67-64-1`).
    #[serde(default)]
    pub query: Option<String>,
    /// Or its PubChem compound id (CID), e.g. `180`.
    #[serde(default)]
    pub cid: Option<u64>,
}

/// `show_ghs`: a GHS card from PubChem.
#[derive(Clone)]
pub struct ShowGhsTool {
    web: Arc<dyn WebPort>,
    now: fn() -> u64,
}

impl ShowGhsTool {
    /// The tool over `web`, dating cards with `now` (ms since the epoch).
    #[must_use]
    pub fn new(web: Arc<dyn WebPort>, now: fn() -> u64) -> Self {
        Self { web, now }
    }
}

/// `text` percent-encoded for a URL path segment.
#[must_use]
pub fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// The PUG REST URL resolving `name` to compound ids.
#[must_use]
pub fn cids_url(name: &str) -> String {
    format!(
        "{PUBCHEM}/rest/pug/compound/name/{}/cids/JSON",
        encode(name.trim())
    )
}

/// The PUG View URL of compound `cid`'s GHS section.
#[must_use]
pub fn ghs_url(cid: u64) -> String {
    format!("{PUBCHEM}/rest/pug_view/data/compound/{cid}/JSON?heading=GHS+Classification")
}

/// The page a person checks: the compound's GHS section on PubChem.
#[must_use]
pub fn page_url(cid: u64) -> String {
    format!("{PUBCHEM}/compound/{cid}#section=GHS-Classification")
}

/// The compound ids of a PUG REST `cids` answer.
///
/// # Errors
///
/// A message when the text is not that JSON.
pub fn parse_cids(text: &str) -> Result<Vec<u64>, String> {
    let v: Value =
        serde_json::from_str(text).map_err(|e| format!("PubChem's answer is not JSON: {e}"))?;
    Ok(v["IdentifierList"]["CID"]
        .as_array()
        .map(|ids| ids.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default())
}

/// The strings of a PUG View `Value.StringWithMarkup`.
fn strings(value: &Value) -> Vec<&Value> {
    value["StringWithMarkup"]
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

fn find_section<'a>(sections: &'a Value, heading: &str) -> Option<&'a Value> {
    for s in sections.as_array()? {
        if s["TOCHeading"].as_str() == Some(heading) {
            return Some(s);
        }
        if let Some(found) = find_section(&s["Section"], heading) {
            return Some(found);
        }
    }
    None
}

/// A hazard statement line: `H225 (> 99.9%): text [class]`.
fn hazard(line: &str) -> Option<HazardStatement> {
    let line = line.trim();
    let (head, _) = line.split_once(':')?;
    let mut parts = head.splitn(2, ' ');
    let code = parts.next()?.trim();
    if !(code.starts_with('H') || code.starts_with("EUH") || code.starts_with("AUH")) {
        return None;
    }
    let share = parts
        .next()
        .map(|s| {
            s.trim()
                .trim_start_matches('(')
                .trim_end_matches(')')
                .to_owned()
        })
        .filter(|s| !s.is_empty());
    Some(HazardStatement {
        code: code.to_owned(),
        text: line.to_owned(),
        share,
    })
}

/// The P codes of `P210, P233, ... and P501 (click each P-code ...)`.
fn p_codes(line: &str) -> Vec<String> {
    let line = line.split(" (click").next().unwrap_or(line);
    line.split([',', ' '])
        .map(str::trim)
        .filter(|w| {
            w.starts_with('P') && w.len() > 1 && w[1..].starts_with(|c: char| c.is_ascii_digit())
        })
        .map(str::to_owned)
        .collect()
}

/// The pictogram code in an icon URL (`.../GHS02.svg` → `GHS02`).
fn pictogram_code(url: &str) -> Option<String> {
    let file = url.rsplit('/').next()?;
    let code = file.split('.').next()?;
    (code.starts_with("GHS") && code.len() == 5).then(|| code.to_owned())
}

/// What a PUG View GHS record holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhsRecord {
    /// The compound id.
    pub cid: Option<u64>,
    /// PubChem's name of the compound.
    pub name: Option<String>,
    /// The classifications, one per source.
    pub classifications: Vec<GhsClassification>,
}

/// The compound id, name and classifications of a PUG View record.
///
/// # Errors
///
/// A message when the text is not a PUG View record.
pub fn parse_ghs(text: &str) -> Result<GhsRecord, String> {
    let v: Value =
        serde_json::from_str(text).map_err(|e| format!("PubChem's answer is not JSON: {e}"))?;
    let record = &v["Record"];
    if record.is_null() {
        return Err("PubChem's answer has no Record".into());
    }
    let cid = record["RecordNumber"].as_u64();
    let name = record["RecordTitle"].as_str().map(str::to_owned);
    let mut references: BTreeMap<u64, &Value> = BTreeMap::new();
    for r in record["Reference"].as_array().into_iter().flatten() {
        if let Some(n) = r["ReferenceNumber"].as_u64() {
            references.insert(n, r);
        }
    }
    let Some(section) = find_section(&record["Section"], "GHS Classification") else {
        return Ok(GhsRecord {
            cid,
            name,
            classifications: Vec::new(),
        });
    };
    // Classifications by source, in the record's order.
    let mut order: Vec<u64> = Vec::new();
    let mut by_ref: BTreeMap<u64, GhsClassification> = BTreeMap::new();
    for info in section["Information"].as_array().into_iter().flatten() {
        let reference = info["ReferenceNumber"].as_u64().unwrap_or(0);
        let entry = by_ref.entry(reference).or_insert_with(|| {
            order.push(reference);
            let r = references.get(&reference);
            let text = |key: &str| {
                r.and_then(|r| r[key].as_str())
                    .map(str::to_owned)
                    .filter(|s| !s.is_empty())
            };
            GhsClassification {
                source: text("SourceName").unwrap_or_else(|| "PubChem".to_owned()),
                substance: text("Name"),
                url: text("URL"),
                license: text("LicenseNote"),
                pictograms: Vec::new(),
                signal: None,
                hazards: Vec::new(),
                precautionary: Vec::new(),
                notes: Vec::new(),
            }
        });
        let label = info["Name"].as_str().unwrap_or("");
        let items = strings(&info["Value"]);
        match label {
            "Pictogram(s)" => {
                for s in items {
                    for m in s["Markup"].as_array().into_iter().flatten() {
                        if let Some(code) = m["URL"].as_str().and_then(pictogram_code)
                            && !entry.pictograms.iter().any(|p| p.code == code)
                        {
                            entry.pictograms.push(Pictogram {
                                code,
                                label: m["Extra"].as_str().map(str::to_owned),
                            });
                        }
                    }
                }
            }
            "Signal" => {
                entry.signal = items
                    .first()
                    .and_then(|s| s["String"].as_str())
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty());
            }
            "GHS Hazard Statements" => {
                for s in items {
                    if entry.hazards.len() >= MAX_HAZARDS {
                        break;
                    }
                    if let Some(h) = s["String"].as_str().and_then(hazard) {
                        entry.hazards.push(h);
                    }
                }
            }
            "Precautionary Statement Codes" => {
                for s in items {
                    if let Some(line) = s["String"].as_str() {
                        entry.precautionary.extend(p_codes(line));
                    }
                }
            }
            _ => {
                for s in items {
                    if let Some(line) = s["String"].as_str() {
                        let line = line.trim();
                        if !line.is_empty() {
                            entry.notes.push(line.to_owned());
                        }
                    }
                }
            }
        }
    }
    let classifications = order
        .into_iter()
        .filter_map(|r| by_ref.remove(&r))
        .filter(|c| c.signal.is_some() || !c.hazards.is_empty() || !c.pictograms.is_empty())
        .take(MAX_SOURCES)
        .collect();
    Ok(GhsRecord {
        cid,
        name,
        classifications,
    })
}

#[async_trait]
impl TypedTool for ShowGhsTool {
    type Input = ShowGhs;

    fn name(&self) -> &str {
        SHOW_GHS
    }

    fn description(&self) -> &str {
        "Looks a chemical up on PubChem (by name, CAS number or CID) and \
         shows its GHS hazard card in the user's timeline: pictograms, \
         signal word, hazard statements and precautionary statement codes \
         per source (EU CLP, ECHA C&L inventory...), with the PubChem link \
         and the retrieval date. Returns exactly what PubChem gives, or that \
         it gave nothing; never add hazards it did not return."
    }

    fn effect(&self, _input: &ShowGhs) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(&self, input: ShowGhs, _ctx: &ToolContext) -> Result<PreparedCall, ToolError> {
        let (query, cid) =
            target(input.query, input.cid).map_err(|message| ToolError::InvalidInput {
                tool: SHOW_GHS.into(),
                message,
            })?;
        let web = Arc::clone(&self.web);
        let now = self.now;
        let subject = query.clone();
        let run = async move {
            Ok(match lookup(web.as_ref(), &query, cid, now()).await {
                Ok(spec) => output(spec),
                Err(message) => ToolOutput::error(message),
            })
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What to look up: the query as shown and the compound id when it is
/// one (`cid`, `CID 180`, `180`; a CAS number like `67-64-1` is a name).
///
/// # Errors
///
/// A message when both or neither are given.
pub fn target(query: Option<String>, cid: Option<u64>) -> Result<(String, Option<u64>), String> {
    let query = query.map(|q| q.trim().to_owned()).filter(|q| !q.is_empty());
    match (query, cid) {
        (Some(_), Some(_)) => Err("give `query` or `cid`, not both".into()),
        (None, None) => Err("give `query` (a name or CAS number) or `cid`".into()),
        (Some(q), None) => {
            let digits = q.trim_start_matches("CID").trim_start_matches("cid").trim();
            Ok(match digits.parse::<u64>() {
                Ok(cid) if !q.contains('-') => (q.clone(), Some(cid)),
                _ => (q, None),
            })
        }
        (None, Some(cid)) => Ok((format!("CID {cid}"), Some(cid))),
    }
}

/// The GHS card of `query` (or compound `cid`) from PubChem through
/// `web`, dated `now_ms`. An unknown compound or one without a
/// classification is a card that says so.
///
/// # Errors
///
/// A message when PubChem cannot be reached (network, domain rules) or
/// answers something that is not its JSON.
pub async fn lookup(
    web: &dyn WebPort,
    query: &str,
    cid: Option<u64>,
    now_ms: u64,
) -> Result<GhsSpec, String> {
    let retrieved = utc_date(now_ms);
    let card = |cid: Option<u64>, name: Option<String>, fetched: String, message: String| GhsSpec {
        query: query.to_owned(),
        cid,
        name,
        classifications: Vec::new(),
        source_url: cid.map_or_else(|| PUBCHEM.to_owned(), page_url),
        fetched_url: fetched,
        retrieved: retrieved.clone(),
        message: Some(message),
    };
    let cid = match cid {
        Some(cid) => cid,
        None => {
            let url = cids_url(query);
            let not_found = format!("PubChem has no compound named `{query}`.");
            match web.fetch(&url).await {
                Ok(page) => match parse_cids(&page.text)?.first() {
                    Some(&cid) => cid,
                    None => return Ok(card(None, None, url, not_found)),
                },
                Err(WebError::Status { status: 404, .. }) => {
                    return Ok(card(None, None, url, not_found));
                }
                Err(e) => return Err(format!("PubChem lookup failed: {e}")),
            }
        }
    };
    let url = ghs_url(cid);
    let none = format!("PubChem has no GHS classification for CID {cid}.");
    let page = match web.fetch(&url).await {
        Ok(page) => page,
        Err(WebError::Status { status: 404, .. }) => return Ok(card(Some(cid), None, url, none)),
        Err(e) => return Err(format!("PubChem lookup failed: {e}")),
    };
    let record = parse_ghs(&page.text)?;
    let cid = record.cid.unwrap_or(cid);
    let mut spec = card(Some(cid), record.name, url, none);
    if !record.classifications.is_empty() {
        spec.message = None;
    }
    spec.source_url = page_url(cid);
    spec.classifications = record.classifications;
    Ok(spec)
}

fn output(spec: GhsSpec) -> ToolOutput {
    match spec.validate() {
        Ok(()) => {
            let summary = ghs_summary(&spec);
            ToolOutput::text(summary).with_block_update(BlockKind::Ghs(Box::new(spec)))
        }
        Err(e) => ToolOutput::error(e.to_string()),
    }
}

/// What the model gets back from `show_ghs`.
#[must_use]
pub fn ghs_summary(spec: &GhsSpec) -> String {
    let who = match (&spec.name, spec.cid) {
        (Some(n), Some(c)) => format!("{n} (PubChem CID {c})"),
        (None, Some(c)) => format!("PubChem CID {c}"),
        _ => format!("`{}`", spec.query),
    };
    if !spec.found() {
        return format!(
            "GHS card for {who} shown to the user: {} Source checked: {} (retrieved {}). Do not \
             state hazards for it from memory: say they could not be verified here.",
            spec.message.as_deref().unwrap_or("nothing found."),
            spec.source_url,
            spec.retrieved
        );
    }
    let mut out = format!(
        "GHS card for {who} shown to the user, from PubChem {} (retrieved {}), {} sources. \
         Quote only these, naming the source:",
        spec.source_url,
        spec.retrieved,
        spec.classifications.len()
    );
    for c in &spec.classifications {
        let _ = write!(out, "\n\nSource: {}", c.source);
        if let Some(s) = &c.signal {
            let _ = write!(out, "\nSignal word: {s}");
        }
        if !c.pictograms.is_empty() {
            let p: Vec<String> = c
                .pictograms
                .iter()
                .map(|p| match (&p.label, p.symbol_name()) {
                    (Some(l), _) => format!("{} {l}", p.code),
                    (None, Some(n)) => format!("{} {n}", p.code),
                    (None, None) => p.code.clone(),
                })
                .collect();
            let _ = write!(out, "\nPictograms: {}", p.join(", "));
        }
        for h in c.hazards.iter().take(LISTED_HAZARDS) {
            let _ = write!(out, "\n{}", h.text);
        }
        if c.hazards.len() > LISTED_HAZARDS {
            let _ = write!(
                out,
                "\n… {} more hazard statements",
                c.hazards.len() - LISTED_HAZARDS
            );
        }
        if !c.precautionary.is_empty() {
            let _ = write!(out, "\nPrecautionary codes: {}", c.precautionary.join(", "));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::json;

    use super::*;
    use crate::ports::web::WebPage;
    use crate::testing::web::ScriptedWeb;
    use crate::tools::registry::{Tool, Typed};

    const GHS: &str = include_str!("../../tests/fixtures/pubchem/acetone_ghs.json");
    const CIDS: &str = include_str!("../../tests/fixtures/pubchem/acetone_cids.json");

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn page(url: &str, text: &str) -> WebPage {
        WebPage {
            url: url.into(),
            title: None,
            text: text.into(),
        }
    }

    fn day() -> u64 {
        1_792_929_600_000
    }

    async fn call(web: ScriptedWeb, input: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let tool = Typed(ShowGhsTool::new(Arc::new(web), day));
        let prepared = tool.prepare(input, &ctx()).await?;
        prepared.run.await
    }

    #[test]
    fn definition_urls_and_parsing() {
        let tool = Typed(ShowGhsTool::new(Arc::new(ScriptedWeb::new()), day));
        insta::assert_json_snapshot!(
            "show_ghs_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
        assert_eq!(
            cids_url("sodium chloride"),
            "https://pubchem.ncbi.nlm.nih.gov/rest/pug/compound/name/sodium%20chloride/cids/JSON"
        );
        assert_eq!(parse_cids(CIDS).unwrap(), [180]);
        let record = parse_ghs(GHS).unwrap();
        assert_eq!(
            (record.cid, record.name.as_deref()),
            (Some(180), Some("Acetone"))
        );
        assert_eq!(record.classifications.len(), 1);
        let eu = &record.classifications[0];
        assert!(
            eu.source.starts_with("Regulation (EC) No 1272/2008"),
            "{}",
            eu.source
        );
        assert_eq!(eu.signal.as_deref(), Some("Danger"));
        let codes: Vec<&str> = eu.pictograms.iter().map(|p| p.code.as_str()).collect();
        assert_eq!(codes, ["GHS02", "GHS07"]);
        let hazards: Vec<&str> = eu.hazards.iter().map(|h| h.code.as_str()).collect();
        assert_eq!(hazards, ["H225", "H319", "H336"]);
        assert!(
            eu.hazards[0]
                .text
                .starts_with("H225: Highly Flammable liquid and vapor")
        );
        assert_eq!(eu.precautionary.first().map(String::as_str), Some("P210"));
        assert_eq!(eu.precautionary.last().map(String::as_str), Some("P501"));
        assert!(eu.precautionary.contains(&"P305+P351+P338".to_owned()));
        assert!(
            eu.license
                .as_deref()
                .is_some_and(|l| l.contains("Creative Commons"))
        );
        assert_eq!(
            hazard("H336 (98.7%): May cause drowsiness")
                .unwrap()
                .share
                .as_deref(),
            Some("98.7%")
        );
    }

    #[tokio::test]
    async fn a_name_is_resolved_and_the_card_shows_only_fetched_data() {
        let web = ScriptedWeb::new()
            .on_fetch(&cids_url("acetone"), page(&cids_url("acetone"), CIDS))
            .on_fetch(&ghs_url(180), page(&ghs_url(180), GHS));
        let out = call(web, json!({ "query": "acetone" })).await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        let text = out.text_content();
        assert!(
            text.starts_with(
                "GHS card for Acetone (PubChem CID 180) shown to the user, from PubChem \
             https://pubchem.ncbi.nlm.nih.gov/compound/180#section=GHS-Classification \
             (retrieved 2026-10-25), 1 sources."
            ),
            "{text}"
        );
        assert!(text.contains("Signal word: Danger\nPictograms: GHS02 Flammable, GHS07 Irritant"));
        assert!(text.contains("\nH319: Causes serious eye irritation"));
        let Some(BlockKind::Ghs(spec)) = out.block_update else {
            panic!("no card");
        };
        assert_eq!(spec.retrieved, "2026-10-25");
        assert_eq!(spec.fetched_url, ghs_url(180));
    }

    #[tokio::test]
    async fn unknown_compounds_and_missing_classifications_say_so() {
        // An unscripted fetch is a 404.
        let out = call(ScriptedWeb::new(), json!({ "query": "unobtainium" }))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(
            out.text_content()
                .contains("PubChem has no compound named `unobtainium`.")
        );
        assert!(out.text_content().contains("Do not state hazards"));
        let Some(BlockKind::Ghs(spec)) = out.block_update else {
            panic!("no card");
        };
        assert!(!spec.found());
        let none = call(ScriptedWeb::new(), json!({ "cid": 5 })).await.unwrap();
        assert!(
            none.text_content()
                .contains("no GHS classification for CID 5")
        );
        // A blocked domain is an error, not a card.
        let blocked = ScriptedWeb::new()
            .on_fetch_error(&ghs_url(180), WebError::DomainNotAllowed(ghs_url(180)));
        let out = call(blocked, json!({ "query": "CID 180" })).await.unwrap();
        assert!(out.is_error);
        assert!(out.block_update.is_none());
        assert!(call(ScriptedWeb::new(), json!({})).await.is_err());
    }
}
