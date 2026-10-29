//! Digital waveform dumps (VCD, FST, GHW) on `wellen` 0.25.7 (BSD-3,
//! Surfer's backend).
//!
//! The header gives the scope tree and the variables; the selected
//! variables' signals are loaded single-threaded from an in-memory copy
//! of the file and their changes copied as strings: bits most significant
//! first, reals as Rust prints them, text as is. Consecutive repeats of a
//! value (VCD files rewrite values at `$dumpvars`) are dropped, so every
//! change is a visible edge. A var's bit range (`[7:0]`) is appended to
//! its name. A reader panic on a malformed file becomes an error.

use std::collections::HashMap;
use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};

use application::ports::formats::WaveRequest;
use domain::engineering::waveform::{
    MAX_CHANGES, MAX_SIGNALS, SignalKind, Timescale, WaveChange, WaveFormat, WaveSignal,
    WaveformSpec,
};
use wellen::viewers::{read_body, read_header};
use wellen::{FileFormat, Hierarchy, ItemRef, LoadOptions, SignalValueRef, VarRef, VarType};

/// One variable picked for the block.
struct Picked {
    var: VarRef,
    scope: String,
    name: String,
}

/// Whether `text` matches `pattern`, where `*` stands for any run of
/// characters.
fn wildcard(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || text.len() < first.len() + last.len() {
        return false;
    }
    let mut rest = &text[first.len()..];
    for middle in &parts[1..parts.len() - 1] {
        match rest.find(middle) {
            Some(at) => rest = &rest[at + middle.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// Whether the signal at `path` (named `name`) is selected by
/// `patterns`: a pattern is a dotted path (the signal, or a scope and
/// everything under it), a bare signal name, or either with `*`.
pub fn selected(patterns: &[String], path: &str, name: &str) -> bool {
    patterns.is_empty()
        || patterns.iter().any(|p| {
            if p.contains('*') {
                wildcard(p, path) || wildcard(p, name)
            } else {
                path == p || name == p || path.starts_with(&format!("{p}."))
            }
        })
}

fn walk(h: &Hierarchy, items: Vec<ItemRef>, scope: &str, out: &mut Vec<Picked>) {
    for item in items {
        match item {
            ItemRef::Var(var) => {
                let v = &h[var];
                let mut name = v.name(h).to_owned();
                if let Some(index) = v.index()
                    && index.width() > 1
                    && !name.contains('[')
                {
                    name.push_str(&format!("[{}:{}]", index.msb(), index.lsb()));
                }
                out.push(Picked {
                    var,
                    scope: scope.to_owned(),
                    name,
                });
            }
            ItemRef::Scope(child) => {
                let s = &h[child];
                let path = if scope.is_empty() {
                    s.name(h).to_owned()
                } else {
                    format!("{scope}.{}", s.name(h))
                };
                walk(h, s.items(h).collect(), &path, out);
            }
        }
    }
}

fn exponent(unit: wellen::TimescaleUnit) -> i8 {
    unit.to_exponent().unwrap_or(-9)
}

fn value_text(value: &SignalValueRef<'_>) -> String {
    match value {
        SignalValueRef::Event => "1".to_owned(),
        SignalValueRef::BitVec(bits) => bits.bit_string(),
        SignalValueRef::String(text) => (*text).to_owned(),
        SignalValueRef::Real(x) => x.to_string(),
    }
}

fn read(bytes: Vec<u8>, request: &WaveRequest) -> Result<WaveformSpec, String> {
    let options = LoadOptions {
        multi_thread: false,
        remove_scopes_with_empty_name: false,
    };
    let header = read_header(Cursor::new(bytes), &options).map_err(|e| e.to_string())?;
    let format = match header.file_format {
        FileFormat::Vcd => WaveFormat::Vcd,
        FileFormat::Fst => WaveFormat::Fst,
        FileFormat::Ghw => WaveFormat::Ghw,
        FileFormat::Unknown => return Err("not a VCD, FST or GHW file".into()),
    };
    let h = header.hierarchy;
    let mut all = Vec::new();
    walk(&h, h.items().collect(), "", &mut all);
    let total = all.len();
    if total == 0 {
        return Err("the dump has no signals".into());
    }
    let picked: Vec<Picked> = all
        .into_iter()
        .filter(|p| {
            let path = if p.scope.is_empty() {
                p.name.clone()
            } else {
                format!("{}.{}", p.scope, p.name)
            };
            let bare = h[p.var].name(&h);
            selected(&request.signals, &path, bare)
        })
        .take(MAX_SIGNALS)
        .collect();
    if picked.is_empty() {
        return Err(format!(
            "no signal matches {}; the dump has {total} signals",
            request.signals.join(", ")
        ));
    }
    let body = read_body(header.body, &h, None).map_err(|e| e.to_string())?;
    let mut source = body.source;
    let times = body.time_table;
    let mut ids: Vec<wellen::SignalRef> = picked.iter().map(|p| h[p.var].signal_ref()).collect();
    ids.sort();
    ids.dedup();
    let loaded: HashMap<wellen::SignalRef, wellen::Signal> = source
        .load_signals(&ids, &h, false)
        .into_iter()
        .map(|s| (s.signal_ref(), s))
        .collect();
    let per_signal = (MAX_CHANGES / picked.len()).max(1);
    let signals = picked
        .into_iter()
        .map(|p| {
            let var = &h[p.var];
            let width = var.length(&h).unwrap_or(0);
            let kind = if var.var_type() == VarType::Event {
                SignalKind::Event
            } else if var.is_real(&h) {
                SignalKind::Real
            } else if var.is_string(&h) {
                SignalKind::Text
            } else if width == 1 {
                SignalKind::Bit
            } else {
                SignalKind::Vector
            };
            let mut changes: Vec<WaveChange> = Vec::new();
            let mut truncated = false;
            if let Some(signal) = loaded.get(&var.signal_ref()) {
                for (ix, value) in signal.iter_changes() {
                    let Some(t) = times.get(ix as usize).copied() else {
                        continue;
                    };
                    let v = value_text(&value);
                    if kind != SignalKind::Event && changes.last().is_some_and(|c| c.v == v) {
                        continue;
                    }
                    if changes.len() == per_signal {
                        truncated = true;
                        break;
                    }
                    changes.push(WaveChange { t, v });
                }
            }
            WaveSignal {
                name: p.name,
                scope: p.scope,
                kind,
                width,
                changes,
                truncated,
            }
        })
        .collect();
    let timescale = h
        .timescale()
        .map_or_else(Timescale::default, |t| Timescale {
            factor: t.factor,
            exponent: exponent(t.unit),
        });
    let generator = [h.version(), h.date()]
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    Ok(WaveformSpec {
        title: None,
        path: request.path.clone(),
        format,
        timescale,
        start: times.first().copied().unwrap_or(0),
        end: times.last().copied().unwrap_or(0),
        signals,
        total_signals: total,
        radix: request.radix,
        generator: (!generator.is_empty()).then_some(generator),
    })
}

/// Reads a dump into a block spec.
///
/// # Errors
///
/// The reader's message, or a note that it failed on the file.
pub fn parse(bytes: &[u8], request: &WaveRequest) -> Result<WaveformSpec, String> {
    let owned = bytes.to_vec();
    catch_unwind(AssertUnwindSafe(|| read(owned, request)))
        .unwrap_or_else(|_| Err("the waveform reader failed on this file (malformed dump?)".into()))
        .map_err(|e| format!("`{}`: {e}", request.path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_select_paths_scopes_and_names() {
        let p = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert!(selected(&[], "tb.clk", "clk"));
        assert!(selected(&p(&["tb.clk"]), "tb.clk", "clk"));
        assert!(selected(&p(&["tb"]), "tb.dut.q", "q"));
        assert!(!selected(&p(&["tb.du"]), "tb.dut.q", "q"));
        assert!(selected(&p(&["clk"]), "tb.dut.clk", "clk"));
        assert!(selected(&p(&["*clk*"]), "tb.dut.clk_en", "clk_en"));
        assert!(selected(&p(&["tb.*.q"]), "tb.dut.q", "q"));
        assert!(!selected(&p(&["tb.*.q"]), "tb.dut.d", "d"));
        assert!(wildcard("a*", "a"));
        assert!(!wildcard("ab*ba", "aba"));
    }
}
