//! Where the session database lives by default.

use std::ffi::OsString;
use std::path::PathBuf;

/// Directory under the data home that holds crest's files.
const APP_DIR: &str = "crest";
/// File name of the session database.
const DB_FILE: &str = "sessions.db";

/// Default path of the session database: `$XDG_DATA_HOME/crest/sessions.db`,
/// or `~/.local/share/crest/sessions.db` when `XDG_DATA_HOME` is unset.
///
/// The XDG layout is used on every platform (also macOS), as in the rest of
/// crest's files, rather than `dirs::data_dir`, which on macOS points to
/// `~/Library/Application Support`. `None` if there is no home directory.
#[must_use]
pub fn default_db_path() -> Option<PathBuf> {
    db_path_from(std::env::var_os("XDG_DATA_HOME"), dirs::home_dir())
}

/// [`default_db_path`] with the environment passed in. Per the XDG spec, a
/// relative or empty `XDG_DATA_HOME` is ignored.
fn db_path_from(xdg_data_home: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    let data_home = xdg_data_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(".local").join("share")))?;
    Some(data_home.join(APP_DIR).join(DB_FILE))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn uses_xdg_data_home_when_absolute() {
        let path = db_path_from(Some("/data".into()), Some("/home/u".into()));
        assert_eq!(path.as_deref(), Some(Path::new("/data/crest/sessions.db")));
    }

    #[test]
    fn falls_back_to_local_share() {
        let expected = Some(Path::new("/home/u/.local/share/crest/sessions.db"));
        for xdg in [None, Some("".into()), Some("relative/dir".into())] {
            let path = db_path_from(xdg, Some("/home/u".into()));
            assert_eq!(path.as_deref(), expected);
        }
    }

    #[test]
    fn none_without_home_or_xdg() {
        assert_eq!(db_path_from(None, None), None);
    }
}
