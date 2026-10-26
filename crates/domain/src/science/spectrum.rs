//! A spectrum with its picked peaks (`show_spectrum`): IR, Raman, UV-Vis
//! and NMR from JCAMP-DX, mass spectra from mzML and MGF.
//!
//! The adapter reads the file into [`SpectrumSpec`]: the points of the
//! chosen block or scan, its axis units and conventions (wavenumber and
//! chemical shift run right to left), and the peaks the engine picked,
//! listed and in a [`Computed`] envelope (`peak.<n>.x`, `peak.<n>.y`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::block::BlockError;
use crate::computed::Computed;

/// Most points kept per spectrum.
pub const MAX_POINTS: usize = 2_000_000;
/// Most peaks picked.
pub const MAX_PEAKS: usize = 100;

/// What was measured.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpectrumKind {
    /// Infrared (wavenumber, right to left).
    Infrared,
    /// Raman (shift, left to right).
    Raman,
    /// UV-visible (wavelength).
    UvVis,
    /// NMR (chemical shift, right to left).
    Nmr,
    /// Mass spectrum (m/z).
    Mass,
    /// Anything else.
    #[default]
    Other,
}

impl SpectrumKind {
    /// Its name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Infrared => "IR",
            Self::Raman => "Raman",
            Self::UvVis => "UV-Vis",
            Self::Nmr => "NMR",
            Self::Mass => "mass",
            Self::Other => "spectrum",
        }
    }
}

/// The file format read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpectrumFormat {
    /// JCAMP-DX 4.24 / 5.01.
    #[default]
    Jcamp,
    /// mzML (HUPO-PSI).
    Mzml,
    /// Mascot generic format.
    Mgf,
}

/// Whether peaks are maxima (absorbance, counts) or minima (IR
/// transmittance).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PeakSense {
    /// Peaks point up.
    #[default]
    Maxima,
    /// Peaks point down.
    Minima,
}

/// A picked peak.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Peak {
    /// Position on the x axis.
    pub x: f64,
    /// Height (or depth) as read.
    pub y: f64,
    /// Prominence above (below) the surrounding baseline, in y units;
    /// `None` for a listed stick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prominence: Option<f64>,
}

/// A spectrum with its peaks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SpectrumSpec {
    /// Title shown above the plot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The workspace file read.
    pub path: String,
    /// Its format.
    pub format: SpectrumFormat,
    /// What was measured.
    pub kind: SpectrumKind,
    /// The block's or scan's own title or id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// x axis label with its unit (`wavenumber (cm⁻¹)`, `δ (ppm)`, `m/z`).
    pub x_label: String,
    /// y axis label with its unit (`transmittance`, `intensity`).
    pub y_label: String,
    /// Whether x runs right to left (IR wavenumber, NMR shift).
    #[serde(default)]
    pub x_reversed: bool,
    /// The points, in file order.
    pub x: Vec<f64>,
    /// One y per x.
    pub y: Vec<f64>,
    /// Whether the points are sticks (centroided MS, a peak table)
    /// rather than a continuous trace.
    #[serde(default)]
    pub sticks: bool,
    /// The peaks picked by the engine (or the file's sticks, strongest
    /// first), at most [`MAX_PEAKS`].
    #[serde(default)]
    pub peaks: Vec<Peak>,
    /// Which way peaks point.
    #[serde(default)]
    pub sense: PeakSense,
    /// The peak picking as an envelope.
    pub picking: Computed,
    /// Header values worth showing (origin, owner, molecular formula,
    /// nucleus, frequency, precursor).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<(String, String)>,
    /// Data blocks or scans in the file.
    #[serde(default = "one")]
    pub blocks: usize,
    /// The block or scan shown, 0-based.
    #[serde(default)]
    pub block: usize,
    /// Reader notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

fn one() -> usize {
    1
}

impl SpectrumSpec {
    /// Checks the points and the envelope.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] describing the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        let invalid = |reason: String| BlockError::Invalid {
            kind: "spectrum",
            reason,
        };
        if self.x.is_empty() {
            return Err(invalid("the spectrum has no points".into()));
        }
        if self.x.len() != self.y.len() {
            return Err(invalid(format!(
                "{} x values but {} y values",
                self.x.len(),
                self.y.len()
            )));
        }
        if self.x.len() > MAX_POINTS {
            return Err(invalid(format!(
                "{} points; at most {MAX_POINTS}",
                self.x.len()
            )));
        }
        if self.x.iter().chain(&self.y).any(|v| !v.is_finite()) {
            return Err(invalid("a point is not finite".into()));
        }
        if self.peaks.len() > MAX_PEAKS {
            return Err(invalid(format!("{} peaks; at most {MAX_PEAKS}", self.peaks.len())));
        }
        self.picking
            .validate()
            .map_err(|e| invalid(format!("peak envelope: {e}")))
    }

    /// The x range (low, high).
    #[must_use]
    pub fn x_range(&self) -> Option<(f64, f64)> {
        range(&self.x)
    }

    /// The y range (low, high).
    #[must_use]
    pub fn y_range(&self) -> Option<(f64, f64)> {
        range(&self.y)
    }

    /// The title, else the block name, else the file name.
    #[must_use]
    pub fn display_title(&self) -> String {
        self.title
            .clone()
            .or_else(|| self.name.clone())
            .unwrap_or_else(|| {
                self.path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&self.path)
                    .to_owned()
            })
    }

    /// The points as CSV with the axis labels as header.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out = format!("{},{}\n", csv(&self.x_label), csv(&self.y_label));
        for (x, y) in self.x.iter().zip(&self.y) {
            out.push_str(&format!("{x},{y}\n"));
        }
        out
    }

    /// The peaks as CSV: x, y, prominence.
    #[must_use]
    pub fn peaks_csv(&self) -> String {
        let mut out = format!("{},{},prominence\n", csv(&self.x_label), csv(&self.y_label));
        for p in &self.peaks {
            out.push_str(&format!(
                "{},{},{}\n",
                p.x,
                p.y,
                p.prominence.map(|v| v.to_string()).unwrap_or_default()
            ));
        }
        out
    }
}

fn csv(s: &str) -> String {
    if s.contains([',', '"']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// The (low, high) of `values`.
#[must_use]
pub fn range(values: &[f64]) -> Option<(f64, f64)> {
    let mut it = values.iter().copied().filter(|v| v.is_finite());
    let first = it.next()?;
    Some(it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
}

/// The points of `x`/`y` inside `lo..=hi` reduced to at most two per
/// pixel column (`columns` of them): the minimum and maximum of each
/// column in x order, so peaks survive. Points stay in x order.
#[must_use]
pub fn decimate(x: &[f64], y: &[f64], lo: f64, hi: f64, columns: usize) -> (Vec<f64>, Vec<f64>) {
    let inside: Vec<usize> = (0..x.len().min(y.len()))
        .filter(|&i| x[i] >= lo && x[i] <= hi)
        .collect();
    if inside.len() <= columns.max(1) * 2 || hi <= lo {
        let mut idx = inside;
        idx.sort_by(|&a, &b| x[a].total_cmp(&x[b]));
        return (idx.iter().map(|&i| x[i]).collect(), idx.iter().map(|&i| y[i]).collect());
    }
    let width = (hi - lo) / columns as f64;
    let mut buckets: Vec<Option<(usize, usize)>> = vec![None; columns];
    for &i in &inside {
        let b = (((x[i] - lo) / width) as usize).min(columns - 1);
        buckets[b] = Some(match buckets[b] {
            None => (i, i),
            Some((mn, mx)) => (
                if y[i] < y[mn] { i } else { mn },
                if y[i] > y[mx] { i } else { mx },
            ),
        });
    }
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    for (mn, mx) in buckets.into_iter().flatten() {
        let (a, b) = if x[mn] <= x[mx] { (mn, mx) } else { (mx, mn) };
        xs.push(x[a]);
        ys.push(y[a]);
        if b != a {
            xs.push(x[b]);
            ys.push(y[b]);
        }
    }
    (xs, ys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_decimation_keep_extremes() {
        assert_eq!(range(&[3.0, -1.0, 2.0]), Some((-1.0, 3.0)));
        assert_eq!(range(&[]), None);
        let x: Vec<f64> = (0..1000).map(f64::from).collect();
        let mut y = vec![0.0; 1000];
        y[500] = 10.0;
        y[700] = -5.0;
        let (dx, dy) = decimate(&x, &y, 0.0, 999.0, 50);
        assert!(dx.len() <= 100);
        assert!(dy.contains(&10.0) && dy.contains(&-5.0));
        assert!(dx.windows(2).all(|w| w[0] <= w[1]));
        // Few points: all of them, sorted by x.
        let (sx, _) = decimate(&[3.0, 1.0, 2.0], &[0.0; 3], 0.0, 5.0, 50);
        assert_eq!(sx, [1.0, 2.0, 3.0]);
    }
}
