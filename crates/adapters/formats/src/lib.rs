//! [`FormatsPort`] adapter: developer file formats behind the report
//! blocks.
//!
//! - JUnit XML test reports ([`junit`], quick-xml);
//! - SARIF 2.1.0 static-analysis logs ([`sarif`], serde);
//! - WaveDrom WaveJSON timing diagrams to SVG ([`wave`], `wavedrom`);
//! - dataset previews of CSV, TSV, JSON lines and Parquet ([`dataset`],
//!   `parquet` without arrow);
//! - 2D molecules from SMILES ([`molecule`], `chematic`).

pub mod dataset;
pub mod junit;
pub mod molecule;
pub mod sarif;
pub mod wave;

use application::ports::formats::{DatasetRequest, FormatError, FormatsPort};
use domain::dataset::DatasetSpec;
use domain::report::{FindingsSpec, TestReportSpec};
use domain::science::MoleculeSpec;

/// The formats adapter. Stateless.
#[derive(Debug, Clone, Copy, Default)]
pub struct Formats;

impl Formats {
    /// The adapter.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl FormatsPort for Formats {
    fn parse_test_report(&self, bytes: &[u8], path: &str) -> Result<TestReportSpec, FormatError> {
        junit::parse(bytes, path).map_err(FormatError::Invalid)
    }

    fn parse_sarif(&self, bytes: &[u8], path: &str) -> Result<FindingsSpec, FormatError> {
        sarif::parse(bytes, path).map_err(FormatError::Invalid)
    }

    fn render_wavedrom(&self, source: &str) -> Result<String, FormatError> {
        wave::render(source).map_err(FormatError::Invalid)
    }

    fn preview_dataset(
        &self,
        bytes: &[u8],
        request: &DatasetRequest,
    ) -> Result<DatasetSpec, FormatError> {
        dataset::preview(bytes, request).map_err(FormatError::Invalid)
    }

    fn depict_molecule(&self, smiles: &str, claimed_formula: Option<&str>) -> MoleculeSpec {
        molecule::depict(smiles, claimed_formula)
    }
}
