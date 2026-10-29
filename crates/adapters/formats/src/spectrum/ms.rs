//! Mass spectra from mzML and MGF through `mzdata` (Apache-2.0): one
//! scan's points (profile or centroid) with its id, MS level and
//! precursor m/z.

use std::io::Cursor;

use mzdata::io::mgf::MGFReader;
use mzdata::io::mzml::MzMLReader;
use mzdata::prelude::*;
use mzdata::spectrum::{MultiLayerSpectrum, SignalContinuity};

/// Most scans counted in one file.
pub const MAX_SCANS: usize = 100_000;

/// One scan.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
    /// The scan id (native id or MGF title).
    pub id: String,
    /// MS level.
    pub level: u8,
    /// Whether the points are centroids.
    pub centroid: bool,
    /// m/z values.
    pub mz: Vec<f64>,
    /// Intensities.
    pub intensity: Vec<f64>,
    /// The first precursor's m/z and charge.
    pub precursor: Option<(f64, Option<i32>)>,
}

/// The file's format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsFormat {
    /// mzML.
    Mzml,
    /// MGF.
    Mgf,
}

fn scan(spectrum: &MultiLayerSpectrum) -> Scan {
    let (mut mz, mut intensity) = (Vec::new(), Vec::new());
    for p in spectrum.peaks().iter() {
        mz.push(p.mz);
        intensity.push(f64::from(p.intensity));
    }
    Scan {
        id: spectrum.id().to_owned(),
        level: spectrum.ms_level(),
        centroid: spectrum.signal_continuity() == SignalContinuity::Centroid,
        mz,
        intensity,
        precursor: spectrum
            .precursor()
            .and_then(|p| p.ions.first())
            .map(|ion| (ion.mz, ion.charge)),
    }
}

/// Scan `index` of the file and the number of scans.
///
/// # Errors
///
/// A message when the file has no scans or fewer than `index + 1`.
pub fn read(bytes: &[u8], format: MsFormat, index: usize) -> Result<(Scan, usize), String> {
    let mut chosen = None;
    let mut count = 0usize;
    let mut visit = |spectrum: MultiLayerSpectrum| {
        if count == index {
            chosen = Some(scan(&spectrum));
        }
        count += 1;
    };
    match format {
        MsFormat::Mzml => {
            let reader: MzMLReader<_> = MzMLReader::new(Cursor::new(bytes));
            for spectrum in reader.take(MAX_SCANS) {
                visit(spectrum);
            }
        }
        MsFormat::Mgf => {
            let reader: MGFReader<_> = MGFReader::new(Cursor::new(bytes));
            for spectrum in reader.take(MAX_SCANS) {
                visit(spectrum);
            }
        }
    }
    let name = match format {
        MsFormat::Mzml => "mzML",
        MsFormat::Mgf => "MGF",
    };
    if count == 0 {
        return Err(format!("the {name} file has no spectra"));
    }
    let scan =
        chosen.ok_or_else(|| format!("scan {} does not exist: the file has {count}", index + 1))?;
    Ok((scan, count))
}
