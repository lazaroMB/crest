//! Opens the crest main window with the placeholder layout.
//!
//! `cargo run -p ui --example window [workspace-label]`; quit with Cmd/Ctrl+Q.

fn main() {
    let workspace_label = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "example".to_owned());
    let opts = ui::app::UiOptions {
        workspace_label,
        ..Default::default()
    };
    if let Err(err) = ui::app::run(opts) {
        eprintln!("crest: {err}");
        std::process::exit(1);
    }
}
