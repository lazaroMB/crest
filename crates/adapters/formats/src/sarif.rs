//! SARIF 2.1.0 logs, through hand-written serde structs for the subset
//! the findings block shows (lighter than the generated `serde-sarif`
//! model and tolerant of missing fields).
//!
//! Every result of every run becomes a [`Finding`]: rule id (from
//! `ruleId`, `rule.id` or the rule at `ruleIndex`), level (the result's,
//! else the rule's default configuration, else `warning`, or `none` for a
//! result whose `kind` is not `fail`), the message (`text`, else the
//! rule's message string with `{n}` arguments filled in), the first
//! physical location (with `uriBaseId` resolved through
//! `originalUriBaseIds` and `file://` URIs turned into paths), the region
//! snippet, the rule's help URI and the result's `fixes` as
//! [`FindingFix`] edits (replacements of a region, resolved to paths like
//! the locations). Turning the edits into a diff needs the files, so the
//! tool does it.

use std::collections::BTreeMap;

use domain::block::SourceRef;
use domain::report::{Finding, FindingFix, FindingSeverity, FindingsSpec, FixEdit};
use serde::Deserialize;

#[derive(Deserialize)]
struct Log {
    #[serde(default)]
    runs: Option<Vec<Run>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Run {
    #[serde(default)]
    tool: Option<Tool>,
    #[serde(default)]
    results: Vec<SarifResult>,
    #[serde(default)]
    original_uri_base_ids: BTreeMap<String, ArtifactLocation>,
}

#[derive(Deserialize)]
struct Tool {
    driver: Component,
    #[serde(default)]
    extensions: Vec<Component>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Component {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    semantic_version: Option<String>,
    #[serde(default)]
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rule {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    help_uri: Option<String>,
    #[serde(default)]
    short_description: Option<Message>,
    #[serde(default)]
    default_configuration: Option<Configuration>,
    #[serde(default)]
    message_strings: BTreeMap<String, Message>,
}

#[derive(Deserialize)]
struct Configuration {
    #[serde(default)]
    level: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SarifResult {
    #[serde(default)]
    rule_id: Option<String>,
    #[serde(default)]
    rule_index: Option<usize>,
    #[serde(default)]
    rule: Option<RuleReference>,
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    message: Option<Message>,
    #[serde(default)]
    locations: Vec<Location>,
    #[serde(default)]
    fixes: Vec<Fix>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fix {
    #[serde(default)]
    description: Option<Message>,
    #[serde(default)]
    artifact_changes: Vec<ArtifactChange>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactChange {
    #[serde(default)]
    artifact_location: Option<ArtifactLocation>,
    #[serde(default)]
    replacements: Vec<Replacement>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Replacement {
    #[serde(default)]
    deleted_region: Option<Region>,
    #[serde(default)]
    inserted_content: Option<Snippet>,
}

#[derive(Deserialize)]
struct RuleReference {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    index: Option<usize>,
}

#[derive(Deserialize, Default)]
struct Message {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    markdown: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    arguments: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    #[serde(default)]
    physical_location: Option<PhysicalLocation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhysicalLocation {
    #[serde(default)]
    artifact_location: Option<ArtifactLocation>,
    #[serde(default)]
    region: Option<Region>,
    #[serde(default)]
    context_region: Option<Region>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ArtifactLocation {
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    uri_base_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Region {
    #[serde(default)]
    start_line: Option<u32>,
    #[serde(default)]
    end_line: Option<u32>,
    #[serde(default)]
    start_column: Option<u32>,
    #[serde(default)]
    end_column: Option<u32>,
    #[serde(default)]
    char_offset: Option<u64>,
    #[serde(default)]
    char_length: Option<u64>,
    #[serde(default)]
    byte_offset: Option<u64>,
    #[serde(default)]
    byte_length: Option<u64>,
    #[serde(default)]
    snippet: Option<Snippet>,
}

#[derive(Deserialize)]
struct Snippet {
    #[serde(default)]
    text: Option<String>,
}

/// Parses `bytes`; `path` names the file in errors.
///
/// # Errors
///
/// Malformed JSON or a log without `runs`.
pub fn parse(bytes: &[u8], path: &str) -> Result<FindingsSpec, String> {
    let log: Log =
        serde_json::from_slice(bytes).map_err(|e| format!("`{path}` is not SARIF JSON: {e}"))?;
    let runs = log
        .runs
        .ok_or_else(|| format!("`{path}` is not a SARIF log: it has no `runs`"))?;
    let mut tools: Vec<String> = Vec::new();
    let mut findings = Vec::new();
    for run in &runs {
        if let Some(tool) = &run.tool
            && let Some(name) = tool_name(&tool.driver)
            && !tools.contains(&name)
        {
            tools.push(name);
        }
        let rules: Vec<&Rule> = run
            .tool
            .iter()
            .flat_map(|tool| std::iter::once(&tool.driver).chain(&tool.extensions))
            .flat_map(|component| &component.rules)
            .collect();
        findings.extend(
            run.results
                .iter()
                .map(|result| finding(result, &rules, run)),
        );
    }
    Ok(FindingsSpec {
        tool: (!tools.is_empty()).then(|| tools.join(", ")),
        findings,
    })
}

fn tool_name(driver: &Component) -> Option<String> {
    let name = driver.name.as_deref()?.trim();
    let version = driver
        .semantic_version
        .as_deref()
        .or(driver.version.as_deref());
    Some(match version {
        Some(version) if !version.trim().is_empty() => format!("{name} {}", version.trim()),
        _ => name.to_owned(),
    })
}

fn finding(result: &SarifResult, rules: &[&Rule], run: &Run) -> Finding {
    let index = result
        .rule_index
        .or_else(|| result.rule.as_ref().and_then(|r| r.index));
    let id = result
        .rule_id
        .clone()
        .or_else(|| result.rule.as_ref().and_then(|r| r.id.clone()));
    let rule = index.and_then(|ix| rules.get(ix).copied()).or_else(|| {
        let id = id.as_deref()?;
        rules.iter().copied().find(|r| r.id.as_deref() == Some(id))
    });
    let id = id.or_else(|| rule.and_then(|r| r.id.clone()));
    let level = result.level.as_deref().or_else(|| {
        rule.and_then(|r| r.default_configuration.as_ref())
            .and_then(|c| c.level.as_deref())
    });
    let severity = match level {
        Some("error") => FindingSeverity::Error,
        Some("warning") => FindingSeverity::Warning,
        Some("note") => FindingSeverity::Note,
        Some(_) => FindingSeverity::None,
        // `kind` other than `fail` (pass, review, informational) means
        // no level.
        None if result.kind.as_deref().is_some_and(|k| k != "fail") => FindingSeverity::None,
        None => FindingSeverity::Warning,
    };
    let physical = result
        .locations
        .iter()
        .find_map(|l| l.physical_location.as_ref());
    let location = physical.and_then(|p| {
        let artifact = p.artifact_location.as_ref()?;
        let path = resolve(artifact, &run.original_uri_base_ids)?;
        let region = p.region.as_ref();
        let line_start = region.and_then(|r| r.start_line).filter(|&l| l > 0);
        let line_end = region
            .and_then(|r| r.end_line)
            .filter(|&e| line_start.is_some_and(|s| e > s));
        Some(SourceRef::File {
            path,
            line_start,
            line_end,
        })
    });
    let snippet = physical.and_then(|p| {
        [p.region.as_ref(), p.context_region.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|r| r.snippet.as_ref()?.text.clone())
            .filter(|t| !t.trim().is_empty())
    });
    Finding {
        rule: id,
        severity,
        message: message(result.message.as_ref(), rule),
        location,
        snippet,
        help_uri: rule.and_then(|r| r.help_uri.clone()),
        fix: None,
        fixes: result
            .fixes
            .iter()
            .filter_map(|f| fix(f, &run.original_uri_base_ids))
            .collect(),
    }
}

/// A SARIF fix as edits; `None` when it changes no file we can name.
fn fix(fix: &Fix, bases: &BTreeMap<String, ArtifactLocation>) -> Option<FindingFix> {
    let mut edits = Vec::new();
    for change in &fix.artifact_changes {
        let Some(path) = change
            .artifact_location
            .as_ref()
            .and_then(|a| resolve(a, bases))
        else {
            continue;
        };
        for replacement in &change.replacements {
            let Some(region) = &replacement.deleted_region else {
                continue;
            };
            edits.push(FixEdit {
                path: path.clone(),
                start_line: region.start_line,
                start_column: region.start_column,
                end_line: region.end_line,
                end_column: region.end_column,
                char_offset: region.char_offset,
                char_length: region.char_length,
                byte_offset: region.byte_offset,
                byte_length: region.byte_length,
                inserted: replacement
                    .inserted_content
                    .as_ref()
                    .and_then(|c| c.text.clone())
                    .unwrap_or_default(),
            });
        }
    }
    (!edits.is_empty()).then(|| FindingFix {
        description: fix
            .description
            .as_ref()
            .and_then(|m| m.text.clone().or_else(|| m.markdown.clone())),
        edits,
    })
}

/// The message text: its own, else the rule's string `id`, with `{n}`
/// replaced by argument `n`; the rule's short description as a last
/// resort.
fn message(message: Option<&Message>, rule: Option<&Rule>) -> String {
    let empty = Message::default();
    let message = message.unwrap_or(&empty);
    let template = message
        .text
        .clone()
        .or_else(|| message.markdown.clone())
        .or_else(|| {
            let id = message.id.as_deref()?;
            let string = rule?.message_strings.get(id)?;
            string.text.clone().or_else(|| string.markdown.clone())
        })
        .or_else(|| rule?.short_description.as_ref()?.text.clone())
        .unwrap_or_default();
    let mut text = template;
    for (ix, argument) in message.arguments.iter().enumerate() {
        text = text.replace(&format!("{{{ix}}}"), argument);
    }
    text
}

/// The path of an artifact: its URI joined to its base, with `file://`
/// and percent-escapes taken out.
fn resolve(
    artifact: &ArtifactLocation,
    bases: &BTreeMap<String, ArtifactLocation>,
) -> Option<String> {
    let uri = artifact.uri.as_deref()?.trim();
    if uri.is_empty() {
        return None;
    }
    let base = artifact
        .uri_base_id
        .as_deref()
        .and_then(|id| bases.get(id))
        .and_then(|b| b.uri.as_deref());
    let joined = match base {
        Some(base) if !uri.contains("://") && !uri.starts_with('/') => {
            format!("{}/{uri}", base.trim_end_matches('/'))
        }
        _ => uri.to_owned(),
    };
    Some(file_path(&joined))
}

/// `file:///a/b%20c` → `/a/b c`; other URIs only lose percent-escapes.
pub fn file_path(uri: &str) -> String {
    let path = uri
        .strip_prefix("file://")
        .map_or(uri, |rest| rest.strip_prefix("localhost").unwrap_or(rest));
    // `file:///C:/x` on Windows.
    let path = match path.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => &path[1..],
        _ => path,
    };
    percent_decode(path)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = text.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}
