//! crest's design system: the shape tokens and the shared components the
//! views and blocks are built from (see `docs/design-system.md`).
//!
//! Colors come from [`crate::theme::token`] (or gpui-component's
//! `cx.theme()`, which the theme feeds); sizes from [`tokens`], so a
//! theme's `shape` table reaches every view built from these.

pub mod button;
pub mod chip;
pub mod header;
pub mod tokens;

pub use button::{danger_button, quiet_button, tool_button};
pub use chip::{ChipKind, chip, filter_chip, readout, toggle_chip};
pub use header::{caption, header, title, toolbar};
pub use tokens::{Radius, Space, TextRole, TokenStyled};

/// Checks over the crate's sources that keep views on the design system:
/// colors come from the theme, and block headers and toolbar buttons from
/// this module.
#[cfg(test)]
mod lint {
    use std::fs;
    use std::path::{Path, PathBuf};

    use regex::Regex;

    /// Marks a deliberate fixed color, on its line or the one above.
    const FIXED_COLOR: &str = "design: fixed color";

    fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read src") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// Every source file outside `kit/` and `theme.rs`, without its test
    /// module (`(path, code)`).
    fn checked() -> Vec<(String, String)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        sources(&root, &mut files);
        files
            .into_iter()
            .filter_map(|path| {
                let name = path
                    .strip_prefix(&root)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                if name.starts_with("kit/")
                    || name == "theme.rs"
                    || name.ends_with("tests.rs")
                    || name.contains("/tests/")
                {
                    return None;
                }
                let text = fs::read_to_string(&path).ok()?;
                let code = text
                    .split("#[cfg(test)]\nmod tests")
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                Some((name, code))
            })
            .collect()
    }

    fn line_of(code: &str, at: usize) -> usize {
        code[..at].matches('\n').count() + 1
    }

    #[test]
    fn colors_come_from_the_theme() {
        let literal = Regex::new(r"\b(rgba?\(0x[0-9a-fA-F]|hsla\(\s*[0-9.])").unwrap();
        let mut found = Vec::new();
        for (name, code) in checked() {
            let lines: Vec<&str> = code.lines().collect();
            for m in literal.find_iter(&code) {
                let line = line_of(&code, m.start());
                let marked = lines[line - 1].contains(FIXED_COLOR)
                    || (line >= 2 && lines[line - 2].contains(FIXED_COLOR));
                if !marked {
                    found.push(format!("{name}:{line}"));
                }
            }
        }
        assert!(
            found.is_empty(),
            "color literals outside the theme (use crate::theme::token, or mark a \
             deliberate one with `// {FIXED_COLOR}`):\n{}",
            found.join("\n")
        );
    }

    #[test]
    fn headers_and_buttons_come_from_the_kit() {
        let patterns = [
            (
                Regex::new(
                    r"\.whitespace_nowrap\(\)\s*\.text_ellipsis\(\)\s*\.font_weight\(FontWeight::SEMIBOLD\)",
                )
                .unwrap(),
                "a header title (use kit::title)",
            ),
            (
                Regex::new(
                    r#"\.bg\(token\(cx, "accent\.bg"\)\)|\.hover\(move \|style\| style\.bg\(accent_hover\)\)"#,
                )
                .unwrap(),
                "an accent toolbar button (use kit::tool_button)",
            ),
        ];
        let mut found = Vec::new();
        for (name, code) in checked() {
            for (pattern, what) in &patterns {
                for m in pattern.find_iter(&code) {
                    found.push(format!("{name}:{}: {what}", line_of(&code, m.start())));
                }
            }
        }
        assert!(
            found.is_empty(),
            "hand-made kit parts:\n{}",
            found.join("\n")
        );
    }
}
