//! `app.opt.window`: how the main window opens.
//!
//! ```lua
//! app.opt.window = "maximized"                                   -- or "fullscreen", "windowed"
//! app.opt.window = { mode = "windowed", width = 1440, height = 900 }
//! ```
//!
//! Unset, the window opens in fullscreen (maximized on Windows, where
//! fullscreen hides the taskbar). It is read from the global config only:
//! the window opens before a project, and a cloned repository should not
//! change it.

use serde_json::{Map, Value};

use crate::ports::plugin::AppOptions;

/// Key of the window option in `app.opt`.
pub const WINDOW_OPTION: &str = "window";

/// Size of a windowed window when the option gives none, in pixels.
pub const DEFAULT_SIZE: (u32, u32) = (1280, 800);

/// Smallest width or height accepted, in pixels.
pub const MIN_SIZE: u32 = 400;

const MODES: [&str; 3] = ["fullscreen", "maximized", "windowed"];
const FIELDS: [&str; 3] = ["mode", "width", "height"];

/// How the main window opens. The size is the windowed size, also the
/// one it returns to when it leaves fullscreen or maximized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowMode {
    /// Covering the whole screen, without the system bars.
    Fullscreen {
        /// Size when it leaves fullscreen.
        width: u32,
        /// Size when it leaves fullscreen.
        height: u32,
    },
    /// Maximized, with the system bars.
    Maximized {
        /// Size when it is restored.
        width: u32,
        /// Size when it is restored.
        height: u32,
    },
    /// A window of this size, centered.
    Windowed {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
}

impl WindowMode {
    /// The mode when the option is not set: fullscreen, maximized on
    /// Windows.
    #[must_use]
    pub const fn platform_default() -> Self {
        let (width, height) = DEFAULT_SIZE;
        if cfg!(windows) {
            Self::Maximized { width, height }
        } else {
            Self::Fullscreen { width, height }
        }
    }

    /// The windowed size.
    #[must_use]
    pub const fn size(self) -> (u32, u32) {
        match self {
            Self::Fullscreen { width, height }
            | Self::Maximized { width, height }
            | Self::Windowed { width, height } => (width, height),
        }
    }
}

impl Default for WindowMode {
    fn default() -> Self {
        Self::platform_default()
    }
}

/// A malformed `app.opt.window`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WindowConfigError {
    /// Neither a string nor a table.
    #[error("app.opt.{WINDOW_OPTION} must be a mode name or a table")]
    WrongType,
    /// A mode crest does not know.
    #[error("app.opt.{WINDOW_OPTION}: unknown mode `{0}` (use {modes})", modes = MODES.join(", "))]
    UnknownMode(String),
    /// A field crest does not know.
    #[error("app.opt.{WINDOW_OPTION}: unknown field `{0}` (known: {known})", known = FIELDS.join(", "))]
    UnknownField(String),
    /// A width or height that is not a whole number of pixels from
    /// [`MIN_SIZE`].
    #[error("app.opt.{WINDOW_OPTION}.{0} must be a whole number of pixels, at least {MIN_SIZE}")]
    BadSize(&'static str),
}

/// The configured mode, or `None` when `app.opt.window` is not set.
///
/// # Errors
///
/// [`WindowConfigError`] when it is malformed.
pub fn from_options(options: &AppOptions) -> Result<Option<WindowMode>, WindowConfigError> {
    match options.extra.get(WINDOW_OPTION) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => from_json(value).map(Some),
    }
}

/// Parses `app.opt.window`: a mode name, or `{ mode, width, height }`.
///
/// # Errors
///
/// [`WindowConfigError`] when it is malformed.
pub fn from_json(value: &Value) -> Result<WindowMode, WindowConfigError> {
    let empty = Map::new();
    let table = match value {
        Value::String(mode) => return mode_named(mode, DEFAULT_SIZE),
        Value::Object(table) => table,
        Value::Array(items) if items.is_empty() => &empty,
        _ => return Err(WindowConfigError::WrongType),
    };
    if let Some(unknown) = table.keys().find(|key| !FIELDS.contains(&key.as_str())) {
        return Err(WindowConfigError::UnknownField(unknown.clone()));
    }
    let width = size(table, "width", DEFAULT_SIZE.0)?;
    let height = size(table, "height", DEFAULT_SIZE.1)?;
    match table.get("mode") {
        None => Ok(WindowMode::Windowed { width, height }),
        Some(Value::String(mode)) => mode_named(mode, (width, height)),
        Some(_) => Err(WindowConfigError::WrongType),
    }
}

fn mode_named(name: &str, (width, height): (u32, u32)) -> Result<WindowMode, WindowConfigError> {
    match name.trim().to_ascii_lowercase().as_str() {
        "fullscreen" => Ok(WindowMode::Fullscreen { width, height }),
        "maximized" | "maximised" => Ok(WindowMode::Maximized { width, height }),
        "windowed" | "normal" => Ok(WindowMode::Windowed { width, height }),
        other => Err(WindowConfigError::UnknownMode(other.to_owned())),
    }
}

fn size(
    table: &Map<String, Value>,
    field: &'static str,
    default: u32,
) -> Result<u32, WindowConfigError> {
    match table.get(field) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .or_else(|| {
                // Lua numbers may arrive as floats (`1440.0`).
                value
                    .as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                    // Whole and non-negative, checked above.
                    .map(|f| {
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        let whole = f as u64;
                        whole
                    })
            })
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n >= MIN_SIZE)
            .ok_or(WindowConfigError::BadSize(field)),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn mode_names() {
        assert_eq!(
            from_json(&json!("maximized")),
            Ok(WindowMode::Maximized {
                width: 1280,
                height: 800
            })
        );
        assert_eq!(
            from_json(&json!(" Fullscreen ")),
            Ok(WindowMode::Fullscreen {
                width: 1280,
                height: 800
            })
        );
        assert_eq!(
            from_json(&json!("normal")),
            Ok(WindowMode::Windowed {
                width: 1280,
                height: 800
            })
        );
        assert_eq!(
            from_json(&json!("kiosk")),
            Err(WindowConfigError::UnknownMode("kiosk".into()))
        );
    }

    #[test]
    fn tables() {
        assert_eq!(
            from_json(&json!({ "mode": "windowed", "width": 1440, "height": 900.0 })),
            Ok(WindowMode::Windowed {
                width: 1440,
                height: 900
            })
        );
        // A size alone means windowed.
        assert_eq!(
            from_json(&json!({ "width": 1000 })),
            Ok(WindowMode::Windowed {
                width: 1000,
                height: 800
            })
        );
        assert_eq!(
            from_json(&json!({ "mode": "maximized", "height": 700 })),
            Ok(WindowMode::Maximized {
                width: 1280,
                height: 700
            })
        );
    }

    #[test]
    fn errors() {
        assert_eq!(from_json(&json!(3)), Err(WindowConfigError::WrongType));
        assert_eq!(
            from_json(&json!({ "widht": 3 })),
            Err(WindowConfigError::UnknownField("widht".into()))
        );
        assert_eq!(
            from_json(&json!({ "width": 100 })),
            Err(WindowConfigError::BadSize("width"))
        );
        assert_eq!(
            from_json(&json!({ "height": 900.5 })),
            Err(WindowConfigError::BadSize("height"))
        );
        assert!(
            WindowConfigError::BadSize("width")
                .to_string()
                .starts_with("app.opt.window.width")
        );
    }

    #[test]
    fn unset_is_none() {
        assert_eq!(from_options(&AppOptions::default()), Ok(None));
    }
}
