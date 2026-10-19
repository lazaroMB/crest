//! The timeline blocks of what the user sends: the prompt, quoted, and the
//! images pasted with it, one block each. The same blocks show a prompt
//! sent to the model and images added to the chat only, so every front end
//! draws them alike.

use domain::block::{Block, BlockKind, ImageData};
use domain::conversation::ImageAttachment;

/// The block that shows a prompt sent by the user in the timeline.
#[must_use]
pub fn prompt_block(prompt: &str) -> Block {
    let quoted: String = prompt.lines().map(|line| format!("> {line}\n")).collect();
    Block::new(BlockKind::Text { markdown: quoted })
}

/// The block that shows a pasted image.
#[must_use]
pub fn image_block(image: &ImageAttachment) -> Block {
    Block::new(BlockKind::Image {
        media_type: image.media_type.clone(),
        data_or_path: ImageData::Base64 {
            data: image.data.clone(),
        },
        source: None,
    })
}

/// The prompt (unless it is empty and images go with it) and then one
/// block per image, in order.
#[must_use]
pub fn user_blocks(prompt: &str, images: &[ImageAttachment]) -> Vec<Block> {
    let mut blocks = Vec::with_capacity(images.len() + 1);
    if !prompt.trim().is_empty() || images.is_empty() {
        blocks.push(prompt_block(prompt));
    }
    blocks.extend(images.iter().map(image_block));
    blocks
}

/// What the user is told once `count` images were added to the chat only.
#[must_use]
pub fn chat_only_notice(count: usize) -> String {
    let images = if count == 1 {
        "1 image".to_owned()
    } else {
        format!("{count} images")
    };
    format!("{images} added to the chat; not sent to the model")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> ImageAttachment {
        ImageAttachment {
            media_type: "image/png".into(),
            data: "iVBO".into(),
            width: 1,
            height: 1,
        }
    }

    fn markdown(block: &Block) -> Option<&str> {
        match &block.kind {
            BlockKind::Text { markdown } => Some(markdown),
            _ => None,
        }
    }

    #[test]
    fn the_prompt_block_quotes_every_line() {
        let block = prompt_block("uno\ndos");
        assert_eq!(markdown(&block), Some("> uno\n> dos\n"));
    }

    #[test]
    fn the_prompt_goes_before_its_images() {
        let blocks = user_blocks("look", &[png(), png()]);
        assert_eq!(blocks.len(), 3);
        assert_eq!(markdown(&blocks[0]), Some("> look\n"));
        assert!(matches!(
            &blocks[1].kind,
            BlockKind::Image { media_type, data_or_path: ImageData::Base64 { data }, source: None }
                if media_type == "image/png" && data == "iVBO"
        ));
        // Images alone have no empty quote; a plain prompt is one block.
        assert_eq!(user_blocks("  ", &[png()]).len(), 1);
        assert_eq!(user_blocks("hi", &[]).len(), 1);
    }

    #[test]
    fn the_notice_counts_the_images() {
        assert_eq!(
            chat_only_notice(1),
            "1 image added to the chat; not sent to the model"
        );
        assert!(chat_only_notice(3).starts_with("3 images"));
    }
}
