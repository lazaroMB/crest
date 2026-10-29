//! The files of a Spec folder, rendered from the research state and the
//! model's answer: `spec.md`, `journal.md`, `sources.json` and the CSV of
//! the charts.

use domain::block::{ChartSpec, SourceRef};
use domain::research::{
    Constraint, ConstraintKind, Evidence, EvidenceId, JournalEntry, ResearchState, Stage,
    extract_citations,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Headings of the ten sections of `spec.md`, in order.
pub const SECTIONS: [&str; 10] = [
    "1. Problem",
    "2. Background",
    "3. Hypothesis",
    "4. Proposed solution",
    "5. Functional specification",
    "6. Technical specification",
    "7. Constraints",
    "8. Test protocol",
    "9. Out of scope and risks",
    "10. Sources",
];

/// The Spanish headings of [`SECTIONS`], written by earlier versions;
/// accepted when an existing `spec.md` is read.
pub const SECTIONS_ES: [&str; 10] = [
    "1. Problema",
    "2. Antecedentes",
    "3. Hipótesis",
    "4. Propuesta de solución",
    "5. Especificación funcional",
    "6. Especificación técnica",
    "7. Restricciones",
    "8. Protocolo de pruebas",
    "9. Fuera de alcance y riesgos",
    "10. Fuentes",
];

/// What the model writes for one Spec; the constraints and the sources
/// come from the research state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpecAnswer {
    /// Short title of the Spec (it names the folder), e.g. "Session cache".
    pub title: String,
    /// The problem in one measurable sentence.
    pub problem: String,
    /// Summary of the research, previous attempts and what was discarded, citing evidence as [E1].
    pub background: String,
    /// The one-sentence hypothesis the solution rests on.
    pub hypothesis: String,
    /// Short description of the chosen solution.
    pub proposal: String,
    /// Black-box diagram in Mermaid (`flowchart`) with the parts it interacts with.
    pub black_box: String,
    /// States, flows, screens, commands and API contracts (markdown).
    pub functional: String,
    /// Sequence diagram in Mermaid (`sequenceDiagram`).
    pub sequence: String,
    /// State diagram in Mermaid (`stateDiagram-v2`).
    pub states: String,
    /// Modules and files to touch, types and signatures, algorithms, migrations and configuration (markdown).
    pub technical: String,
    /// Tests and experiments that show the problem and must pass at the end (markdown).
    pub test_protocol: String,
    /// What is out of scope (markdown).
    pub out_of_scope: String,
    /// Risks and how to mitigate them (markdown).
    pub risks: String,
}

impl SpecAnswer {
    /// Strips code fences around the diagrams.
    #[must_use]
    pub fn normalized(mut self) -> Self {
        for diagram in [&mut self.black_box, &mut self.sequence, &mut self.states] {
            *diagram = strip_fence(diagram);
        }
        self
    }

    /// Problems that make the answer unusable, before rendering.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut errors = Vec::new();
        let required = [
            ("title", &self.title),
            ("problem", &self.problem),
            ("background", &self.background),
            ("hypothesis", &self.hypothesis),
            ("proposal", &self.proposal),
            ("functional", &self.functional),
            ("technical", &self.technical),
            ("test_protocol", &self.test_protocol),
        ];
        for (field, value) in required {
            if value.trim().is_empty() {
                errors.push(format!("the field {field} is empty"));
            }
        }
        let diagrams = [
            ("black_box", &self.black_box, &["flowchart", "graph"][..]),
            ("sequence", &self.sequence, &["sequenceDiagram"][..]),
            ("states", &self.states, &["stateDiagram"][..]),
        ];
        for (field, source, starts) in diagrams {
            let first = source.trim_start();
            if !starts.iter().any(|s| first.starts_with(s)) {
                errors.push(format!(
                    "the field {field} must be Mermaid starting with {}",
                    starts.join(" or ")
                ));
            }
        }
        errors
    }
}

/// Removes a surrounding ```` ``` ```` fence (with or without language).
fn strip_fence(source: &str) -> String {
    let trimmed = source.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed.to_owned();
    };
    let body = rest.split_once('\n').map_or("", |(_, b)| b);
    body.trim_end()
        .strip_suffix("```")
        .unwrap_or(body)
        .trim()
        .to_owned()
}

/// A code fence longer than any backtick run in `body`.
fn fence_for(body: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in body.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    "`".repeat(longest.max(2) + 1)
}

fn mermaid(source: &str) -> String {
    let fence = fence_for(source);
    format!("{fence}mermaid\n{}\n{fence}\n", source.trim_end())
}

/// Text of one table cell: one line, `|` escaped.
fn cell(text: &str) -> String {
    text.replace(['\n', '\r'], " ").replace('|', "\\|")
}

const fn kind_name(kind: ConstraintKind) -> &'static str {
    match kind {
        ConstraintKind::Platform => "Platform",
        ConstraintKind::Performance => "Performance",
        ConstraintKind::Compatibility => "Compatibility",
        ConstraintKind::Security => "Security",
        ConstraintKind::Other => "Other",
    }
}

fn constraints_table(constraints: &[Constraint]) -> String {
    if constraints.is_empty() {
        return "No constraints recorded.\n".into();
    }
    let mut out = String::from("| Id | Kind | Constraint | Test |\n| --- | --- | --- | --- |\n");
    for c in constraints {
        let test = c
            .test
            .as_ref()
            .map_or_else(|| "no test".to_owned(), |t| format!("`{}`", t.reference));
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            c.id,
            kind_name(c.kind),
            cell(&c.statement),
            cell(&test)
        ));
    }
    out
}

/// One line describing where `source` points.
#[must_use]
pub fn describe_source(source: &SourceRef) -> String {
    match source {
        SourceRef::File {
            path,
            line_start,
            line_end,
        } => match (line_start, line_end) {
            (Some(s), Some(e)) if s != e => format!("`{path}:{s}-{e}`"),
            (Some(s), _) => format!("`{path}:{s}`"),
            _ => format!("`{path}`"),
        },
        SourceRef::Commit { sha, summary } => format!("commit `{sha}`: {summary}"),
        SourceRef::Url { url, title } => match title {
            Some(t) => format!("[{t}]({url})"),
            None => format!("<{url}>"),
        },
        SourceRef::Command { command, .. } => format!("command `{command}`"),
        other => match other.link() {
            Some(url) => format!("[{}]({url})", other.reference()),
            None => other.reference(),
        },
    }
}

/// `spec.md` of `answer`, titled with the folder `number`.
#[must_use]
pub fn spec_markdown(number: &str, answer: &SpecAnswer, state: &ResearchState) -> String {
    let body = sections_without_sources(answer, &state.constraints);
    let cited = cited_evidence(&body, state);
    let mut sources = String::new();
    if cited.is_empty() {
        sources.push_str("No sources cited.\n");
    }
    for e in &cited {
        sources.push_str(&format!(
            "- **[{}]** {}: {}\n",
            e.id,
            one_line(&e.claim),
            describe_source(&e.source)
        ));
    }
    format!(
        "# Spec {number}: {}\n\n{body}## {}\n\n{sources}",
        one_line(&answer.title),
        SECTIONS[9]
    )
}

fn sections_without_sources(a: &SpecAnswer, constraints: &[Constraint]) -> String {
    let block = |text: &str| format!("{}\n\n", text.trim());
    let mut out = String::new();
    out.push_str(&format!("## {}\n\n{}", SECTIONS[0], block(&a.problem)));
    out.push_str(&format!("## {}\n\n{}", SECTIONS[1], block(&a.background)));
    out.push_str(&format!("## {}\n\n{}", SECTIONS[2], block(&a.hypothesis)));
    out.push_str(&format!(
        "## {}\n\n{}{}\n",
        SECTIONS[3],
        block(&a.proposal),
        mermaid(&a.black_box)
    ));
    out.push_str(&format!(
        "## {}\n\n{}### Sequence\n\n{}\n### States\n\n{}\n",
        SECTIONS[4],
        block(&a.functional),
        mermaid(&a.sequence),
        mermaid(&a.states)
    ));
    out.push_str(&format!("## {}\n\n{}", SECTIONS[5], block(&a.technical)));
    out.push_str(&format!(
        "## {}\n\n{}\n",
        SECTIONS[6],
        constraints_table(constraints)
    ));
    out.push_str(&format!(
        "## {}\n\n{}",
        SECTIONS[7],
        block(&a.test_protocol)
    ));
    let or_none = |t: &str| {
        if t.trim().is_empty() {
            "None.".to_owned()
        } else {
            t.trim().to_owned()
        }
    };
    out.push_str(&format!(
        "## {}\n\n### Out of scope\n\n{}\n\n### Risks\n\n{}\n\n",
        SECTIONS[8],
        or_none(&a.out_of_scope),
        or_none(&a.risks)
    ));
    out
}

/// The evidence cited in `markdown` that exists, in id order.
#[must_use]
pub fn cited_evidence<'a>(markdown: &str, state: &'a ResearchState) -> Vec<&'a Evidence> {
    let mut ids = extract_citations(markdown);
    ids.sort();
    ids.iter().filter_map(|id| state.evidence(*id)).collect()
}

/// Problems with the citations of a rendered `spec.md`.
#[must_use]
pub fn citation_problems(
    markdown: &str,
    answer: &SpecAnswer,
    state: &ResearchState,
) -> Vec<String> {
    let mut errors = Vec::new();
    let ids = extract_citations(markdown);
    if let Err(unknown) = state.cites_only_known_evidence(&ids) {
        let known: Vec<String> = state.evidence.iter().map(|e| e.id.to_string()).collect();
        errors.push(format!(
            "cites evidence that does not exist: {}; use only {}",
            join(&unknown),
            known.join(", ")
        ));
    }
    if !state.evidence.is_empty() && extract_citations(&answer.background).is_empty() {
        errors.push("the background cites no evidence (write ids such as [E1])".into());
    }
    errors
}

fn join(ids: &[EvidenceId]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `sources.json`: the cited evidence with its source.
///
/// # Errors
///
/// When the evidence does not serialize.
pub fn sources_json(cited: &[&Evidence]) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(cited).map(|s| s + "\n")
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Folder slug of `title`: lowercase ASCII words joined by `-`, at most
/// 48 characters, `spec` when nothing is left.
#[must_use]
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out = out.trim_end_matches('-').to_owned();
    if out.len() > 48 {
        let cut = out[..48].rfind('-').unwrap_or(48);
        out.truncate(cut);
    }
    if out.is_empty() { "spec".into() } else { out }
}

/// Number of a folder named `NNNN-slug`.
#[must_use]
pub fn folder_number(folder: &str) -> Option<u32> {
    let (digits, _) = folder.split_once('-')?;
    (digits.len() == 4 && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// `journal.md`: the entries in order, under a heading per stage run.
#[must_use]
pub fn journal_markdown(journal: &[JournalEntry]) -> String {
    let mut out = String::from("# Research journal\n");
    let mut current: Option<Stage> = None;
    for entry in journal {
        if current != Some(entry.stage) {
            current = Some(entry.stage);
            out.push_str(&format!(
                "\n## {}. {}\n\n",
                entry.stage.index(),
                entry.stage.name()
            ));
        }
        out.push_str(&format!(
            "- `{}` {}\n",
            utc(entry.at),
            entry.text.trim().replace('\n', "\n  ")
        ));
    }
    if journal.is_empty() {
        out.push_str("\nNo entries.\n");
    }
    out
}

/// `YYYY-MM-DD HH:MM` (UTC) of Unix epoch milliseconds.
pub(crate) fn utc(ms: u64) -> String {
    let secs = ms / 1000;
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

/// Path in the Spec folder of the CSV of the `index`-th chart (0-based).
#[must_use]
pub fn chart_file(index: usize, chart: &ChartSpec) -> String {
    format!("evidence/chart-{}-{}.csv", index + 1, slug(&chart.title))
}

/// The "Charts" part of the sources of `spec.md`: one link per chart to
/// its CSV in `evidence/`. Empty without charts.
#[must_use]
pub fn charts_section(charts: &[ChartSpec]) -> String {
    if charts.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n### Charts\n\n");
    for (i, chart) in charts.iter().enumerate() {
        let file = chart_file(i, chart);
        out.push_str(&format!(
            "- **{}**: data in [`{file}`]({file})\n",
            one_line(&chart.title)
        ));
    }
    out
}

/// Data of `chart` as CSV: a label column and one column per series.
#[must_use]
pub fn chart_csv(chart: &ChartSpec) -> String {
    let mut out = std::iter::once(csv_field("label"))
        .chain(chart.series.iter().map(|s| csv_field(&s.name)))
        .collect::<Vec<_>>()
        .join(",");
    out.push('\n');
    for (i, label) in chart.labels.iter().enumerate() {
        let row = std::iter::once(csv_field(label))
            .chain(
                chart
                    .series
                    .iter()
                    .map(|s| s.values.get(i).map(ToString::to_string).unwrap_or_default()),
            )
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&row);
        out.push('\n');
    }
    out
}

fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::block::{ChartKind, Series};

    #[test]
    fn slugs_fold_accents_and_punctuation() {
        assert_eq!(slug("Session cache"), "session-cache");
        assert_eq!(slug("Caché de sesiones"), "cache-de-sesiones");
        assert_eq!(slug("  ¿Por qué? ¡Ñandú!  "), "por-que-nandu");
        assert_eq!(slug("!!!"), "spec");
        assert!(slug(&"word ".repeat(20)).len() <= 48);
    }

    #[test]
    fn folder_numbers_need_four_digits() {
        assert_eq!(folder_number("0007-cache"), Some(7));
        assert_eq!(folder_number("7-cache"), None);
        assert_eq!(folder_number("notes"), None);
    }

    #[test]
    fn diagrams_lose_their_fences() {
        assert_eq!(
            strip_fence("```mermaid\ngraph TD\n a-->b\n```"),
            "graph TD\n a-->b"
        );
        assert_eq!(strip_fence(" graph TD "), "graph TD");
    }

    #[test]
    fn csv_quotes_what_needs_it() {
        let chart = ChartSpec {
            title: "t".into(),
            kind: ChartKind::Bar,
            labels: vec!["a,b".into(), "c".into()],
            series: vec![Series {
                name: "p95 \"ms\"".into(),
                values: vec![1.5, 20.0],
            }],
        };
        assert_eq!(
            chart_csv(&chart),
            "label,\"p95 \"\"ms\"\"\"\n\"a,b\",1.5\nc,20\n"
        );
    }

    #[test]
    fn utc_dates_are_civil() {
        assert_eq!(utc(0), "1970-01-01 00:00");
        assert_eq!(utc(1_792_238_400_000), "2026-10-17 12:00");
    }
}
