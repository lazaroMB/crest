//! The asset source of the app: the SVGs that `gpui_component::IconName`
//! paths (`icons/plus.svg`) name. GPUI loads an `Icon` through the app's
//! asset source, and the default one finds nothing, so an app that registers
//! none paints every icon as empty space (no error).
//!
//! [`install`] is the one place the source is chosen; `app::run_with` and the
//! examples call it, and the tests below load every icon crest draws through
//! [`source`].

use gpui::{Application, AssetSource};

/// The source that serves the icons: gpui-component's default bundle
/// (Lucide), embedded in the binary.
pub fn source() -> impl AssetSource {
    gpui_kit_assets::Assets
}

/// Registers [`source`] on `app`; call it before `run`.
pub fn install(app: Application) -> Application {
    app.with_assets(source())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_component::{IconName, IconNamed as _};

    /// Every icon crest draws, by the name it has in `IconName`.
    fn used() -> [(&'static str, IconName); 6] {
        [
            ("Plus", IconName::Plus),
            ("Search", IconName::Search),
            ("Folder", IconName::Folder),
            ("Close", IconName::Close),
            ("Check", IconName::Check),
            ("Copy", IconName::Copy),
        ]
    }

    #[test]
    fn the_source_serves_every_icon_crest_draws() {
        let source = source();
        for (name, icon) in used() {
            let path = icon.path();
            let bytes = source
                .load(&path)
                .unwrap_or_else(|err| panic!("{name} ({path}): {err}"))
                .unwrap_or_else(|| panic!("{name} ({path}) is not served"));
            let text = std::str::from_utf8(&bytes).expect("svg is utf-8");
            assert!(text.contains("<svg"), "{name} ({path}) is not an svg");
        }
    }

    #[test]
    fn the_default_source_serves_nothing() {
        // What the app had before `install`: the reason the test above exists.
        let none: &dyn AssetSource = &();
        assert!(none.load(&IconName::Plus.path()).unwrap().is_none());
    }

    /// A new `IconName::X` in the sources must be added to `used`, so that
    /// the test above covers it.
    #[test]
    fn every_icon_the_sources_name_is_in_the_table() {
        let known: Vec<&str> = used().iter().map(|(name, _)| *name).collect();
        let mut found = Vec::new();
        let mut dirs = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs")
                    && path.file_name().is_some_and(|name| name != "assets.rs")
                {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for part in text.split("IconName::").skip(1) {
                        let ident: String = part
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        found.push((path.clone(), ident));
                    }
                }
            }
        }
        assert!(!found.is_empty());
        for (path, ident) in found {
            assert!(
                known.contains(&ident.as_str()),
                "IconName::{ident} in {} is not in the assets test table",
                path.display()
            );
        }
    }
}
