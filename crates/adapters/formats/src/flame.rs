//! Profiles to a call tree: Brendan Gregg's folded stacks and pprof.
//!
//! - Folded (collapsed) stacks, one `frame;frame;frame count` line per
//!   stack, as `stackcollapse-*`, `cargo flamegraph --folded`, py-spy
//!   `--format raw`, async-profiler `collapsed` and `perf script | inferno`
//!   write. Our own parser (inferno's licence, CAL-1.0, is not one crest
//!   can depend on). A differential line (`stack before after`) counts
//!   the second number. `#` lines are comments.
//! - pprof `profile.proto`, gzipped or not (Go's `pprof`, `pprof-rs`,
//!   py-spy `--format speedscope` is not pprof). Decoded with our own
//!   message structs on prost's derive, reading only the fields a flame
//!   graph needs: sample types, samples, locations with their (inlined)
//!   lines and functions, and the string table.
//!
//! Both go through [`FlameTree`], which merges the stacks and flattens
//! the tree for the block.

use std::collections::HashMap;
use std::io::Read as _;

use domain::devops::flame::{FlameSpec, FlameTree, ProfileFormat};
use prost::Message as _;

/// Largest decompressed pprof profile.
pub const MAX_INFLATED_BYTES: u64 = 256 * 1024 * 1024;

/// Reads a profile in either format, picking the sample type named
/// `sample_type` of a pprof profile.
///
/// # Errors
///
/// A message naming `path`.
pub fn parse(bytes: &[u8], path: &str, sample_type: Option<&str>) -> Result<FlameSpec, String> {
    let mut spec = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut inflated = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .take(MAX_INFLATED_BYTES)
            .read_to_end(&mut inflated)
            .map_err(|e| format!("`{path}`: not a gzipped profile ({e})"))?;
        pprof(&inflated, sample_type)
    } else if let Some(text) = folded_text(bytes) {
        folded(text)
    } else {
        pprof(bytes, sample_type).map_err(|e| {
            format!("neither folded stacks (`a;b;c 12` lines) nor a pprof profile: {e}")
        })
    }
    .map_err(|e| format!("`{path}`: {e}"))?;
    spec.path = Some(path.to_owned());
    Ok(spec)
}

/// `bytes` as text when its first stack line looks folded.
fn folded_text(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))?;
    split_line(first).map(|_| text)
}

/// The frames and count of a folded line.
fn split_line(line: &str) -> Option<(&str, u64)> {
    let (stack, count) = line.rsplit_once(char::is_whitespace)?;
    let count = parse_count(count)?;
    let stack = stack.trim_end();
    // Differential folded output: `stack before after`; keep `after`.
    let stack = match stack.rsplit_once(char::is_whitespace) {
        Some((head, before)) if parse_count(before).is_some() && !head.trim().is_empty() => {
            head.trim_end()
        }
        _ => stack,
    };
    (!stack.is_empty()).then_some((stack, count))
}

fn parse_count(text: &str) -> Option<u64> {
    if let Ok(n) = text.parse::<u64>() {
        return Some(n);
    }
    // Some tools write fractional weights.
    text.parse::<f64>()
        .ok()
        .filter(|f| f.is_finite() && *f >= 0.0)
        .map(|f| f.round() as u64)
}

/// Folded stacks to a flame graph.
///
/// # Errors
///
/// When a line is not `frames count`, or no line has a positive count.
pub fn folded(text: &str) -> Result<FlameSpec, String> {
    let mut tree = FlameTree::new();
    for (ix, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((stack, count)) = split_line(line) else {
            return Err(format!(
                "line {} is not `frame;frame;... count`: {}",
                ix + 1,
                cut(line, 80)
            ));
        };
        tree.add(stack.split(';').map(str::trim), count);
    }
    if tree.is_empty() {
        return Err("no stack has a positive count".into());
    }
    Ok(tree.finish(ProfileFormat::Folded, "samples"))
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        let head: String = text.chars().take(max).collect();
        format!("{head}…")
    } else {
        text.to_owned()
    }
}

// The fields of `perftools.profiles.Profile` (profile.proto) a flame
// graph reads; prost skips the others.

#[derive(Clone, PartialEq, prost::Message)]
struct Profile {
    #[prost(message, repeated, tag = "1")]
    sample_type: Vec<ValueType>,
    #[prost(message, repeated, tag = "2")]
    sample: Vec<Sample>,
    #[prost(message, repeated, tag = "4")]
    location: Vec<Location>,
    #[prost(message, repeated, tag = "5")]
    function: Vec<Function>,
    #[prost(string, repeated, tag = "6")]
    string_table: Vec<String>,
    #[prost(int64, tag = "14")]
    default_sample_type: i64,
}

#[derive(Clone, PartialEq, prost::Message)]
struct ValueType {
    #[prost(int64, tag = "1")]
    r#type: i64,
    #[prost(int64, tag = "2")]
    unit: i64,
}

#[derive(Clone, PartialEq, prost::Message)]
struct Sample {
    #[prost(uint64, repeated, tag = "1")]
    location_id: Vec<u64>,
    #[prost(int64, repeated, tag = "2")]
    value: Vec<i64>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct Location {
    #[prost(uint64, tag = "1")]
    id: u64,
    #[prost(uint64, tag = "3")]
    address: u64,
    #[prost(message, repeated, tag = "4")]
    line: Vec<Line>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct Line {
    #[prost(uint64, tag = "1")]
    function_id: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
struct Function {
    #[prost(uint64, tag = "1")]
    id: u64,
    #[prost(int64, tag = "2")]
    name: i64,
}

/// A decoded pprof profile to a flame graph of one sample type.
///
/// # Errors
///
/// When the bytes do not decode, the sample type is unknown or no sample
/// has a positive value.
pub fn pprof(bytes: &[u8], sample_type: Option<&str>) -> Result<FlameSpec, String> {
    let profile = Profile::decode(bytes).map_err(|e| format!("not a pprof profile ({e})"))?;
    let string = |ix: i64| -> &str {
        usize::try_from(ix)
            .ok()
            .and_then(|i| profile.string_table.get(i))
            .map_or("", String::as_str)
    };
    if profile.sample_type.is_empty() {
        return Err("the pprof profile has no sample types".into());
    }
    let names: Vec<&str> = profile
        .sample_type
        .iter()
        .map(|t| string(t.r#type))
        .collect();
    let index = match sample_type {
        Some(wanted) => names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(wanted.trim()))
            .ok_or_else(|| {
                format!(
                    "no sample type `{wanted}`; the profile has {}",
                    names.join(", ")
                )
            })?,
        None => {
            let default = string(profile.default_sample_type);
            names
                .iter()
                .position(|n| profile.default_sample_type != 0 && *n == default)
                .unwrap_or(names.len() - 1)
        }
    };
    let chosen = &profile.sample_type[index];
    let unit = match string(chosen.unit) {
        "" => string(chosen.r#type).to_owned(),
        unit => format!("{} {unit}", string(chosen.r#type)),
    };
    let functions: HashMap<u64, &str> = profile
        .function
        .iter()
        .map(|f| (f.id, string(f.name)))
        .collect();
    // Frames of each location, outermost (caller) first: line[0] is the
    // innermost inlined function.
    let locations: HashMap<u64, Vec<String>> = profile
        .location
        .iter()
        .map(|loc| {
            let mut frames: Vec<String> = loc
                .line
                .iter()
                .rev()
                .map(|line| match functions.get(&line.function_id) {
                    Some(name) if !name.is_empty() => (*name).to_owned(),
                    _ => format!("0x{:x}", loc.address),
                })
                .collect();
            if frames.is_empty() {
                frames.push(format!("0x{:x}", loc.address));
            }
            (loc.id, frames)
        })
        .collect();
    let mut tree = FlameTree::new();
    for sample in &profile.sample {
        let value = sample.value.get(index).copied().unwrap_or(0);
        let Ok(value) = u64::try_from(value) else {
            continue;
        };
        // location_id[0] is the leaf.
        let frames = sample
            .location_id
            .iter()
            .rev()
            .filter_map(|id| locations.get(id))
            .flatten()
            .map(String::as_str);
        tree.add(frames, value);
    }
    if tree.is_empty() {
        return Err(format!("no sample has a positive `{}` value", names[index]));
    }
    Ok(tree.finish(ProfileFormat::Pprof, &unit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folded_lines_merge_and_differential_takes_after() {
        let spec = folded("# comment\nmain;a 3\nmain;a 2\nmain;b 1 4\n\nmain 1\n").unwrap();
        assert_eq!(spec.total(), 10);
        let names: Vec<_> = spec
            .nodes
            .iter()
            .map(|n| (n.name.as_str(), n.total))
            .collect();
        assert_eq!(names, [("all", 10), ("main", 10), ("a", 5), ("b", 4)]);
        assert!(folded("main;a three").unwrap_err().contains("line 1"));
        assert!(folded("a 0\n").unwrap_err().contains("positive"));
    }

    #[test]
    fn frames_may_contain_spaces() {
        let spec = folded("java;Foo.bar(int, int) 7\n").unwrap();
        assert_eq!(spec.nodes[2].name, "Foo.bar(int, int)");
    }

    fn sample_profile() -> Profile {
        let strings = [
            "",
            "samples",
            "count",
            "cpu",
            "nanoseconds",
            "main",
            "work",
            "inl",
        ];
        Profile {
            sample_type: vec![
                ValueType { r#type: 1, unit: 2 },
                ValueType { r#type: 3, unit: 4 },
            ],
            sample: vec![
                Sample {
                    location_id: vec![2, 1],
                    value: vec![3, 300],
                },
                Sample {
                    location_id: vec![1],
                    value: vec![1, 100],
                },
            ],
            location: vec![
                Location {
                    id: 1,
                    address: 0x10,
                    line: vec![Line { function_id: 1 }],
                },
                Location {
                    id: 2,
                    address: 0x20,
                    // `inl` inlined into `work`.
                    line: vec![Line { function_id: 3 }, Line { function_id: 2 }],
                },
            ],
            function: vec![
                Function { id: 1, name: 5 },
                Function { id: 2, name: 6 },
                Function { id: 3, name: 7 },
            ],
            string_table: strings.iter().map(|s| (*s).to_owned()).collect(),
            default_sample_type: 0,
        }
    }

    #[test]
    fn pprof_picks_the_last_sample_type_and_unrolls_inlined_lines() {
        let bytes = sample_profile().encode_to_vec();
        let spec = pprof(&bytes, None).unwrap();
        assert_eq!(spec.unit, "cpu nanoseconds");
        assert_eq!(spec.total(), 400);
        assert_eq!(spec.stack(3), ["main", "work", "inl"]);
        let counts = pprof(&bytes, Some("samples")).unwrap();
        assert_eq!(counts.total(), 4);
        assert!(
            pprof(&bytes, Some("alloc"))
                .unwrap_err()
                .contains("samples, cpu")
        );
    }

    #[test]
    fn gzipped_pprof_is_detected() {
        use std::io::Write as _;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(&sample_profile().encode_to_vec())
            .unwrap();
        let gz = encoder.finish().unwrap();
        let spec = parse(&gz, "cpu.pb.gz", None).unwrap();
        assert_eq!(spec.format, ProfileFormat::Pprof);
        assert_eq!(spec.path.as_deref(), Some("cpu.pb.gz"));
        let folded = parse(b"a;b 2\n", "x.folded", None).unwrap();
        assert_eq!(folded.format, ProfileFormat::Folded);
        assert!(parse(b"\x00\x01garbage", "x", None).is_err());
    }
}
