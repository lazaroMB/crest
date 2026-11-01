//! `show_sequence`: a DNA, RNA or protein sequence in the sequence viewer
//! (ruler, features, frames, search) with a plasmid map for circular DNA.
//!
//! The model gets the record (name, length, alphabet, topology, GC
//! content), the features by kind with their locations, problems of the
//! CDSs (frame shifts, premature stops), the enzymes that cut once and
//! the hits of `find`, never the residues themselves.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::permission::ToolEffect;
use domain::science::sequence::{Alphabet, SequenceSpec, Strand, Topology};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::diagram::{one_of, source_text};
use super::{FormatToolPorts, invalid, non_blank, one_line, parse_off, shown};
use crate::ports::formats::SequenceRequest;
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, TypedTool};

/// Features listed to the model.
pub const MAX_FEATURES_LISTED: usize = 30;
/// Hits of `find` listed to the model.
pub const MAX_HITS_LISTED: usize = 20;
/// Single cutters listed to the model.
const MAX_CUTTERS_LISTED: usize = 30;

/// Input of `show_sequence`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowSequence {
    /// A sequence file of the workspace: GenBank (`.gb`, `.gbk`), FASTA
    /// (`.fa`, `.fasta`, `.faa`, `.fna`) or bare residues.
    #[serde(default)]
    pub path: Option<String>,
    /// Residues or a FASTA record given inline, instead of `path`.
    #[serde(default)]
    pub sequence: Option<String>,
    /// The record of a multi-record file to show, 1-based (default 1).
    #[serde(default)]
    pub record: Option<usize>,
    /// `dna`, `rna` or `protein`; guessed from the residues when absent
    /// (GenBank says).
    #[serde(default)]
    pub alphabet: Option<Alphabet>,
    /// Whether the sequence is circular (a plasmid); GenBank's LOCUS line
    /// says, otherwise linear.
    #[serde(default)]
    pub circular: Option<bool>,
    /// Search the bundled restriction enzymes (default true; DNA only).
    #[serde(default)]
    pub enzymes: Option<bool>,
    /// A motif to find (IUPAC codes allowed, both strands), e.g. a primer;
    /// it is highlighted and its positions are returned.
    #[serde(default)]
    pub find: Option<String>,
    /// Title shown above the viewer; defaults to the record name.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_sequence`: the sequence viewer.
#[derive(Clone)]
pub struct ShowSequenceTool {
    ports: FormatToolPorts,
}

impl ShowSequenceTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_sequence`.
pub const SHOW_SEQUENCE: &str = "show_sequence";

#[async_trait]
impl TypedTool for ShowSequenceTool {
    type Input = ShowSequence;

    fn name(&self) -> &str {
        SHOW_SEQUENCE
    }

    fn description(&self) -> &str {
        "Shows a DNA, RNA or protein sequence in the user's timeline: a \
         GenBank, FASTA or plain file (`path`) or residues inline \
         (`sequence`), with a ruler, feature tracks, translation frames, \
         search with IUPAC codes on both strands, and a plasmid map with \
         restriction sites for circular DNA. Returns the length, GC \
         content, features with their locations, CDS problems (frame \
         shifts, premature stops), single-cutter enzymes and the hits of \
         `find`. Cite these; do not count bases or positions yourself, \
         and do not paste the sequence back."
    }

    fn effect(&self, _input: &ShowSequence) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowSequence,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let input = normalize_sequence(input).map_err(|e| invalid(SHOW_SEQUENCE, e))?;
        let ports = self.ports.clone();
        let subject = input
            .path
            .clone()
            .unwrap_or_else(|| "inline sequence".to_owned());
        let run = async move {
            let find = input.find.clone();
            shown(
                build_sequence(&ports, input).await,
                |spec| sequence_summary(spec, find.as_deref()),
                |spec| BlockKind::Sequence(Box::new(spec)),
            )
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// Checks a `show_sequence` input: exactly one of `path` and `sequence`
/// (not too large), a 1-based `record`, and the motif and title when not
/// blank.
///
/// # Errors
///
/// A message for the caller: neither or both given, `sequence` too
/// large, or `record` 0.
pub fn normalize_sequence(input: ShowSequence) -> Result<ShowSequence, String> {
    let (path, sequence) = one_of(input.path, input.sequence, "sequence")?;
    if input.record == Some(0) {
        return Err("`record` counts from 1".to_owned());
    }
    Ok(ShowSequence {
        path,
        sequence,
        find: non_blank(input.find),
        title: non_blank(input.title),
        ..input
    })
}

/// The sequence block of a checked call ([`normalize_sequence`]): reads
/// the file through the guard (or takes the residues) and parses it off
/// the executor.
///
/// # Errors
///
/// A message for the caller: the file cannot be read or parsed, or the
/// result is not a valid block.
pub async fn build_sequence(
    ports: &FormatToolPorts,
    input: ShowSequence,
) -> Result<SequenceSpec, String> {
    let request = SequenceRequest {
        path: input.path.clone(),
        record: input.record.map_or(0, |r| r.saturating_sub(1)),
        alphabet: input.alphabet,
        topology: input.circular.map(|c| {
            if c {
                Topology::Circular
            } else {
                Topology::Linear
            }
        }),
        enzymes: input.enzymes.unwrap_or(true),
    };
    let text = source_text(ports, input.path.as_deref(), input.sequence).await?;
    let formats = Arc::clone(&ports.formats);
    let mut spec = parse_off(ports, "the sequence reader", move || {
        formats.parse_sequence(text.as_bytes(), &request)
    })
    .await?
    .map_err(|e| e.to_string())?;
    spec.title = input.title;
    spec.validate().map_err(|e| e.to_string())?;
    Ok(spec)
}

/// A percentage with one decimal.
fn percent(v: f64) -> String {
    format!("{v:.1}%")
}

/// A 1-based inclusive range of a 0-based half-open one.
fn range_text(start: usize, end: usize) -> String {
    format!("{}..{}", start + 1, end)
}

/// What the model gets back from `show_sequence`; `find` adds its hits.
#[must_use]
pub fn sequence_summary(spec: &SequenceSpec, find: Option<&str>) -> String {
    let unit = spec.alphabet.unit();
    let mut out = format!(
        "{} {} `{}` shown to the user: {} {unit}",
        if spec.is_circular() {
            "Circular"
        } else {
            "Linear"
        },
        spec.alphabet.name(),
        one_line(&spec.display_title(), 60),
        spec.len()
    );
    if let Some(gc) = spec.gc() {
        let _ = write!(out, ", GC {}", percent(gc));
    }
    out.push('.');
    if let Some(d) = &spec.description {
        let _ = write!(out, " {}.", one_line(d, 120));
    }
    if let Some(o) = &spec.organism {
        let _ = write!(out, " Organism: {}.", one_line(o, 60));
    }
    if let Some(a) = &spec.accession {
        let _ = write!(out, " Accession {a}.");
    }
    if spec.records > 1 {
        let _ = write!(out, " Record {} of {}.", spec.record + 1, spec.records);
    }
    if !spec.features.is_empty() {
        let counts: Vec<String> = spec
            .kind_counts()
            .iter()
            .map(|(k, n)| format!("{n} {k}"))
            .collect();
        let _ = write!(out, "\nFeatures: {}.", counts.join(", "));
        for f in spec.features.iter().take(MAX_FEATURES_LISTED) {
            let _ = write!(
                out,
                "\n- {} {}: {} ({} {unit})",
                f.kind,
                one_line(f.name(), 60),
                f.location_text(),
                f.len()
            );
        }
        if spec.features.len() > MAX_FEATURES_LISTED {
            let _ = write!(
                out,
                "\n… {} more features (the user sees them all)",
                spec.features.len() - MAX_FEATURES_LISTED
            );
        }
    }
    for issue in spec.cds_issues().iter().take(10) {
        let _ = write!(out, "\nCheck: {issue}");
    }
    if spec.alphabet.is_nucleic() && spec.enzyme_source.is_some() {
        let cutters = spec.unique_cutters();
        if cutters.is_empty() {
            let _ = write!(out, "\nNo enzyme of the bundled table cuts once.");
        } else {
            let listed: Vec<String> = cutters
                .iter()
                .take(MAX_CUTTERS_LISTED)
                .map(|e| {
                    let cut = spec
                        .cut_sites
                        .iter()
                        .find(|c| c.enzyme == *e)
                        .map_or(0, |c| c.cut);
                    format!("{e} (cuts after {cut})")
                })
                .collect();
            let _ = write!(out, "\nSingle cutters: {}.", listed.join(", "));
        }
        let enzymes: std::collections::BTreeSet<&str> =
            spec.cut_sites.iter().map(|c| c.enzyme.as_str()).collect();
        let _ = write!(
            out,
            " {} sites of {} enzymes in all (table: {}).",
            spec.cut_sites.len(),
            enzymes.len(),
            spec.enzyme_source.as_deref().unwrap_or("")
        );
    }
    if let Some(query) = find {
        match spec.search(query) {
            Ok(hits) if hits.is_empty() => {
                let _ = write!(out, "\n`{query}`: not found.");
            }
            Ok(hits) => {
                let listed: Vec<String> = hits
                    .iter()
                    .take(MAX_HITS_LISTED)
                    .map(|h| {
                        let strand = match h.strand {
                            Strand::Reverse => " (−)",
                            Strand::Forward => " (+)",
                            Strand::None => "",
                        };
                        format!("{}{strand}", range_text(h.start, h.end))
                    })
                    .collect();
                let _ = write!(
                    out,
                    "\n`{query}`: {} hits at {}{}.",
                    hits.len(),
                    listed.join(", "),
                    if hits.len() > MAX_HITS_LISTED {
                        ", …"
                    } else {
                        ""
                    }
                );
            }
            Err(err) => {
                let _ = write!(out, "\n`{query}` cannot be searched: {err}.");
            }
        }
    }
    for w in spec.warnings.iter().take(5) {
        let _ = write!(out, "\nNote: {}", one_line(w, 160));
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
    use crate::tools::registry::{Tool, ToolOutput, Typed};
    use crate::usecases::compute::inline_offload;

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn ports() -> FormatToolPorts {
        let fs = InMemoryFs::new(test_root()).with_file("dna/p.gb", "LOCUS x\n");
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
        let tool = Typed(ShowSequenceTool::new(ports()));
        insta::assert_json_snapshot!(
            "show_sequence_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn the_model_gets_features_cutters_and_hits() {
        let tool = Typed(ShowSequenceTool::new(ports()));
        let out = call(&tool, json!({ "path": "dna/p.gb", "find": "GAATTC" }))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "Circular DNA `pFake` shown to the user: 30 bp, GC 33.3%. a fake plasmid.\n\
             Features: 1 CDS.\n\
             - CDS orf: 1..9 (9 bp)\n\
             Single cutters: EcoRI (cuts after 10). 1 sites of 1 enzymes in all (table: REBASE).\n\
             `GAATTC`: 1 hits at 10..15 (+)."
        );
        assert!(matches!(out.block_update, Some(BlockKind::Sequence(_))));
        let inline = call(&tool, json!({ "sequence": "ACGT", "circular": false }))
            .await
            .unwrap();
        assert!(inline.text_content().starts_with("Linear DNA"));
        assert!(call(&tool, json!({})).await.is_err());
        assert!(
            call(&tool, json!({ "sequence": "A", "record": 0 }))
                .await
                .is_err()
        );
        let missing = call(&tool, json!({ "path": "dna/none.gb" })).await.unwrap();
        assert!(missing.is_error);
    }
}
