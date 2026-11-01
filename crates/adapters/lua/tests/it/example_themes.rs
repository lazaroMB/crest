//! The example themes of `examples/themes` load with the real loader,
//! resolve without a single warning and keep their text readable.

use std::path::PathBuf;
use std::sync::Arc;

use adapter_lua::LuaThemeSource;
use application::ports::theme::ThemeSourcePort as _;
use application::theme::{ThemeCatalog, ThemeOrigin};

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/themes")
}

#[test]
fn every_example_theme_resolves_cleanly_and_reads_well() {
    let source = LuaThemeSource::new(examples());
    let names = source.list();
    assert!(names.len() >= 14, "{names:?}");
    let catalog = ThemeCatalog::new().with_files(Arc::new(source));
    for name in &names {
        let resolved = catalog
            .resolve(name, None)
            .unwrap_or_else(|err| panic!("{name}: {err}"));
        assert!(
            resolved.warnings.is_empty(),
            "{name}: {:#?}",
            resolved.warnings
        );
        let theme = resolved.theme;
        assert_eq!(&theme.name, name);
        let (bg, fg) = (theme.color("background"), theme.color("foreground"));
        let ratio = fg.contrast_ratio(bg);
        assert!(
            ratio >= 4.5,
            "{name}: foreground on background is {ratio:.2}"
        );
        let accent = theme.color("accent.bg");
        let on_accent = theme.color("accent.fg").contrast_ratio(accent);
        assert!(
            on_accent >= 3.0,
            "{name}: text on the accent is {on_accent:.2}"
        );
    }
    let file_themes = catalog
        .entries()
        .into_iter()
        .filter(|entry| entry.origin == ThemeOrigin::File)
        .count();
    assert_eq!(file_themes, names.len());
}

#[test]
fn the_template_is_a_valid_theme_and_is_not_listed() {
    let source = LuaThemeSource::new(examples());
    assert!(!source.list().iter().any(|name| name.starts_with('_')));
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        examples().join("_template.lua"),
        dir.path().join("template.lua"),
    )
    .unwrap();
    let catalog = ThemeCatalog::new().with_files(Arc::new(LuaThemeSource::new(dir.path())));
    let resolved = catalog.resolve("template", None).unwrap();
    assert!(resolved.warnings.is_empty(), "{:#?}", resolved.warnings);
}
