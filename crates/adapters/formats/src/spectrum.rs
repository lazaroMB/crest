//! Spectra for `show_spectrum`: JCAMP-DX ([`jcamp`], our own reader) for
//! IR, Raman, UV-Vis and NMR, mzML and MGF ([`ms`], mzdata) for mass
//! spectra, told apart by content, and peak picking.
//!
//! Peaks of a trace are local extrema ranked by topographic prominence
//! (the height above the higher of the two lowest points between the peak
//! and the nearest higher point on each side, or the end), kept above a
//! share of the y range; IR in transmittance is picked for minima. Peaks
//! of sticks (centroided MS, a JCAMP peak table) are the strongest sticks
//! above the same share of the base peak. The result is listed and put in
//! a `crest-chem` envelope (`peak.<n>.x`, `peak.<n>.y`, strongest first).

pub mod jcamp;
pub mod ms;

use std::collections::BTreeMap;

use application::ports::formats::SpectrumRequest;
use domain::computed::{Computed, ComputedValue};
use domain::science::spectrum::{
    MAX_PEAKS, MAX_POINTS, Peak, PeakSense, SpectrumFormat, SpectrumKind, SpectrumSpec, range,
};
use serde_json::json;

use self::jcamp::Block;
use self::ms::MsFormat;
use crate::chem;

/// Kind of the peak envelope.
pub const KIND: &str = "chem.peak_picking";
/// Default smallest prominence, as a share of the y range.
pub const DEFAULT_THRESHOLD: f64 = 0.05;
/// Default number of peaks kept.
pub const DEFAULT_PEAKS: usize = 20;
/// Local extrema examined at most (the highest first).
const MAX_CANDIDATES: usize = 5_000;
/// Header records shown, with their labels.
const METADATA: [(&str, &str); 12] = [
    ("ORIGIN", "origin"),
    ("OWNER", "owner"),
    ("LONGDATE", "date"),
    ("DATE", "date"),
    ("MOLFORM", "formula"),
    ("NAMES", "names"),
    ("CASREGISTRYNO", "CAS"),
    ("SPECTROMETER/DATASYSTEM", "instrument"),
    ("SPECTROMETERDATASYSTEM", "instrument"),
    ("STATE", "state"),
    (".SOLVENTNAME", "solvent"),
    (".OBSERVENUCLEUS", "nucleus"),
];

/// Reads `bytes` (JCAMP-DX when it starts with `##`, mzML when it holds an
/// `<mzML` or `<indexedmzML` element, MGF when it has `BEGIN IONS`) and
/// picks the peaks.
///
/// # Errors
///
/// A message when the file is none of these, holds no data or the block
/// does not exist.
pub fn parse(bytes: &[u8], request: &SpectrumRequest) -> Result<SpectrumSpec, String> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_string();
    let trimmed = head.trim_start();
    let mut spec = if trimmed.starts_with("##") {
        let text = std::str::from_utf8(bytes).map_err(|_| "the JCAMP-DX file is not UTF-8 text")?;
        from_jcamp(text, request)?
    } else if head.contains("<mzML") || head.contains("<indexedmzML") {
        from_ms(bytes, MsFormat::Mzml, request)?
    } else if head.contains("BEGIN IONS") {
        from_ms(bytes, MsFormat::Mgf, request)?
    } else {
        return Err("not a spectrum file: expected JCAMP-DX (##TITLE=), mzML or MGF".into());
    };
    if spec.x.len() > MAX_POINTS {
        return Err(format!(
            "the spectrum has {} points; at most {MAX_POINTS} can be shown",
            spec.x.len()
        ));
    }
    pick_into(&mut spec, bytes, request);
    Ok(spec)
}

fn empty(request: &SpectrumRequest, format: SpectrumFormat) -> SpectrumSpec {
    SpectrumSpec {
        title: None,
        path: request.path.clone(),
        format,
        kind: SpectrumKind::Other,
        name: None,
        x_label: "x".into(),
        y_label: "y".into(),
        x_reversed: false,
        x: Vec::new(),
        y: Vec::new(),
        sticks: false,
        peaks: Vec::new(),
        sense: PeakSense::Maxima,
        picking: Computed::new("ch_0", KIND, "Peak picking", chem::engine("crest-chem")),
        metadata: Vec::new(),
        blocks: 1,
        block: 0,
        warnings: Vec::new(),
    }
}

/// What a JCAMP block measured, from `DATA TYPE` or else the x unit.
fn jcamp_kind(block: &Block) -> SpectrumKind {
    let data_type = block.get("DATATYPE").unwrap_or("").to_ascii_uppercase();
    let x_units = block.get("XUNITS").unwrap_or("").to_ascii_uppercase();
    if data_type.contains("RAMAN") {
        SpectrumKind::Raman
    } else if data_type.contains("INFRARED") || data_type.starts_with("IR") {
        SpectrumKind::Infrared
    } else if data_type.contains("UV") || data_type.contains("VIS") {
        SpectrumKind::UvVis
    } else if data_type.contains("NMR") {
        SpectrumKind::Nmr
    } else if data_type.contains("MASS") {
        SpectrumKind::Mass
    } else if x_units.contains("1/CM") {
        SpectrumKind::Infrared
    } else if x_units.contains("NANOMETER") {
        SpectrumKind::UvVis
    } else if x_units.contains("PPM") {
        SpectrumKind::Nmr
    } else if x_units.contains("M/Z") {
        SpectrumKind::Mass
    } else {
        SpectrumKind::Other
    }
}

fn from_jcamp(text: &str, request: &SpectrumRequest) -> Result<SpectrumSpec, String> {
    let blocks = jcamp::parse(text)?;
    let count = blocks.len();
    let Some(block) = blocks.get(request.block) else {
        return Err(format!(
            "block {} does not exist: the file has {count} data blocks",
            request.block + 1
        ));
    };
    let mut spec = empty(request, SpectrumFormat::Jcamp);
    spec.blocks = count;
    spec.block = request.block;
    spec.kind = jcamp_kind(block);
    spec.name = block.get("TITLE").map(str::to_owned);
    spec.x = block.x.clone();
    spec.y = block.y.clone();
    spec.sticks = block.sticks || spec.kind == SpectrumKind::Mass;
    spec.warnings = block.warnings.clone();
    let x_units = block.get("XUNITS").unwrap_or("").to_ascii_uppercase();
    let y_units = block.get("YUNITS").unwrap_or("").to_ascii_uppercase();
    // NMR in Hz: δ = ν / ν₀ with ν₀ the observe frequency in MHz.
    if spec.kind == SpectrumKind::Nmr && x_units.contains("HZ") {
        match block.number(".OBSERVEFREQUENCY").filter(|f| *f > 0.0) {
            Some(mhz) => {
                for x in &mut spec.x {
                    *x /= mhz;
                }
                spec.metadata.push(("frequency".into(), format!("{mhz} MHz")));
            }
            None => spec
                .warnings
                .push("x is in Hz and the file gives no .OBSERVE FREQUENCY: not converted to ppm".into()),
        }
    }
    let nucleus = block.get(".OBSERVENUCLEUS").map(|n| n.trim_start_matches('^').to_owned());
    spec.x_label = match spec.kind {
        SpectrumKind::Infrared => "wavenumber (cm⁻¹)".into(),
        SpectrumKind::Raman => "Raman shift (cm⁻¹)".into(),
        SpectrumKind::UvVis if x_units.contains("MICRO") => "wavelength (µm)".into(),
        SpectrumKind::UvVis => "wavelength (nm)".into(),
        SpectrumKind::Nmr if x_units.contains("HZ") && spec.metadata.is_empty() => "ν (Hz)".into(),
        SpectrumKind::Nmr => match &nucleus {
            Some(n) => format!("δ {n} (ppm)"),
            None => "δ (ppm)".into(),
        },
        SpectrumKind::Mass => "m/z".into(),
        SpectrumKind::Other => block
            .get("XUNITS")
            .map_or_else(|| "x".to_owned(), str::to_lowercase),
    };
    spec.y_label = if y_units.is_empty() || y_units.contains("ARBITRARY") {
        "intensity".into()
    } else {
        y_units.to_lowercase()
    };
    spec.x_reversed = matches!(spec.kind, SpectrumKind::Infrared | SpectrumKind::Nmr);
    spec.sense = if spec.kind == SpectrumKind::Infrared && y_units.contains("TRANSMITTANCE") {
        PeakSense::Minima
    } else {
        PeakSense::Maxima
    };
    for (key, label) in METADATA {
        if let Some(v) = block.get(key)
            && !spec.metadata.iter().any(|(l, _)| l == label)
        {
            let v: String = v.chars().take(120).collect();
            spec.metadata.push((label.to_owned(), v));
        }
    }
    if count > 1 {
        let names: Vec<String> = blocks
            .iter()
            .enumerate()
            .map(|(i, b)| format!("{}: {}", i + 1, b.get("TITLE").unwrap_or("untitled")))
            .take(8)
            .collect();
        spec.warnings
            .push(format!("{count} data blocks ({}); showing {}", names.join(", "), request.block + 1));
    }
    Ok(spec)
}

fn from_ms(bytes: &[u8], format: MsFormat, request: &SpectrumRequest) -> Result<SpectrumSpec, String> {
    let (scan, count) = ms::read(bytes, format, request.block)?;
    let mut spec = empty(
        request,
        match format {
            MsFormat::Mzml => SpectrumFormat::Mzml,
            MsFormat::Mgf => SpectrumFormat::Mgf,
        },
    );
    spec.kind = SpectrumKind::Mass;
    spec.name = Some(scan.id.clone()).filter(|s| !s.is_empty());
    spec.x_label = "m/z".into();
    spec.y_label = "intensity".into();
    spec.sticks = scan.centroid;
    spec.blocks = count;
    spec.block = request.block;
    // Points in m/z order, non-finite ones dropped.
    let mut points: Vec<(f64, f64)> = scan
        .mz
        .iter()
        .zip(&scan.intensity)
        .map(|(m, i)| (*m, *i))
        .filter(|(m, i)| m.is_finite() && i.is_finite())
        .collect();
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    spec.x = points.iter().map(|p| p.0).collect();
    spec.y = points.iter().map(|p| p.1).collect();
    if spec.x.is_empty() {
        return Err(format!("scan {} has no peaks", request.block + 1));
    }
    spec.metadata.push(("MS level".into(), scan.level.to_string()));
    if let Some((mz, charge)) = scan.precursor {
        let charge = charge.map(|z| format!(", charge {z:+}")).unwrap_or_default();
        spec.metadata.push(("precursor".into(), format!("m/z {mz:.4}{charge}")));
    }
    if count > 1 {
        spec.warnings
            .push(format!("{count} scans in the file; showing {}", request.block + 1));
    }
    Ok(spec)
}

/// Local maxima of `y` (the middle of a flat top), highest first.
fn candidates(y: &[f64]) -> Vec<usize> {
    let n = y.len();
    let mut out = Vec::new();
    let mut i = 1;
    while i + 1 < n {
        if y[i] > y[i - 1] {
            // Walk over a plateau.
            let mut j = i;
            while j + 1 < n && y[j + 1] == y[i] {
                j += 1;
            }
            if j + 1 < n && y[j + 1] < y[i] {
                out.push((i + j) / 2);
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out.sort_by(|&a, &b| y[b].total_cmp(&y[a]));
    out.truncate(MAX_CANDIDATES);
    out
}

/// The topographic prominence of the maximum at `i`.
fn prominence(y: &[f64], i: usize) -> f64 {
    let h = y[i];
    let mut left = h;
    for k in (0..i).rev() {
        if y[k] > h {
            break;
        }
        left = left.min(y[k]);
    }
    let mut right = h;
    for &v in &y[i + 1..] {
        if v > h {
            break;
        }
        right = right.min(v);
    }
    h - left.max(right)
}

/// The peaks of a trace: maxima of `y` (of `-y` for minima) with a
/// prominence of at least `threshold` × the y range, most prominent
/// first, at most `max`.
#[must_use]
pub fn pick_trace(x: &[f64], y: &[f64], sense: PeakSense, threshold: f64, max: usize) -> Vec<Peak> {
    let signal: Vec<f64> = match sense {
        PeakSense::Maxima => y.to_vec(),
        PeakSense::Minima => y.iter().map(|v| -v).collect(),
    };
    let Some((lo, hi)) = range(&signal) else {
        return Vec::new();
    };
    let floor = threshold * (hi - lo);
    let mut peaks: Vec<Peak> = candidates(&signal)
        .into_iter()
        .map(|i| (i, prominence(&signal, i)))
        .filter(|(_, p)| *p >= floor && *p > 0.0)
        .map(|(i, p)| Peak {
            x: x[i],
            y: y[i],
            prominence: Some(p),
        })
        .collect();
    peaks.sort_by(|a, b| b.prominence.unwrap_or(0.0).total_cmp(&a.prominence.unwrap_or(0.0)));
    peaks.truncate(max);
    peaks
}

/// The strongest sticks at or above `threshold` × the base peak, at most
/// `max`, strongest first.
#[must_use]
pub fn pick_sticks(x: &[f64], y: &[f64], threshold: f64, max: usize) -> Vec<Peak> {
    let base = y.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mut order: Vec<usize> = (0..x.len().min(y.len()))
        .filter(|&i| y[i] > 0.0 && y[i] >= threshold * base)
        .collect();
    order.sort_by(|&a, &b| y[b].total_cmp(&y[a]));
    order
        .into_iter()
        .take(max)
        .map(|i| Peak {
            x: x[i],
            y: y[i],
            prominence: None,
        })
        .collect()
}

/// Decimals for peak positions of `kind`.
fn x_decimals(kind: SpectrumKind) -> u32 {
    match kind {
        SpectrumKind::Infrared | SpectrumKind::Raman | SpectrumKind::UvVis => 1,
        SpectrumKind::Nmr => 3,
        SpectrumKind::Mass => 4,
        SpectrumKind::Other => 3,
    }
}

fn pick_into(spec: &mut SpectrumSpec, bytes: &[u8], request: &SpectrumRequest) {
    let threshold = request
        .threshold
        .filter(|t| t.is_finite() && *t >= 0.0 && *t < 1.0)
        .unwrap_or(DEFAULT_THRESHOLD);
    let max = request.max_peaks.unwrap_or(DEFAULT_PEAKS).clamp(1, MAX_PEAKS);
    spec.peaks = if spec.sticks {
        pick_sticks(&spec.x, &spec.y, threshold, max)
    } else {
        pick_trace(&spec.x, &spec.y, spec.sense, threshold, max)
    };
    let params = json!({
        "block": request.block,
        "threshold": threshold,
        "max_peaks": max,
    });
    let mut id_bytes = blake3_input(bytes);
    id_bytes.extend(params.to_string().bytes());
    let method = if spec.sticks {
        "Strongest sticks"
    } else {
        "Peak picking by prominence"
    };
    let mut computed = Computed::new(
        chem::derive_id(KIND, &id_bytes),
        KIND,
        method,
        chem::engine(match spec.format {
            SpectrumFormat::Jcamp => "crest JCAMP-DX reader",
            SpectrumFormat::Mzml | SpectrumFormat::Mgf => "mzdata 0.67.4",
        }),
    );
    computed.method.why = Some(if spec.sticks {
        format!(
            "Sticks at or above {:.0}% of the base peak, strongest first.",
            threshold * 100.0
        )
    } else {
        format!(
            "Local {} with a topographic prominence of at least {:.0}% of the y range, most \
             prominent first.",
            match spec.sense {
                PeakSense::Maxima => "maxima",
                PeakSense::Minima => "minima",
            },
            threshold * 100.0
        )
    });
    computed.method.params = params
        .as_object()
        .map(|o| o.clone().into_iter().collect())
        .unwrap_or_default();
    computed.inputs = vec![chem::input(
        spec.path.rsplit('/').next().unwrap_or(&spec.path),
        "data",
        bytes,
        Some(&spec.path),
    )];
    let mut values = BTreeMap::new();
    let mut count = ComputedValue::new(spec.peaks.len() as f64);
    count.decimals = Some(0);
    values.insert("peaks".to_owned(), count);
    for (i, p) in spec.peaks.iter().enumerate() {
        let mut x = ComputedValue::new(p.x);
        x.decimals = Some(x_decimals(spec.kind));
        values.insert(format!("peak.{}.x", i + 1), x);
        let mut y = ComputedValue::new(p.y);
        y.sig = Some(4);
        values.insert(format!("peak.{}.y", i + 1), y);
    }
    computed.values = values;
    spec.picking = computed;
}

/// The bytes the id is derived from: the file's hash (the file itself
/// can be large).
fn blake3_input(bytes: &[u8]) -> Vec<u8> {
    chem::hash(bytes).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prominence_ranks_peaks_and_minima_flip() {
        let x: Vec<f64> = (0..9).map(f64::from).collect();
        // A tall peak at 2, a small bump at 5 on a shoulder, a plateau
        // peak at 7..8? (edge: not a peak).
        let y = [0.0, 1.0, 10.0, 1.0, 2.0, 3.0, 2.5, 4.0, 4.0];
        let peaks = pick_trace(&x, &y, PeakSense::Maxima, 0.0, 10);
        assert_eq!(peaks[0].x, 2.0);
        assert_eq!(peaks[0].prominence, Some(9.0));
        assert_eq!(peaks[1].x, 5.0);
        assert_eq!(peaks[1].prominence, Some(0.5));
        // 5% of the range (0.5) keeps the bump, 10% does not.
        assert_eq!(pick_trace(&x, &y, PeakSense::Maxima, 0.1, 10).len(), 1);
        let dips: Vec<f64> = y.iter().map(|v| 10.0 - v).collect();
        let minima = pick_trace(&x, &dips, PeakSense::Minima, 0.0, 10);
        assert_eq!((minima[0].x, minima[0].y), (2.0, 0.0));
        let sticks = pick_sticks(&[1.0, 2.0, 3.0], &[5.0, 100.0, 1.0], 0.02, 10);
        assert_eq!(sticks.iter().map(|p| p.x).collect::<Vec<_>>(), [2.0, 1.0]);
    }
}
