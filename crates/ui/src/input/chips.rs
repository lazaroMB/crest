//! The documents a prompt names, as chips above it: a reference to a
//! document (`/doc use`, the "+" menu) and a passage selected on the page
//! ("Ask agent"). They go to the agent as context with the next message,
//! which shows `↳ <title>` under it. A click on `×`, or `Backspace` in an
//! empty prompt, removes the last one.

use domain::doc::DocId;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use gpui_component::{ActiveTheme as _, Icon, Sizable as _, h_flex};

use super::InputView;

/// The longest label of a chip, in pixels.
const LABEL_MAX_PX: f32 = 260.0;

/// The most characters of a passage a chip shows.
const PASSAGE_CHARS: usize = 40;

/// A document the next message names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocChip {
    /// The document to change.
    Reference {
        /// Its id.
        doc_id: DocId,
        /// Its title.
        title: String,
    },
    /// A passage of a document the reply should replace.
    Selection {
        /// The document's id.
        doc_id: DocId,
        /// Its title.
        title: String,
        /// The selected text.
        passage: String,
    },
}

impl DocChip {
    /// The document's title.
    #[must_use]
    pub fn title(&self) -> &str {
        match self {
            Self::Reference { title, .. } | Self::Selection { title, .. } => title,
        }
    }

    /// The document's id.
    #[must_use]
    pub fn doc_id(&self) -> DocId {
        match self {
            Self::Reference { doc_id, .. } | Self::Selection { doc_id, .. } => *doc_id,
        }
    }

    /// The text that goes before the prompt the agent gets.
    #[must_use]
    pub fn context(&self) -> String {
        match self {
            Self::Reference { doc_id, title } => format!(
                "The document to change is “{title}” (doc id {doc_id}). \
                 Use doc_edit or doc_write on it."
            ),
            Self::Selection {
                doc_id,
                title,
                passage,
            } => format!(
                "Selected passage of “{title}” (doc id {doc_id}):\n{passage}\n\
                 The reply should replace that passage with doc_edit."
            ),
        }
    }

    /// What the chip says: the title, or the start of the passage.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Reference { title, .. } => title.clone(),
            Self::Selection { passage, .. } => {
                let flat: String = passage.split_whitespace().collect::<Vec<_>>().join(" ");
                let mut shown: String = flat.chars().take(PASSAGE_CHARS).collect();
                if flat.chars().count() > PASSAGE_CHARS {
                    shown.push('…');
                }
                format!("“{shown}”")
            }
        }
    }
}

impl InputView {
    /// Puts `chip` on the prompt. A document is referenced once: naming it
    /// again moves its chip to the end.
    pub fn add_doc_chip(&mut self, chip: DocChip, cx: &mut Context<Self>) {
        if matches!(chip, DocChip::Reference { .. }) {
            self.chips.retain(|known| {
                !(matches!(known, DocChip::Reference { .. }) && known.doc_id() == chip.doc_id())
            });
        }
        self.chips.push(chip);
        cx.notify();
    }

    /// The chips on the prompt, in the order they were added.
    #[must_use]
    pub fn doc_chips(&self) -> &[DocChip] {
        &self.chips
    }

    /// Removes the chip at `ix`.
    pub fn remove_doc_chip(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.chips.len() {
            self.chips.remove(ix);
            cx.notify();
        }
    }

    /// Takes the chips for sending: the prompt is empty of them from now on.
    pub fn take_doc_chips(&mut self) -> Vec<DocChip> {
        std::mem::take(&mut self.chips)
    }

    /// Puts sent chips back (a queued prompt taken back to edit).
    pub fn restore_doc_chips(&mut self, chips: Vec<DocChip>, cx: &mut Context<Self>) {
        for chip in chips {
            self.add_doc_chip(chip, cx);
        }
    }

    /// The row of chips above the prompt, if there are any.
    pub(super) fn render_chips(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.chips.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let (border, fill, muted, text) = (
            theme.border,
            theme.muted,
            theme.muted_foreground,
            theme.foreground,
        );
        let chips: Vec<AnyElement> = self
            .chips
            .iter()
            .enumerate()
            .map(|(ix, chip)| {
                let remove = cx.listener(move |input, _, _, cx| input.remove_doc_chip(ix, cx));
                h_flex()
                    .id(("doc-chip", ix))
                    .debug_selector(move || format!("doc-chip-{ix}"))
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .border_color(border)
                    .bg(fill)
                    .text_xs()
                    .child(
                        Icon::new(gpui_kit_assets::IconName::FileText)
                            .xsmall()
                            .text_color(muted),
                    )
                    .child(
                        div()
                            .max_w(px(LABEL_MAX_PX))
                            .truncate()
                            .text_color(text)
                            .child(SharedString::from(chip.label())),
                    )
                    .child(
                        div()
                            .id(("doc-chip-remove", ix))
                            .debug_selector(move || format!("doc-chip-remove-{ix}"))
                            .px_1()
                            .cursor_pointer()
                            .text_color(muted)
                            .child("×")
                            .on_click(remove),
                    )
                    .into_any_element()
            })
            .collect();
        Some(
            h_flex()
                .id("doc-chips")
                .debug_selector(|| "doc-chips".to_owned())
                .mb_1()
                .gap_1()
                .flex_wrap()
                .children(chips)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_and_a_selection_say_what_the_agent_should_do() {
        let id = DocId::new();
        let reference = DocChip::Reference {
            doc_id: id,
            title: "Release notes".into(),
        };
        assert_eq!(
            reference.context(),
            format!(
                "The document to change is “Release notes” (doc id {id}). \
                 Use doc_edit or doc_write on it."
            )
        );
        let selection = DocChip::Selection {
            doc_id: id,
            title: "Release notes".into(),
            passage: "first\nsecond".into(),
        };
        assert_eq!(
            selection.context(),
            format!(
                "Selected passage of “Release notes” (doc id {id}):\nfirst\nsecond\n\
                 The reply should replace that passage with doc_edit."
            )
        );
        assert_eq!(selection.title(), "Release notes");
        assert_eq!(selection.label(), "“first second”");
    }

    #[test]
    fn a_long_passage_is_cut_in_its_chip() {
        let chip = DocChip::Selection {
            doc_id: DocId::new(),
            title: "T".into(),
            passage: "word ".repeat(30),
        };
        assert!(chip.label().ends_with("…”"), "{}", chip.label());
    }
}
