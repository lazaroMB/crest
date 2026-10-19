//! The tray of pasted images above the prompt: the images waiting to be
//! sent ([`PendingImage`]) and what `Enter` does with them
//! ([`ImageMode`]), with the paste hook that fills it. The [`InputView`]
//! owns it and takes its content when the prompt is sent.

use base64::Engine as _;
use domain::conversation::{ImageAttachment, MAX_IMAGES_PER_MESSAGE, size_text};
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, WeakEntity,
    Window, div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::InputView;
use super::attachments::{ImageMode, PendingImage, pending_image};

/// Side of a tray thumbnail, in pixels.
const THUMB_SIZE: f32 = 64.0;

/// The images waiting to be sent and what `Enter` does with them.
#[derive(Default)]
pub struct ImageTray {
    images: Vec<PendingImage>,
    mode: ImageMode,
}

impl ImageTray {
    /// The images, in paste order.
    pub fn images(&self) -> &[PendingImage] {
        &self.images
    }

    /// What `Enter` does with them.
    pub fn mode(&self) -> ImageMode {
        self.mode
    }

    /// Whether there is no image.
    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// Switches the mode; returns whether it changed (only with images).
    pub fn toggle(&mut self) -> bool {
        if self.images.is_empty() {
            return false;
        }
        self.mode = self.mode.toggled();
        true
    }

    /// Adds pasted images; returns why the ones left out could not be
    /// attached.
    pub fn attach(&mut self, images: Vec<gpui::Image>) -> Vec<String> {
        let mut problems = Vec::new();
        for image in images {
            if self.images.len() >= MAX_IMAGES_PER_MESSAGE {
                problems.push(format!(
                    "at most {MAX_IMAGES_PER_MESSAGE} images go with a message; remove one first"
                ));
                break;
            }
            match pending_image(&image) {
                Ok(pending) => self.images.push(pending),
                Err(err) => problems.push(err),
            }
        }
        problems
    }

    /// Puts sent images back (a queued prompt taken back to edit).
    pub fn restore(&mut self, images: Vec<ImageAttachment>) -> Vec<String> {
        let decoded = images
            .into_iter()
            .filter_map(|attachment| {
                let format = gpui::ImageFormat::from_mime_type(&attachment.media_type)?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(attachment.data)
                    .ok()?;
                Some(gpui::Image::from_bytes(format, bytes))
            })
            .collect();
        self.attach(decoded)
    }

    /// Removes the image at `ix`; the mode goes back to the default with
    /// the last one. Returns whether there was one.
    pub fn remove(&mut self, ix: usize) -> bool {
        if ix >= self.images.len() {
            return false;
        }
        self.images.remove(ix);
        if self.images.is_empty() {
            self.mode = ImageMode::default();
        }
        true
    }

    /// Empties the tray for sending: the images and their mode, which goes
    /// back to the default.
    pub fn take(&mut self) -> (Vec<ImageAttachment>, ImageMode) {
        let images = std::mem::take(&mut self.images)
            .into_iter()
            .map(|image| image.attachment)
            .collect();
        (images, std::mem::take(&mut self.mode))
    }

    /// Draws the tray, if it has images: thumbnails with `×`, the mode
    /// chips and a line saying what `Enter` does.
    pub fn render(&self, cx: &mut Context<InputView>) -> Option<AnyElement> {
        if self.images.is_empty() {
            return None;
        }
        let thumbs: Vec<_> = self
            .images
            .iter()
            .enumerate()
            .map(|(ix, image)| thumbnail(ix, image, cx))
            .collect();
        let model = self.chip("→ Model", ImageMode::Model, cx);
        let chat = self.chip("Chat only", ImageMode::ChatOnly, cx);
        let theme = cx.theme();
        Some(
            v_flex()
                .id("image-tray")
                .mb_1()
                .p_1()
                .gap_1()
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .text_sm()
                .child(
                    h_flex()
                        .gap_2()
                        .items_start()
                        .child(h_flex().flex_1().flex_wrap().gap_2().children(thumbs))
                        .child(h_flex().gap_1().child(model).child(chat)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(tray_hint(self.images.len(), self.mode))),
                )
                .into_any_element(),
        )
    }

    fn chip(
        &self,
        label: &'static str,
        mode: ImageMode,
        cx: &mut Context<InputView>,
    ) -> AnyElement {
        let on = self.mode == mode;
        let selector = match mode {
            ImageMode::Model => "tray-model",
            ImageMode::ChatOnly => "tray-chat-only",
        };
        let listener = cx.listener(move |input, _, _, cx| {
            input.tray.mode = mode;
            cx.notify();
        });
        let theme = cx.theme();
        let chip = div()
            .id(label)
            .debug_selector(move || selector.to_owned())
            .px_2()
            .rounded_md()
            .border_1()
            .cursor_pointer()
            .child(if on {
                format!("◉ {label}")
            } else {
                format!("○ {label}")
            })
            .on_click(listener);
        if on {
            chip.border_color(theme.primary)
                .text_color(theme.primary)
                .into_any_element()
        } else {
            chip.border_color(theme.border)
                .text_color(theme.muted_foreground)
                .into_any_element()
        }
    }
}

/// The thumbnail of image `ix`, with its size and a `×` that removes it.
fn thumbnail(ix: usize, image: &PendingImage, cx: &mut Context<InputView>) -> AnyElement {
    let info = format!(
        "{}×{} · {}",
        image.attachment.width,
        image.attachment.height,
        size_text(image.byte_len())
    );
    let remove = cx.listener(move |input, _, _, cx| input.remove_image(ix, cx));
    let theme = cx.theme();
    v_flex()
        .id(("tray-image", ix))
        .gap_0p5()
        .items_center()
        .child(
            div()
                .relative()
                .size(gpui::px(THUMB_SIZE))
                .border_1()
                .border_color(theme.border)
                .rounded_sm()
                .overflow_hidden()
                .bg(theme.muted)
                .child(
                    gpui::img(image.preview.clone())
                        .size_full()
                        .object_fit(gpui::ObjectFit::Contain),
                )
                .child(
                    div()
                        .id(("remove-image", ix))
                        .debug_selector(move || format!("tray-remove-{ix}"))
                        .absolute()
                        .top_0()
                        .right_0()
                        .px_1()
                        .rounded_sm()
                        .bg(theme.background)
                        .text_xs()
                        .text_color(theme.foreground)
                        .cursor_pointer()
                        .child("×")
                        .on_click(remove),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(info)),
        )
        .into_any_element()
}

/// What `Enter` will do with `count` images in `mode`, said under the tray.
fn tray_hint(count: usize, mode: ImageMode) -> String {
    let images = if count == 1 {
        "1 image".to_owned()
    } else {
        format!("{count} images")
    };
    match mode {
        ImageMode::Model => format!(
            "Enter: send the text and {images} to the model · Alt+M: chat only · × or Backspace removes"
        ),
        ImageMode::ChatOnly => format!(
            "Enter: add {images} to the chat; the model is not called · Alt+M: send to the model · × or Backspace removes"
        ),
    }
}

/// Takes the images of a paste for the tray (the rest of the paste is
/// dropped); a paste without images inserts its text as usual.
pub fn paste_handler(
    input: WeakEntity<InputView>,
) -> impl Fn(&gpui::ClipboardItem, &mut Window, &mut App) -> bool + 'static {
    move |item, _, cx| {
        let images: Vec<gpui::Image> = item
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                gpui::ClipboardEntry::Image(image) => Some(image.clone()),
                _ => None,
            })
            .collect();
        if images.is_empty() {
            return false;
        }
        input
            .update(cx, |input, cx| input.attach_images(images, cx))
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::attachments::tests::encoded;

    fn png(width: u32) -> gpui::Image {
        gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            encoded(width, 2, image::ImageFormat::Png),
        )
    }

    #[test]
    fn the_tray_says_what_enter_does() {
        assert!(
            tray_hint(1, ImageMode::Model)
                .starts_with("Enter: send the text and 1 image to the model")
        );
        let chat = tray_hint(2, ImageMode::ChatOnly);
        assert!(
            chat.contains("add 2 images to the chat; the model is not called"),
            "{chat}"
        );
    }

    #[test]
    fn images_come_and_go_with_their_mode() {
        let mut tray = ImageTray::default();
        assert!(!tray.toggle(), "no mode without images");
        assert!(tray.attach(vec![png(3), png(4)]).is_empty());
        assert!(tray.toggle());
        assert_eq!(tray.mode(), ImageMode::ChatOnly);
        let (images, mode) = tray.take();
        assert_eq!((images.len(), mode), (2, ImageMode::ChatOnly));
        assert!(tray.is_empty());
        assert_eq!(tray.mode(), ImageMode::Model);
        // Taken back from the queue, then removed one by one.
        assert!(tray.restore(images).is_empty());
        tray.toggle();
        assert!(tray.remove(0));
        assert!(tray.remove(0));
        assert!(!tray.remove(0));
        assert_eq!(tray.mode(), ImageMode::Model);
    }

    #[test]
    fn a_full_tray_says_so() {
        let mut tray = ImageTray::default();
        let problems = tray.attach((0..=MAX_IMAGES_PER_MESSAGE).map(|_| png(1)).collect());
        assert_eq!(tray.images().len(), MAX_IMAGES_PER_MESSAGE);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("at most"));
    }
}
