//! The menu inside the chat: the user's questions, in order. The question
//! of the block that is selected is the blue row.

use domain::block::BlockKind;

use super::TimelineEntry;

/// How long a heading or a row may be before it ends in an ellipsis.
const LABEL_LIMIT: usize = 42;

/// One numbered row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    /// Block the row scrolls to.
    pub block: usize,
    /// Number within its heading, starting at 1.
    pub number: usize,
    /// The row text.
    pub label: String,
    /// The block that is selected.
    pub active: bool,
}

/// The rows of the menu. The title stays empty: only questions are listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineGroup {
    /// Unused while the menu lists questions with no headings.
    pub title: String,
    /// Block the group's mark scrolls to when it has no rows.
    pub anchor: usize,
    /// The user's questions, in order.
    pub items: Vec<OutlineItem>,
}

/// One mark on the line beside the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutlineMark {
    /// Block the mark scrolls to.
    pub block: usize,
    /// The block that is selected.
    pub active: bool,
}

/// The menu for the blocks on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outline {
    /// Headings, in timeline order.
    pub groups: Vec<OutlineGroup>,
}

impl Outline {
    /// Whether the chat has nothing to list.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// One mark per row. A heading with no rows yet still leaves a mark,
    /// aimed at the heading itself.
    pub fn marks(&self) -> Vec<OutlineMark> {
        let mut marks = Vec::new();
        for group in &self.groups {
            if group.items.is_empty() {
                marks.push(OutlineMark {
                    block: group.anchor,
                    active: false,
                });
            } else {
                marks.extend(group.items.iter().map(|item| OutlineMark {
                    block: item.block,
                    active: item.active,
                }));
            }
        }
        marks
    }
}

/// The menu of `entries`: only the user's questions. `selected` is the
/// block index that paints its question blue, including the reply after it.
pub fn chat_outline(entries: &[TimelineEntry], selected: Option<usize>) -> Outline {
    let questions: Vec<(usize, String)> = entries
        .iter()
        .enumerate()
        .filter_map(|(ix, entry)| prompt_title(entry).map(|title| (ix, title)))
        .collect();
    if questions.is_empty() {
        return Outline { groups: Vec::new() };
    }
    let items = questions
        .iter()
        .enumerate()
        .map(|(n, (ix, title))| {
            let end = questions.get(n + 1).map(|(next, _)| *next);
            OutlineItem {
                block: *ix,
                number: n + 1,
                label: title.clone(),
                active: selected.is_some_and(|sel| *ix <= sel && end.is_none_or(|end| sel < end)),
            }
        })
        .collect();
    let anchor = questions[0].0;
    Outline {
        groups: vec![OutlineGroup {
            title: String::new(),
            anchor,
            items,
        }],
    }
}

/// The prompt's text, when `entry` is a user prompt.
fn prompt_title(entry: &TimelineEntry) -> Option<String> {
    let BlockKind::Text { markdown } = &entry.block.kind else {
        return None;
    };
    if markdown.trim().is_empty() || !markdown.lines().all(|line| line.starts_with('>')) {
        return None;
    }
    let text = markdown
        .lines()
        .map(|line| line.trim().trim_start_matches('>').trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let title = shorten(&text);
    Some(if title.is_empty() {
        "Chat".into()
    } else {
        title
    })
}

/// `text` on one line, cut to [`LABEL_LIMIT`] characters.
fn shorten(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = text.chars();
    let short: String = chars.by_ref().take(LABEL_LIMIT).collect();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

#[cfg(test)]
mod tests {
    use domain::block::Block;

    use super::*;

    fn entry(kind: BlockKind) -> TimelineEntry {
        TimelineEntry::new(Block::new(kind))
    }

    fn prompt(text: &str) -> TimelineEntry {
        entry(BlockKind::Text {
            markdown: format!("> {text}"),
        })
    }

    fn text(markdown: &str) -> TimelineEntry {
        entry(BlockKind::Text {
            markdown: markdown.into(),
        })
    }

    #[test]
    fn only_the_users_questions_are_listed() {
        let entries = vec![
            prompt("hola"),
            text("The user said hola"),
            text("¡Hola! ¿En qué te puedo ayudar?"),
            prompt("podrias abrir un proyecto?"),
            text("tree: ."),
        ];
        let outline = chat_outline(&entries, Some(2));
        let items = &outline.groups[0].items;
        assert!(outline.groups[0].title.is_empty());
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].number, 1);
        assert_eq!(items[0].label, "hola");
        assert_eq!(items[0].block, 0);
        assert!(items[0].active, "the reply still belongs to the question");
        assert_eq!(items[1].number, 2);
        assert_eq!(items[1].label, "podrias abrir un proyecto?");
        assert!(!items[1].active);
        assert_eq!(
            outline.marks(),
            vec![
                OutlineMark {
                    block: 0,
                    active: true,
                },
                OutlineMark {
                    block: 3,
                    active: false,
                },
            ]
        );
    }

    #[test]
    fn a_long_question_is_cut_and_a_reply_alone_is_absent() {
        let outline = chat_outline(&[text("# Hola")], None);
        assert!(outline.is_empty());
        let long = "a".repeat(80);
        let listed = chat_outline(&[prompt(&long)], None);
        let label = &listed.groups[0].items[0].label;
        assert_eq!(label.chars().count(), LABEL_LIMIT + 1);
        assert!(label.ends_with('…'));
    }
}
