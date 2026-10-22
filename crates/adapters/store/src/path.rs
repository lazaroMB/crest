//! Where the session database lives by default.

use std::path::PathBuf;

/// File name of the session database.
const DB_FILE: &str = "sessions.db";

/// Default path of the session database, in crest's data directory:
/// `$XDG_DATA_HOME/crest/sessions.db`, else
/// `~/.local/share/crest/sessions.db` (also on macOS, as the rest of
/// crest's files) or `%LOCALAPPDATA%\crest\sessions.db` on Windows. See
/// [`platform::dirs::data_dir`]. `None` if there is no home directory.
#[must_use]
pub fn default_db_path() -> Option<PathBuf> {
    platform::dirs::data_dir().map(|dir| dir.join(DB_FILE))
}
