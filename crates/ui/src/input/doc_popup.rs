//! The documents popup of the prompt: after `/doc find `, `/doc open `,
//! `/doc use ` and a bare `/doc <text>` it lists the documents whose title
//! matches, with the title of their chat and what choosing does; after
//! `/doc export ` it lists the formats. `Enter` runs the command with the
//! document's exact title, `Tab` completes the line to it, `Esc` hides the
//! popup. The session hands the input the documents
//! ([`InputView::set_docs`](super::InputView::set_docs)) and refreshes
//! them when one changes.

use application_core::keymap::Key;
use gpui::{AnyElement, ParentElement as _, SharedString, Styled as _, div};
use gpui_component::h_flex;

use super::completion::{self, Look, Nav, Paint, Popup, PopupKey, nav_key};

/// A document the popup can offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocChoice {
    /// Its id, as printed.
    pub id: String,
    /// Its title.
    pub title: String,
    /// The title of its chat.
    pub chat: String,
}

/// A row of the popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocRow {
    /// A document's title, or a format's name.
    pub title: String,
    /// The chat of a document, or what a format does.
    pub detail: String,
    /// What choosing the row does.
    pub note: String,
    /// The command `Enter` runs, without the slash.
    pub run: String,
    /// The line `Tab` puts in the prompt.
    pub complete: String,
}

/// What the popup shows: the rows, or why there is none (an inert row).
pub type DocPopup = Popup<DocRow>;

/// What the line asks the popup to list.
#[derive(Debug, PartialEq, Eq)]
enum Query<'a> {
    /// The documents whose title holds `text`, to run `verb` on.
    Docs { verb: &'static str, text: &'a str },
    /// The export formats that start with the word.
    Export(&'a str),
}

/// The subcommands of `/doc`: a bare word that is one (or the start of
/// one) is not a title yet.
const SUBCOMMANDS: [&str; 10] = [
    "new", "find", "open", "use", "add", "split", "only", "close", "source", "export",
];

/// The export formats: the word, its name and what it does.
const FORMATS: [(&str, &str, &str); 4] = [
    ("md", "Markdown", "One .md file with the images inside"),
    ("html", "Web page", "A .html file with the print style"),
    ("pdf", "PDF", "The browser's print dialog, to save a PDF"),
    ("copy", "Copy", "The markdown on the clipboard"),
];

/// What `text` asks of the popup, if it is a `/doc` line that takes a
/// document or a format.
fn query(text: &str) -> Option<Query<'_>> {
    let head = text.get(..5)?;
    if !head.eq_ignore_ascii_case("/doc ") {
        return None;
    }
    let rest = text.get(5..)?.trim_start();
    let (word, tail) = match rest.split_once(char::is_whitespace) {
        Some((word, tail)) => (word, Some(tail.trim_start())),
        None => (rest, None),
    };
    let lower = word.to_lowercase();
    match (lower.as_str(), tail) {
        ("find" | "open", Some(text)) => Some(Query::Docs { verb: "open", text }),
        ("use", Some(text)) => Some(Query::Docs { verb: "use", text }),
        ("export", Some(word)) => Some(Query::Export(word)),
        ("", None) => Some(Query::Docs {
            verb: "open",
            text: "",
        }),
        // Still typing a subcommand.
        (word, None) if SUBCOMMANDS.iter().any(|sub| sub.starts_with(word)) => None,
        // A subcommand with its own arguments (`new Plan`).
        (word, Some(_)) if SUBCOMMANDS.contains(&word) => None,
        _ => Some(Query::Docs {
            verb: "open",
            text: rest,
        }),
    }
}

/// What the popup shows for `text` given the documents (newest first), if
/// it is a `/doc` line that lists them.
#[must_use]
pub fn doc_popup(docs: &[DocChoice], text: &str) -> Option<DocPopup> {
    Some(match query(text)? {
        Query::Docs { verb, text } => docs_popup(docs, verb, text),
        Query::Export(word) => export_popup(word),
    })
}

fn docs_popup(docs: &[DocChoice], verb: &str, text: &str) -> DocPopup {
    let wanted = text.trim().to_lowercase();
    let rows: Vec<DocRow> = docs
        .iter()
        .filter(|doc| wanted.is_empty() || doc.title.to_lowercase().contains(&wanted))
        .map(|doc| {
            // Two documents with one title are told apart by id.
            let twin = docs
                .iter()
                .filter(|other| other.title.trim().eq_ignore_ascii_case(doc.title.trim()))
                .count()
                > 1;
            let argument = if twin { &doc.id } else { &doc.title };
            DocRow {
                title: doc.title.clone(),
                detail: doc.chat.clone(),
                note: if verb == "use" {
                    "Reference on the next message"
                } else {
                    "Open beside the chat"
                }
                .to_owned(),
                run: format!("doc {verb} {argument}"),
                complete: format!("/doc {verb} {argument}"),
            }
        })
        .collect();
    if rows.is_empty() {
        let why = if wanted.is_empty() {
            "No documents yet"
        } else {
            "No document matches"
        };
        return DocPopup::Message(why.to_owned());
    }
    DocPopup::Entries(rows)
}

fn export_popup(word: &str) -> DocPopup {
    let wanted = word.trim().to_lowercase();
    let rows: Vec<DocRow> = FORMATS
        .iter()
        .filter(|(id, _, _)| id.starts_with(&wanted))
        .map(|(id, name, what)| DocRow {
            title: (*name).to_owned(),
            detail: (*what).to_owned(),
            note: format!("/doc export {id}"),
            run: format!("doc export {id}"),
            complete: format!("/doc export {id}"),
        })
        .collect();
    if rows.is_empty() {
        return DocPopup::Message("md, html, pdf or copy".to_owned());
    }
    DocPopup::Entries(rows)
}

/// What `key` does in `popup` with row `selected` chosen: `Enter` runs the
/// command and `Tab` completes the line to it.
#[must_use]
pub fn doc_key(key: &Key, popup: &DocPopup, selected: usize) -> PopupKey {
    match nav_key(key, popup, selected) {
        Nav::Select(ix) => PopupKey::Select(ix),
        Nav::Hide => PopupKey::Hide,
        Nav::Complete(row) => PopupKey::SetText(row.complete.clone()),
        Nav::Accept(row) => PopupKey::Run(row.run.clone()),
        Nav::Ignored => PopupKey::Ignored,
    }
}

/// Draws `popup` from the window's first row on, the chosen row lit.
pub(super) fn render(popup: DocPopup, paint: &Paint) -> AnyElement {
    let theme = paint.theme;
    completion::render(
        &Look {
            id: "doc-palette",
            row_id: "doc",
            keys: "↑↓ choose · Enter run · Tab complete · Esc hide",
        },
        popup,
        paint,
        |row| {
            h_flex()
                .gap_3()
                .px_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.foreground)
                        .child(SharedString::from(row.title)),
                )
                .child(
                    div()
                        .flex_none()
                        .max_w_48()
                        .truncate()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(row.detail)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(row.note)),
                )
        },
    )
}

#[cfg(test)]
mod tests {
    use application_core::keymap::KeyCode;

    use super::*;

    fn doc(id: &str, title: &str, chat: &str) -> DocChoice {
        DocChoice {
            id: id.into(),
            title: title.into(),
            chat: chat.into(),
        }
    }

    fn docs() -> Vec<DocChoice> {
        vec![
            doc("a", "Release notes", "Ship it"),
            doc("b", "Plan", "Roadmap"),
            doc("c", "Release plan", "Roadmap"),
        ]
    }

    fn entries(popup: Option<DocPopup>) -> Vec<DocRow> {
        match popup {
            Some(DocPopup::Entries(rows)) => rows,
            other => panic!("expected rows, got {other:?}"),
        }
    }

    #[test]
    fn find_open_use_and_a_bare_word_list_the_matching_documents() {
        let rows = entries(doc_popup(&docs(), "/doc find rel"));
        let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["Release notes", "Release plan"]);
        assert_eq!(rows[0].detail, "Ship it");
        assert_eq!(rows[0].run, "doc open Release notes");
        let used = entries(doc_popup(&docs(), "/doc use plan"));
        assert_eq!(used[0].run, "doc use Plan");
        assert_eq!(used[0].complete, "/doc use Plan");
        let bare = entries(doc_popup(&docs(), "/doc rel"));
        assert_eq!(bare.len(), 2);
        assert_eq!(bare[1].run, "doc open Release plan");
        assert_eq!(entries(doc_popup(&docs(), "/doc open ")).len(), 3);
        assert_eq!(entries(doc_popup(&docs(), "/doc ")).len(), 3);
    }

    #[test]
    fn a_title_two_documents_share_is_told_apart_by_id() {
        let docs = vec![doc("a", "Notes", "One"), doc("b", "notes", "Two")];
        let rows = entries(doc_popup(&docs, "/doc open no"));
        assert_eq!(rows[0].run, "doc open a");
        assert_eq!(rows[1].run, "doc open b");
    }

    #[test]
    fn no_match_is_an_inert_row_and_other_lines_list_nothing() {
        assert_eq!(
            doc_popup(&docs(), "/doc find zzz"),
            Some(DocPopup::Message("No document matches".into()))
        );
        assert_eq!(
            doc_popup(&[], "/doc find "),
            Some(DocPopup::Message("No documents yet".into()))
        );
        for text in [
            "/doc",
            "/doc f",
            "/doc find",
            "/doc ne",
            "/doc new Plan",
            "/doc split ",
            "/docs ",
            "hello",
            "/model ",
        ] {
            assert_eq!(doc_popup(&docs(), text), None, "{text}");
        }
    }

    #[test]
    fn export_lists_the_four_formats() {
        let rows = entries(doc_popup(&docs(), "/doc export "));
        let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["Markdown", "Web page", "PDF", "Copy"]);
        assert_eq!(rows[0].run, "doc export md");
        let html = entries(doc_popup(&docs(), "/doc export h"));
        assert_eq!(html.len(), 1);
        assert_eq!(
            doc_popup(&docs(), "/doc export zip"),
            Some(DocPopup::Message("md, html, pdf or copy".into()))
        );
    }

    #[test]
    fn enter_runs_tab_completes_and_an_inert_row_ignores_both() {
        let popup = doc_popup(&docs(), "/doc find rel").unwrap();
        let key = |code| Key::plain(code);
        assert_eq!(
            doc_key(&key(KeyCode::Enter), &popup, 1),
            PopupKey::Run("doc open Release plan".into())
        );
        assert_eq!(
            doc_key(&key(KeyCode::Tab), &popup, 0),
            PopupKey::SetText("/doc open Release notes".into())
        );
        assert_eq!(doc_key(&key(KeyCode::Down), &popup, 1), PopupKey::Select(0));
        let none = doc_popup(&docs(), "/doc find zzz").unwrap();
        assert_eq!(doc_key(&key(KeyCode::Enter), &none, 0), PopupKey::Ignored);
        assert_eq!(doc_key(&key(KeyCode::Esc), &none, 0), PopupKey::Hide);
    }
}
