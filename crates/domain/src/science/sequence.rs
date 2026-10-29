//! A DNA, RNA or protein sequence with its annotation (`show_sequence`):
//! GenBank features, restriction sites and the helpers the viewer and the
//! tool share (GC content, reverse complement, translation, search with
//! IUPAC ambiguity codes on both strands, CDS checks).
//!
//! Positions are 0-based and ranges half-open (`start..end`), as in the
//! code; the viewer and the summaries print them 1-based like GenBank. A
//! feature that crosses the origin of a circular sequence has two
//! segments (`x..len` and `0..y`).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::block::BlockError;

/// Longest sequence shown, in residues.
pub const MAX_LENGTH: usize = 20_000_000;
/// Most features kept per record.
pub const MAX_FEATURES: usize = 5_000;
/// Most qualifiers kept per feature.
pub const MAX_QUALIFIERS: usize = 24;
/// Longest qualifier value kept, in bytes (a `/translation` is cut).
pub const MAX_QUALIFIER_BYTES: usize = 4_096;
/// Most search hits returned.
pub const MAX_HITS: usize = 10_000;

/// What the residues are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Alphabet {
    /// Nucleotides with T.
    #[default]
    Dna,
    /// Nucleotides with U.
    Rna,
    /// Amino acids.
    Protein,
}

impl Alphabet {
    /// Whether the residues are nucleotides.
    #[must_use]
    pub fn is_nucleic(self) -> bool {
        !matches!(self, Self::Protein)
    }

    /// The unit of a length: `bp`, `nt` or `aa`.
    #[must_use]
    pub fn unit(self) -> &'static str {
        match self {
            Self::Dna => "bp",
            Self::Rna => "nt",
            Self::Protein => "aa",
        }
    }

    /// Lower-case name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Dna => "DNA",
            Self::Rna => "RNA",
            Self::Protein => "protein",
        }
    }
}

/// The file the sequence came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SequenceFormat {
    /// A GenBank flat file.
    Genbank,
    /// FASTA.
    Fasta,
    /// Bare residues.
    #[default]
    Plain,
}

/// Linear or circular (a plasmid).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Topology {
    /// Has two ends.
    #[default]
    Linear,
    /// Its end joins its start.
    Circular,
}

/// The strand a feature or hit is on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Strand {
    /// The given (top) strand.
    #[default]
    Forward,
    /// The complementary strand.
    Reverse,
    /// No strand (proteins, unstranded features).
    None,
}

/// A half-open range of residues, `start..end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct SeqSpan {
    /// First residue, 0-based.
    pub start: usize,
    /// One past the last residue.
    pub end: usize,
}

impl SeqSpan {
    /// Its length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Whether it is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// A feature qualifier, `/key="value"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Qualifier {
    /// Its key (`gene`, `product`, `note`).
    pub key: String,
    /// Its value; empty for a flag such as `/pseudo`.
    #[serde(default)]
    pub value: String,
}

/// An annotated region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SeqFeature {
    /// Feature key (`CDS`, `gene`, `promoter`, `rep_origin`).
    pub kind: String,
    /// Its name: `/label`, `/gene`, `/product` or `/note`, the first given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Its parts in reading order (two for one crossing the origin).
    pub segments: Vec<SeqSpan>,
    /// The strand.
    #[serde(default)]
    pub strand: Strand,
    /// Its qualifiers (capped at [`MAX_QUALIFIERS`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub qualifiers: Vec<Qualifier>,
}

impl SeqFeature {
    /// The label, else the kind.
    #[must_use]
    pub fn name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.kind)
    }

    /// Residues covered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.segments.iter().map(SeqSpan::len).sum()
    }

    /// Whether it covers nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Where it starts (its first segment's start) and ends (its last
    /// segment's end); the end is below the start across the origin.
    #[must_use]
    pub fn bounds(&self) -> Option<(usize, usize)> {
        Some((self.segments.first()?.start, self.segments.last()?.end))
    }

    /// The value of qualifier `key`, if present.
    #[must_use]
    pub fn qualifier(&self, key: &str) -> Option<&str> {
        self.qualifiers
            .iter()
            .find(|q| q.key == key)
            .map(|q| q.value.as_str())
    }

    /// Whether it covers residue `at`.
    #[must_use]
    pub fn covers(&self, at: usize) -> bool {
        self.segments.iter().any(|s| s.start <= at && at < s.end)
    }

    /// Its location as GenBank writes it (1-based, inclusive):
    /// `complement(join(10..20,30..40))`.
    #[must_use]
    pub fn location_text(&self) -> String {
        let parts: Vec<String> = self
            .segments
            .iter()
            .map(|s| {
                if s.len() == 1 {
                    (s.start + 1).to_string()
                } else {
                    format!("{}..{}", s.start + 1, s.end)
                }
            })
            .collect();
        let inner = if parts.len() == 1 {
            parts.join("")
        } else {
            format!("join({})", parts.join(","))
        };
        if self.strand == Strand::Reverse {
            format!("complement({inner})")
        } else {
            inner
        }
    }
}

/// Where a restriction enzyme cuts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CutSite {
    /// The enzyme (`EcoRI`).
    pub enzyme: String,
    /// Its recognition sequence on the top strand (`GAATTC`).
    pub site: String,
    /// First residue of the recognition site, 0-based.
    pub start: usize,
    /// The top-strand cut, before residue `cut` (0-based, modulo the
    /// length on a circular sequence).
    pub cut: usize,
    /// The strand the site reads on (non-palindromic sites).
    #[serde(default)]
    pub strand: Strand,
}

/// A DNA, RNA or protein sequence with its features and cut sites.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SequenceSpec {
    /// Title shown above the viewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The workspace file read, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// What it was read from.
    #[serde(default)]
    pub format: SequenceFormat,
    /// Record name (GenBank LOCUS, FASTA id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Description (GenBank DEFINITION, the rest of the FASTA header).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Accession and version, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accession: Option<String>,
    /// Organism, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organism: Option<String>,
    /// The residues.
    pub alphabet: Alphabet,
    /// Linear or circular.
    #[serde(default)]
    pub topology: Topology,
    /// The residues, upper case.
    pub sequence: String,
    /// The annotation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<SeqFeature>,
    /// Restriction sites of the bundled enzyme table (nucleic acids).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cut_sites: Vec<CutSite>,
    /// Where the enzyme table comes from, when sites were searched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enzyme_source: Option<String>,
    /// Records in the file.
    #[serde(default = "one")]
    pub records: usize,
    /// The record shown, 0-based.
    #[serde(default)]
    pub record: usize,
    /// Reader notes (non-IUPAC characters, features dropped).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

fn one() -> usize {
    1
}

fn invalid(reason: impl Into<String>) -> BlockError {
    BlockError::Invalid {
        kind: "sequence",
        reason: reason.into(),
    }
}

/// The IUPAC set of a nucleotide code as a mask (A=1, C=2, G=4, T/U=8),
/// 0 for anything else.
#[must_use]
pub fn iupac_mask(code: u8) -> u8 {
    match code.to_ascii_uppercase() {
        b'A' => 1,
        b'C' => 2,
        b'G' => 4,
        b'T' | b'U' => 8,
        b'R' => 1 | 4,
        b'Y' => 2 | 8,
        b'S' => 2 | 4,
        b'W' => 1 | 8,
        b'K' => 4 | 8,
        b'M' => 1 | 2,
        b'B' => 2 | 4 | 8,
        b'D' => 1 | 4 | 8,
        b'H' => 1 | 2 | 8,
        b'V' => 1 | 2 | 4,
        b'N' => 15,
        _ => 0,
    }
}

/// The complement of a nucleotide code (IUPAC ambiguity codes included,
/// `rna` for U instead of T); other characters stay.
#[must_use]
pub fn complement(code: u8, rna: bool) -> u8 {
    let upper = code.to_ascii_uppercase();
    let c = match upper {
        b'A' if rna => b'U',
        b'A' => b'T',
        b'T' | b'U' => b'A',
        b'C' => b'G',
        b'G' => b'C',
        b'R' => b'Y',
        b'Y' => b'R',
        b'K' => b'M',
        b'M' => b'K',
        b'B' => b'V',
        b'V' => b'B',
        b'D' => b'H',
        b'H' => b'D',
        other => other,
    };
    if code.is_ascii_lowercase() {
        c.to_ascii_lowercase()
    } else {
        c
    }
}

/// The reverse complement of `seq`.
#[must_use]
pub fn reverse_complement(seq: &str, rna: bool) -> String {
    seq.bytes()
        .rev()
        .map(|b| char::from(complement(b, rna)))
        .collect()
}

/// Whether `c` is a valid residue of `alphabet` (IUPAC codes, `-` gaps
/// and, for proteins, `*` stops).
#[must_use]
pub fn is_residue(c: u8, alphabet: Alphabet) -> bool {
    match alphabet {
        Alphabet::Dna | Alphabet::Rna => iupac_mask(c) != 0 || c == b'-',
        Alphabet::Protein => c.is_ascii_alphabetic() || c == b'*' || c == b'-',
    }
}

/// The alphabet `residues` most likely are: nucleic acid when at least
/// 90% are A, C, G, T, U or N (RNA when it has U and no T), else protein.
#[must_use]
pub fn guess_alphabet(residues: &str) -> Alphabet {
    let letters: Vec<u8> = residues
        .bytes()
        .filter(u8::is_ascii_alphabetic)
        .map(|b| b.to_ascii_uppercase())
        .collect();
    if letters.is_empty() {
        return Alphabet::Dna;
    }
    let nucleic = letters
        .iter()
        .filter(|b| matches!(b, b'A' | b'C' | b'G' | b'T' | b'U' | b'N'))
        .count();
    if nucleic * 10 < letters.len() * 9 {
        return Alphabet::Protein;
    }
    let has_u = letters.contains(&b'U');
    let has_t = letters.contains(&b'T');
    if has_u && !has_t {
        Alphabet::Rna
    } else {
        Alphabet::Dna
    }
}

/// The standard genetic code (NCBI table 1), codons in TCAG order.
const STANDARD_CODE: &[u8; 64] =
    b"FFLLSSSSYY**CC*WLLLLPPPPHHQQRRRRIIIMTTTTNNKKSSRRVVVVAAAADDEEGGGG";

fn base_index(b: u8) -> Option<usize> {
    match b.to_ascii_uppercase() {
        b'T' | b'U' => Some(0),
        b'C' => Some(1),
        b'A' => Some(2),
        b'G' => Some(3),
        _ => None,
    }
}

/// The amino acid of one codon (standard code); `X` when a base is
/// ambiguous or not a nucleotide.
#[must_use]
pub fn codon_amino(codon: &[u8]) -> u8 {
    match codon {
        [a, b, c] => match (base_index(*a), base_index(*b), base_index(*c)) {
            (Some(a), Some(b), Some(c)) => STANDARD_CODE[a * 16 + b * 4 + c],
            _ => b'X',
        },
        _ => b'X',
    }
}

/// `seq` translated codon by codon with the standard code; a trailing
/// partial codon is left out.
#[must_use]
pub fn translate(seq: &[u8]) -> String {
    seq.as_chunks::<3>()
        .0
        .iter()
        .map(|c| char::from(codon_amino(c)))
        .collect()
}

/// Share of G and C among the A, C, G and T/U of `seq` (S counts as G or
/// C, W as A or T), in percent; `None` without any of them.
#[must_use]
pub fn gc_percent(seq: &[u8]) -> Option<f64> {
    let (mut gc, mut all) = (0usize, 0usize);
    for &b in seq {
        match b.to_ascii_uppercase() {
            b'G' | b'C' | b'S' => {
                gc += 1;
                all += 1;
            }
            b'A' | b'T' | b'U' | b'W' => all += 1,
            _ => {}
        }
    }
    if all == 0 {
        return None;
    }
    Some(gc as f64 * 100.0 / all as f64)
}

/// A search match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct SeqHit {
    /// First residue, 0-based.
    pub start: usize,
    /// One past the last residue; below `start` when the hit crosses the
    /// origin of a circular sequence.
    pub end: usize,
    /// Forward for the query as given, reverse for its reverse complement.
    pub strand: Strand,
}

/// Why a query cannot be searched.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SearchError {
    /// Nothing to search for.
    #[error("the query is empty")]
    Empty,
    /// A character that is not a residue code.
    #[error("`{0}` is not an IUPAC code")]
    NotIupac(char),
}

fn matches_at(seq: &[u8], at: usize, query: &[u8], nucleic: bool) -> bool {
    query.iter().enumerate().all(|(i, &q)| {
        let s = seq[at + i];
        if nucleic {
            iupac_mask(q) & iupac_mask(s) != 0
        } else {
            let q = q.to_ascii_uppercase();
            q == b'X' || q == s.to_ascii_uppercase()
        }
    })
}

impl SequenceSpec {
    /// Checks the length and that every feature and site lies inside the
    /// sequence.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] describing the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        let len = self.sequence.len();
        if len == 0 {
            return Err(invalid("the sequence is empty"));
        }
        if len > MAX_LENGTH {
            return Err(invalid(format!(
                "the sequence has {len} residues, more than {MAX_LENGTH}"
            )));
        }
        if !self.sequence.is_ascii() {
            return Err(invalid("the sequence has non-ASCII characters"));
        }
        for f in &self.features {
            if f.segments.is_empty() {
                return Err(invalid(format!("feature `{}` has no location", f.name())));
            }
            if let Some(s) = f.segments.iter().find(|s| s.is_empty() || s.end > len) {
                return Err(invalid(format!(
                    "feature `{}` at {}..{} is outside the {len} residues",
                    f.name(),
                    s.start + 1,
                    s.end
                )));
            }
        }
        if let Some(c) = self
            .cut_sites
            .iter()
            .find(|c| c.start >= len || c.cut > len)
        {
            return Err(invalid(format!(
                "{} site at {} is outside the sequence",
                c.enzyme,
                c.start + 1
            )));
        }
        if self.record >= self.records.max(1) {
            return Err(invalid("the record index is past the records"));
        }
        Ok(())
    }

    /// Number of residues.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sequence.len()
    }

    /// Whether there are no residues.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sequence.is_empty()
    }

    /// Whether the sequence is circular.
    #[must_use]
    pub fn is_circular(&self) -> bool {
        self.topology == Topology::Circular
    }

    /// The title, else the name, else the file name, else `sequence`.
    #[must_use]
    pub fn display_title(&self) -> String {
        if let Some(t) = &self.title {
            return t.clone();
        }
        if let Some(n) = &self.name {
            return n.clone();
        }
        self.path
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .unwrap_or("sequence")
            .to_owned()
    }

    /// GC content of the whole sequence, in percent (nucleic acids).
    #[must_use]
    pub fn gc(&self) -> Option<f64> {
        if self.alphabet.is_nucleic() {
            gc_percent(self.sequence.as_bytes())
        } else {
            None
        }
    }

    /// Residues `start..end`; across the origin of a circular sequence
    /// when `end <= start` (the whole sequence for `end == start`).
    #[must_use]
    pub fn subsequence(&self, start: usize, end: usize) -> String {
        let len = self.len();
        let (start, end) = (start.min(len), end.min(len));
        if start < end {
            return self.sequence[start..end].to_owned();
        }
        if self.is_circular() {
            format!("{}{}", &self.sequence[start..], &self.sequence[..end])
        } else {
            String::new()
        }
    }

    /// The residues of `feature` in its reading direction (the reverse
    /// complement on the reverse strand).
    #[must_use]
    pub fn feature_sequence(&self, feature: &SeqFeature) -> String {
        let len = self.len();
        let mut out = String::new();
        for s in &feature.segments {
            if s.end <= len && s.start < s.end {
                out.push_str(&self.sequence[s.start..s.end]);
            }
        }
        if feature.strand == Strand::Reverse && self.alphabet.is_nucleic() {
            reverse_complement(&out, self.alphabet == Alphabet::Rna)
        } else {
            out
        }
    }

    /// The translation of reading frame `frame` (1, 2, 3 on the top
    /// strand; -1, -2, -3 on the reverse complement, frame -1 starting at
    /// the last residue), one amino acid per codon. Empty for proteins.
    #[must_use]
    pub fn frame_translation(&self, frame: i8) -> String {
        if !self.alphabet.is_nucleic() || frame == 0 || frame.unsigned_abs() > 3 {
            return String::new();
        }
        let offset = usize::from(frame.unsigned_abs() - 1);
        if frame > 0 {
            translate(self.sequence.as_bytes().get(offset..).unwrap_or_default())
        } else {
            let rc = reverse_complement(&self.sequence, self.alphabet == Alphabet::Rna);
            translate(rc.as_bytes().get(offset..).unwrap_or_default())
        }
    }

    /// Where `query` occurs: on both strands for nucleic acids (IUPAC
    /// codes match any base of their set, in the query and in the
    /// sequence), case-insensitively for proteins (`X` matches any).
    /// Across the origin of a circular sequence too. At most
    /// [`MAX_HITS`], by start.
    ///
    /// # Errors
    ///
    /// [`SearchError`] for an empty query or a character that is not a
    /// residue code.
    pub fn search(&self, query: &str) -> Result<Vec<SeqHit>, SearchError> {
        let query: Vec<u8> = query.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        if query.is_empty() {
            return Err(SearchError::Empty);
        }
        let nucleic = self.alphabet.is_nucleic();
        if let Some(&bad) = query.iter().find(|&&b| {
            if nucleic {
                iupac_mask(b) == 0
            } else {
                !b.is_ascii_alphabetic() && b != b'*'
            }
        }) {
            return Err(SearchError::NotIupac(char::from(bad)));
        }
        let len = self.len();
        let q = query.len();
        if q > len {
            return Ok(Vec::new());
        }
        // A circular sequence is searched with its start appended, so a
        // match may cross the origin.
        let haystack: Vec<u8> = if self.is_circular() {
            let mut h = self.sequence.as_bytes().to_vec();
            h.extend_from_slice(&self.sequence.as_bytes()[..q - 1]);
            h
        } else {
            self.sequence.as_bytes().to_vec()
        };
        let starts = haystack.len() - q + 1;
        let mut hits = Vec::new();
        let scan = |pattern: &[u8], strand: Strand, hits: &mut Vec<SeqHit>| {
            for at in 0..starts.min(len) {
                if hits.len() >= MAX_HITS {
                    break;
                }
                if matches_at(&haystack, at, pattern, nucleic) {
                    hits.push(SeqHit {
                        start: at,
                        end: (at + q) % len.max(1),
                        strand,
                    });
                }
            }
        };
        scan(
            &query,
            if nucleic {
                Strand::Forward
            } else {
                Strand::None
            },
            &mut hits,
        );
        if nucleic {
            let rc: Vec<u8> = query.iter().rev().map(|&b| complement(b, false)).collect();
            if rc != query {
                scan(&rc, Strand::Reverse, &mut hits);
            }
        }
        for hit in &mut hits {
            if hit.end == 0 {
                hit.end = len;
            }
        }
        hits.sort_by_key(|h| (h.start, h.strand == Strand::Reverse));
        // A stretch both the query and its reverse complement match is
        // one hit (the forward one).
        hits.dedup_by_key(|h| (h.start, h.end));
        hits.truncate(MAX_HITS);
        Ok(hits)
    }

    /// Residue counts by letter (upper case), gaps left out.
    #[must_use]
    pub fn composition(&self) -> BTreeMap<char, usize> {
        let mut counts = BTreeMap::new();
        for b in self.sequence.bytes().filter(|&b| b != b'-') {
            *counts
                .entry(char::from(b.to_ascii_uppercase()))
                .or_default() += 1;
        }
        counts
    }

    /// Characters that are not residue codes of the alphabet.
    #[must_use]
    pub fn invalid_residues(&self) -> usize {
        self.sequence
            .bytes()
            .filter(|&b| !is_residue(b, self.alphabet))
            .count()
    }

    /// Problems of the CDS features: a length that is not a multiple of
    /// 3, no ATG/GTG/TTG start, stop codons inside the frame, no stop at
    /// the end. Partial or `/codon_start` CDSs are checked from their
    /// first full codon.
    #[must_use]
    pub fn cds_issues(&self) -> Vec<String> {
        if !self.alphabet.is_nucleic() {
            return Vec::new();
        }
        let mut issues = Vec::new();
        for f in self.features.iter().filter(|f| f.kind == "CDS") {
            let offset = f
                .qualifier("codon_start")
                .and_then(|v| v.trim().parse::<usize>().ok())
                .map_or(0, |c| c.saturating_sub(1).min(2));
            let seq = self.feature_sequence(f);
            let coding = seq.as_bytes().get(offset..).unwrap_or_default();
            let name = format!("CDS {} ({})", f.name(), f.location_text());
            if coding.len() % 3 != 0 {
                issues.push(format!(
                    "{name}: length {} is not a multiple of 3 (frame shift?)",
                    coding.len()
                ));
            }
            let protein = translate(coding).into_bytes();
            if offset == 0 && !matches!(coding.get(..3), Some(b"ATG" | b"GTG" | b"TTG")) {
                issues.push(format!("{name}: does not start with ATG/GTG/TTG"));
            }
            let body = protein.split_last().map_or(&[][..], |(_, body)| body);
            let stops = body.iter().filter(|&&a| a == b'*').count();
            if stops > 0 {
                issues.push(format!("{name}: {stops} premature stop codon(s)"));
            }
            if coding.len() % 3 == 0 && protein.last() != Some(&b'*') {
                issues.push(format!("{name}: no stop codon at the end"));
            }
        }
        issues
    }

    /// Enzymes of the cut sites with exactly one site.
    #[must_use]
    pub fn unique_cutters(&self) -> Vec<&str> {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for c in &self.cut_sites {
            *counts.entry(c.enzyme.as_str()).or_default() += 1;
        }
        counts
            .into_iter()
            .filter(|(_, n)| *n == 1)
            .map(|(e, _)| e)
            .collect()
    }

    /// Feature counts by kind.
    #[must_use]
    pub fn kind_counts(&self) -> BTreeMap<&str, usize> {
        let mut counts = BTreeMap::new();
        for f in &self.features {
            *counts.entry(f.kind.as_str()).or_default() += 1;
        }
        counts
    }

    /// The sequence as FASTA, 70 residues per line.
    #[must_use]
    pub fn to_fasta(&self) -> String {
        let mut header = self.name.clone().unwrap_or_else(|| self.display_title());
        if let Some(d) = &self.description {
            header = format!("{header} {d}");
        }
        fasta(&header, &self.sequence)
    }

    /// The features as CSV: kind, label, location, strand, length.
    #[must_use]
    pub fn features_csv(&self) -> String {
        let mut out = String::from("kind,label,location,strand,length\n");
        for f in &self.features {
            let strand = match f.strand {
                Strand::Forward => "+",
                Strand::Reverse => "-",
                Strand::None => "",
            };
            let _ = writeln!(
                out,
                "{},{},{},{strand},{}",
                csv_field(&f.kind),
                csv_field(f.label.as_deref().unwrap_or("")),
                csv_field(&f.location_text()),
                f.len()
            );
        }
        out
    }

    /// The cut sites as CSV: enzyme, site, position (1-based site start),
    /// cut (1-based, the cut is after this residue), strand.
    #[must_use]
    pub fn sites_csv(&self) -> String {
        let mut out = String::from("enzyme,site,position,cut_after,strand\n");
        for c in &self.cut_sites {
            let strand = if c.strand == Strand::Reverse {
                "-"
            } else {
                "+"
            };
            let _ = writeln!(
                out,
                "{},{},{},{},{strand}",
                c.enzyme,
                c.site,
                c.start + 1,
                c.cut
            );
        }
        out
    }
}

/// `residues` as a FASTA record under `header`, 70 per line.
#[must_use]
pub fn fasta(header: &str, residues: &str) -> String {
    let mut out = format!(">{header}\n");
    for chunk in residues.as_bytes().chunks(70) {
        out.push_str(&String::from_utf8_lossy(chunk));
        out.push('\n');
    }
    out
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// A stable palette slot for a feature kind: the common kinds get their
/// own (CDS, gene, promoter, terminator, origin), the rest share by name.
#[must_use]
pub fn feature_slot(kind: &str) -> usize {
    match kind {
        "CDS" => 0,
        "gene" | "mRNA" => 1,
        "promoter" | "enhancer" | "RBS" | "regulatory" => 2,
        "terminator" | "polyA_signal" => 3,
        "rep_origin" | "origin" | "oriT" => 4,
        other => {
            let hash = other.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
            });
            usize::try_from(hash % 5).unwrap_or(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plasmid() -> SequenceSpec {
        SequenceSpec {
            title: None,
            path: Some("dna/p.gb".into()),
            format: SequenceFormat::Genbank,
            name: Some("pTest".into()),
            description: None,
            accession: None,
            organism: None,
            alphabet: Alphabet::Dna,
            topology: Topology::Circular,
            // ATG AAA TAA, then a GAATTC (EcoRI) site, then filler; 30 bp.
            sequence: "ATGAAATAAGAATTCCCCGGGTTTTAAAAC".into(),
            features: vec![SeqFeature {
                kind: "CDS".into(),
                label: Some("orf".into()),
                segments: vec![SeqSpan { start: 0, end: 9 }],
                strand: Strand::Forward,
                qualifiers: Vec::new(),
            }],
            cut_sites: Vec::new(),
            enzyme_source: None,
            records: 1,
            record: 0,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn complements_translations_and_gc() {
        assert_eq!(reverse_complement("ATGCRYN", false), "NRYGCAT");
        assert_eq!(reverse_complement("AUG", true), "CAU");
        assert_eq!(translate(b"ATGAAATAAGG"), "MK*");
        assert_eq!(codon_amino(b"ANG"), b'X');
        assert_eq!(gc_percent(b"GGCCAATT"), Some(50.0));
        assert_eq!(gc_percent(b"NNN"), None);
        assert_eq!(guess_alphabet("ACGUACGU"), Alphabet::Rna);
        assert_eq!(guess_alphabet("MKVLAAGIVGLL"), Alphabet::Protein);
        assert_eq!(guess_alphabet("ACGTNNACGT"), Alphabet::Dna);
    }

    #[test]
    fn frames_read_both_strands() {
        let spec = plasmid();
        assert_eq!(&spec.frame_translation(1)[..3], "MK*");
        // Frame -1 reads the reverse complement from the last base.
        let rc = reverse_complement(&spec.sequence, false);
        assert_eq!(spec.frame_translation(-1), translate(rc.as_bytes()));
        assert_eq!(spec.frame_translation(4), "");
    }

    #[test]
    fn search_uses_iupac_codes_both_strands_and_the_origin() {
        let spec = plasmid();
        // EcoRI is palindromic: one hit, forward.
        assert_eq!(
            spec.search("GAATTC").unwrap(),
            vec![SeqHit {
                start: 9,
                end: 15,
                strand: Strand::Forward
            }]
        );
        // R = A or G.
        assert_eq!(spec.search("GRATTC").unwrap().len(), 1);
        // TTTA reversed and complemented is TAAA: found on both strands.
        let hits = spec.search("TTTA").unwrap();
        assert!(hits.iter().any(|h| h.strand == Strand::Forward));
        assert!(hits.iter().any(|h| h.strand == Strand::Reverse));
        // AAC + ATG crosses the origin of the circle.
        let wrap = spec.search("ACATG").unwrap();
        assert_eq!(wrap.len(), 1);
        assert_eq!((wrap[0].start, wrap[0].end), (28, 3));
        assert_eq!(spec.subsequence(28, 3), "ACATG");
        assert_eq!(spec.search(""), Err(SearchError::Empty));
        assert_eq!(spec.search("AZ"), Err(SearchError::NotIupac('Z')));
    }

    #[test]
    fn cds_checks_and_csv() {
        let mut spec = plasmid();
        assert!(spec.cds_issues().is_empty(), "{:?}", spec.cds_issues());
        spec.features[0].segments[0].end = 8;
        let issues = spec.cds_issues();
        assert!(issues[0].contains("not a multiple of 3"), "{issues:?}");
        spec.features[0].segments[0] = SeqSpan { start: 3, end: 12 };
        let issues = spec.cds_issues();
        assert!(issues.iter().any(|i| i.contains("premature")), "{issues:?}");
        assert!(
            issues.iter().any(|i| i.contains("does not start")),
            "{issues:?}"
        );
        assert_eq!(
            plasmid().features_csv(),
            "kind,label,location,strand,length\nCDS,orf,1..9,+,9\n"
        );
        let mut rev = plasmid().features[0].clone();
        rev.strand = Strand::Reverse;
        rev.segments.push(SeqSpan { start: 20, end: 21 });
        assert_eq!(rev.location_text(), "complement(join(1..9,21))");
    }

    #[test]
    fn validation_checks_bounds() {
        assert!(plasmid().validate().is_ok());
        let mut bad = plasmid();
        bad.features[0].segments[0].end = 31;
        assert!(bad.validate().is_err());
        let mut empty = plasmid();
        empty.sequence.clear();
        assert!(empty.validate().is_err());
        assert!(feature_slot("CDS") == 0 && feature_slot("misc_feature") < 5);
    }
}
