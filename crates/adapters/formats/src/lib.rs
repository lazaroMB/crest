//! [`FormatsPort`] adapter: developer file formats behind the report
//! blocks.
//!
//! - JUnit XML test reports ([`junit`], quick-xml);
//! - SARIF 2.1.0 static-analysis logs ([`sarif`], serde);
//! - WaveDrom WaveJSON timing diagrams to SVG ([`wave`], `wavedrom`).

pub mod junit;
pub mod sarif;
pub mod wave;

use application::ports::formats::{FormatError, FormatsPort};
use domain::report::{FindingsSpec, TestReportSpec};

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
}
