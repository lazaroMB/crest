//! Sequences for `show_sequence`: GenBank flat files through `gb-io`, our
//! own FASTA reader, and bare residues.
//!
//! GenBank gives the name, definition, accession, organism, topology and
//! molecule type, and the features with their locations (complements and
//! joins become a strand and segments in top-strand order) and the first
//! [`MAX_QUALIFIERS`] qualifiers. The `source` feature is not drawn: its
//! organism goes to the header. FASTA may hold several records; one is
//! shown. Residues are upper-cased; whitespace and digits (GenBank-style
//! numbering pasted inline) are dropped and any other character that is
//! not a residue code is counted in a warning. Cut sites come from the
//! bundled enzyme table ([`domain::science::enzymes`]).

use application::ports::formats::SequenceRequest;
use domain::science::enzymes::{ENZYME_SOURCE, ENZYMES, find_cut_sites};
use domain::science::sequence::{
    Alphabet, MAX_FEATURES, MAX_LENGTH, MAX_QUALIFIER_BYTES, MAX_QUALIFIERS, Qualifier, SeqFeature,
    SeqSpan, SequenceFormat, SequenceSpec, Strand, Topology, guess_alphabet,
};
use gb_io::seq::{Location, Seq};

/// Longest sequence searched for cut sites; a genome would have too many.
pub const MAX_SITE_SEARCH: usize = 1_000_000;
/// Most cut sites kept.
pub const MAX_SITES: usize = 20_000;
/// Record names listed in a warning about other records.
const LISTED_RECORDS: usize = 5;

/// Reads `bytes` as GenBank, FASTA or bare residues (told by the first
/// non-blank character) and fills in the cut sites when asked.
///
/// # Errors
///
/// A message when nothing reads as a sequence, the record does not exist
/// or the sequence is longer than [`MAX_LENGTH`].
pub fn parse(bytes: &[u8], request: &SequenceRequest) -> Result<SequenceSpec, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the file is not UTF-8 text".to_owned())?;
    let head = text.trim_start();
    let mut spec = if head.starts_with("LOCUS") {
        genbank(bytes, request)?
    } else if head.starts_with('>') {
        fasta(text, request)?
    } else {
        plain(text, request)?
    };
    if spec.sequence.is_empty() {
        return Err("no residues found".to_owned());
    }
    if spec.sequence.len() > MAX_LENGTH {
        return Err(format!(
            "the sequence has {} residues; at most {MAX_LENGTH} can be shown",
            spec.sequence.len()
        ));
    }
    if let Some(topology) = request.topology {
        spec.topology = topology;
    }
    let bad = spec.invalid_residues();
    if bad > 0 {
        spec.warnings.push(format!(
            "{bad} characters are not {} residue codes",
            spec.alphabet.name()
        ));
    }
    if request.enzymes && spec.alphabet.is_nucleic() {
        if spec.sequence.len() > MAX_SITE_SEARCH {
            spec.warnings.push(format!(
                "restriction sites are not searched in sequences over {MAX_SITE_SEARCH} residues"
            ));
        } else {
            let mut sites = find_cut_sites(spec.sequence.as_bytes(), spec.is_circular(), ENZYMES);
            if sites.len() > MAX_SITES {
                spec.warnings
                    .push(format!("only the first {MAX_SITES} cut sites are kept"));
                sites.truncate(MAX_SITES);
            }
            spec.cut_sites = sites;
            spec.enzyme_source = Some(ENZYME_SOURCE.to_owned());
        }
    }
    Ok(spec)
}

fn empty_spec(request: &SequenceRequest, format: SequenceFormat) -> SequenceSpec {
    SequenceSpec {
        title: None,
        path: request.path.clone(),
        format,
        name: None,
        description: None,
        accession: None,
        organism: None,
        alphabet: Alphabet::Dna,
        topology: Topology::Linear,
        sequence: String::new(),
        features: Vec::new(),
        cut_sites: Vec::new(),
        enzyme_source: None,
        records: 1,
        record: 0,
        warnings: Vec::new(),
    }
}

/// The residues of `text`: whitespace and digits dropped, upper-cased.
fn residues(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace() && !c.is_ascii_digit())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn check_record(record: usize, records: usize) -> Result<(), String> {
    if record >= records {
        return Err(format!(
            "record {} does not exist: the file has {records}",
            record + 1
        ));
    }
    Ok(())
}

fn others_warning(names: &[String], shown: usize) -> Option<String> {
    if names.len() < 2 {
        return None;
    }
    let listed: Vec<&str> = names
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != shown)
        .take(LISTED_RECORDS)
        .map(|(_, n)| n.as_str())
        .collect();
    Some(format!(
        "{} records in the file; showing {} (others: {}{})",
        names.len(),
        shown + 1,
        listed.join(", "),
        if names.len() - 1 > LISTED_RECORDS {
            ", …"
        } else {
            ""
        }
    ))
}

fn plain(text: &str, request: &SequenceRequest) -> Result<SequenceSpec, String> {
    check_record(request.record, 1)?;
    let mut spec = empty_spec(request, SequenceFormat::Plain);
    spec.sequence = residues(text);
    spec.alphabet = request
        .alphabet
        .unwrap_or_else(|| guess_alphabet(&spec.sequence));
    Ok(spec)
}

fn fasta(text: &str, request: &SequenceRequest) -> Result<SequenceSpec, String> {
    let mut records: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(header) = line.strip_prefix('>') {
            records.push((header.trim().to_owned(), String::new()));
        } else if let Some((_, body)) = records.last_mut()
            && !line.starts_with(';')
        {
            body.push_str(line);
        }
    }
    check_record(request.record, records.len())?;
    let names: Vec<String> = records
        .iter()
        .map(|(h, _)| h.split_whitespace().next().unwrap_or("").to_owned())
        .collect();
    let (header, body) = &records[request.record];
    let mut spec = empty_spec(request, SequenceFormat::Fasta);
    let mut words = header.splitn(2, char::is_whitespace);
    spec.name = words.next().filter(|s| !s.is_empty()).map(str::to_owned);
    spec.description = words
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    spec.sequence = residues(body);
    spec.alphabet = request
        .alphabet
        .unwrap_or_else(|| guess_alphabet(&spec.sequence));
    spec.records = records.len();
    spec.record = request.record;
    spec.warnings.extend(others_warning(&names, request.record));
    Ok(spec)
}

fn genbank(bytes: &[u8], request: &SequenceRequest) -> Result<SequenceSpec, String> {
    let seqs = gb_io::reader::parse_slice(bytes).map_err(|e| format!("invalid GenBank: {e}"))?;
    check_record(request.record, seqs.len())?;
    let names: Vec<String> = seqs
        .iter()
        .map(|s| s.name.clone().unwrap_or_default())
        .collect();
    let records = seqs.len();
    let Some(seq) = seqs.into_iter().nth(request.record) else {
        return Err("no record".to_owned());
    };
    let mut spec = record_spec(seq, request)?;
    spec.records = records;
    spec.record = request.record;
    spec.warnings.extend(others_warning(&names, request.record));
    Ok(spec)
}

fn record_spec(seq: Seq, request: &SequenceRequest) -> Result<SequenceSpec, String> {
    let mut spec = empty_spec(request, SequenceFormat::Genbank);
    spec.name = seq.name.clone().filter(|n| !n.is_empty());
    spec.description = seq
        .definition
        .clone()
        .map(|d| d.trim().trim_end_matches('.').to_owned())
        .filter(|d| !d.is_empty() && d != ".");
    let given = |v: &Option<String>| v.clone().filter(|v| !v.is_empty() && v != ".");
    spec.accession = given(&seq.version).or_else(|| given(&seq.accession));
    spec.organism = seq.source.as_ref().and_then(|s| {
        s.organism
            .clone()
            .or_else(|| Some(s.source.clone()))
            // The lineage follows the organism on its own lines.
            .and_then(|o| o.lines().next().map(|l| l.trim().to_owned()))
            .filter(|o| !o.is_empty())
    });
    spec.topology = if seq.is_circular() {
        Topology::Circular
    } else {
        Topology::Linear
    };
    let sequence =
        String::from_utf8(seq.seq.clone()).map_err(|_| "the sequence is not ASCII".to_owned())?;
    spec.sequence = residues(&sequence);
    spec.alphabet = request.alphabet.unwrap_or_else(|| {
        let molecule = seq
            .molecule_type
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        if molecule.contains("rna") {
            Alphabet::Rna
        } else if molecule.contains("aa") || molecule.contains("protein") {
            Alphabet::Protein
        } else if molecule.contains("dna") {
            Alphabet::Dna
        } else {
            guess_alphabet(&spec.sequence)
        }
    });
    let len = spec.sequence.len();
    let (mut dropped, mut kept) = (0usize, Vec::new());
    for f in &seq.features {
        if f.kind == "source" {
            continue;
        }
        let Some((segments, strand)) = location(&f.location, len, spec.is_circular()) else {
            dropped += 1;
            continue;
        };
        if kept.len() >= MAX_FEATURES {
            dropped += 1;
            continue;
        }
        let qualifiers: Vec<Qualifier> = f
            .qualifiers
            .iter()
            .take(MAX_QUALIFIERS)
            .map(|(k, v)| Qualifier {
                key: k.to_string(),
                value: clip(v.as_deref().unwrap_or("")),
            })
            .collect();
        let label = [
            "label",
            "gene",
            "product",
            "locus_tag",
            "standard_name",
            "note",
        ]
        .iter()
        .find_map(|key| f.qualifier_values(key).next())
        .map(|v| clip(v.trim()))
        .filter(|v| !v.is_empty());
        kept.push(SeqFeature {
            kind: f.kind.to_string(),
            label,
            segments,
            strand: if spec.alphabet.is_nucleic() {
                strand
            } else {
                Strand::None
            },
            qualifiers,
        });
    }
    spec.features = kept;
    if dropped > 0 {
        spec.warnings.push(format!(
            "{dropped} features left out (no usable location, or over {MAX_FEATURES})"
        ));
    }
    Ok(spec)
}

/// `value` cut to [`MAX_QUALIFIER_BYTES`] on a character boundary, with
/// whitespace runs (GenBank line breaks) joined.
fn clip(value: &str) -> String {
    let joined: String = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.len() <= MAX_QUALIFIER_BYTES {
        return joined;
    }
    let mut end = MAX_QUALIFIER_BYTES;
    while !joined.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &joined[..end])
}

/// The segments of `loc` in top-strand order and its strand; `None` when
/// nothing of it lies in the sequence.
fn location(loc: &Location, len: usize, circular: bool) -> Option<(Vec<SeqSpan>, Strand)> {
    let (segments, strand) = parts(loc, len, circular);
    (!segments.is_empty()).then_some((segments, strand))
}

fn parts(loc: &Location, len: usize, circular: bool) -> (Vec<SeqSpan>, Strand) {
    match loc {
        Location::Range((a, _), (b, _)) => {
            let (Ok(a), Ok(b)) = (usize::try_from(*a), usize::try_from(*b)) else {
                return (Vec::new(), Strand::Forward);
            };
            let span = |start: usize, end: usize| {
                let end = end.min(len);
                (start < end).then_some(SeqSpan { start, end })
            };
            let segments = if a < b {
                span(a, b).into_iter().collect()
            } else if circular && a > b {
                // Some writers give an origin-crossing range directly.
                span(a, len).into_iter().chain(span(0, b)).collect()
            } else {
                Vec::new()
            };
            (segments, Strand::Forward)
        }
        Location::Complement(inner) => {
            let (segments, strand) = parts(inner, len, circular);
            let flipped = match strand {
                Strand::Forward => Strand::Reverse,
                Strand::Reverse => Strand::Forward,
                Strand::None => Strand::None,
            };
            (segments, flipped)
        }
        Location::Join(list)
        | Location::Order(list)
        | Location::Bond(list)
        | Location::OneOf(list) => {
            let children: Vec<(Vec<SeqSpan>, Strand)> = list
                .iter()
                .map(|l| parts(l, len, circular))
                .filter(|(s, _)| !s.is_empty())
                .collect();
            let all_reverse =
                !children.is_empty() && children.iter().all(|(_, s)| *s == Strand::Reverse);
            if all_reverse {
                // join(complement(b), complement(a)) lists the parts in
                // reading order on the bottom strand: reversed, they are in
                // top-strand order.
                let segments = children.into_iter().rev().flat_map(|(s, _)| s).collect();
                (segments, Strand::Reverse)
            } else {
                let segments = children.into_iter().flat_map(|(s, _)| s).collect();
                (segments, Strand::Forward)
            }
        }
        Location::Between(..) | Location::External(..) | Location::Gap(_) => {
            (Vec::new(), Strand::Forward)
        }
    }
}
