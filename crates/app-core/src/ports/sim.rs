//! Exporting a simulated solution so it can be shared without crest.

use domain::sysmodel::Board;

/// Why a board could not be exported.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("export: {0}")]
pub struct SimExportError(pub String);

/// Turns a simulated [`Board`] into a shareable file.
pub trait SimExport: Send + Sync {
    /// A self-contained HTML page (no network access) that replays the
    /// recorded runs of `board` with the look of the window's board.
    ///
    /// # Errors
    ///
    /// [`SimExportError`] when the page cannot be built.
    fn board_html(&self, board: &Board) -> Result<String, SimExportError>;
}
