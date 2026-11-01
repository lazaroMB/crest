//! Pasted images: the paste chain, the tray and the image mode.

use domain::conversation::ImageAttachment;
use gpui::{AppContext as _, Context, Task};

use super::attachments::{ImageMode, PendingImage};
use super::{InputEvent, InputView, tray};

/// A step of the paste chain of [`InputView`].
pub(super) enum PasteStep {
    /// Pasted images, being prepared in the background.
    Images(Task<Vec<Result<PendingImage, String>>>),
    /// A prompt sent while images were being prepared.
    Submit(String),
}

/// A [`PasteStep`] ready to land.
enum Landed {
    Images(Vec<Result<PendingImage, String>>),
    Submit(String),
}

impl InputView {
    /// The images waiting in the tray, in paste order.
    pub fn images(&self) -> &[PendingImage] {
        self.tray.images()
    }

    /// What `Enter` does with the images in the tray.
    pub fn image_mode(&self) -> ImageMode {
        self.tray.mode()
    }

    /// Switches what `Enter` does with the images (no-op without images).
    pub fn toggle_image_mode(&mut self, cx: &mut Context<Self>) {
        if self.tray.toggle() {
            cx.notify();
        }
    }

    /// Adds pasted images to the tray; an image that cannot be attached
    /// is reported with [`InputEvent::Notice`]. They are checked and
    /// converted (a bmp re-encoded as png, the base64 of every one) on the
    /// background executor and land in the tray a moment later, in paste
    /// order.
    pub fn attach_images(&mut self, images: Vec<gpui::Image>, cx: &mut Context<Self>) {
        let prepared = cx.background_spawn(async move { tray::prepare(&images) });
        self.after_pastes(PasteStep::Images(prepared), cx);
    }

    /// Whether pasted images are still being prepared (or a prompt sent
    /// meanwhile still waits for them).
    pub fn pastes_pending(&self) -> bool {
        self.pastes_pending > 0
    }

    /// Chains `step` after the pastes not landed yet, so pastes land in
    /// paste order and a prompt sent while some were being prepared goes
    /// with them.
    pub(super) fn after_pastes(&mut self, step: PasteStep, cx: &mut Context<Self>) {
        let previous = self.attaching.take();
        self.pastes_pending += 1;
        self.attaching = Some(cx.spawn(async move |this, cx| {
            let landed = match step {
                PasteStep::Images(prepared) => Landed::Images(prepared.await),
                PasteStep::Submit(text) => Landed::Submit(text),
            };
            // An earlier step lands first.
            if let Some(previous) = previous {
                previous.await;
            }
            this.update(cx, |this, cx| {
                this.pastes_pending = this.pastes_pending.saturating_sub(1);
                match landed {
                    Landed::Images(prepared) => {
                        let problems = this.tray.add(prepared);
                        this.report(problems, cx);
                    }
                    Landed::Submit(text) => {
                        if let Some(event) = this.prompt_event(text) {
                            cx.emit(event);
                        }
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
    }

    /// Puts images back in the tray (a queued prompt taken back to edit).
    pub fn restore_images(&mut self, images: Vec<ImageAttachment>, cx: &mut Context<Self>) {
        let problems = self.tray.restore(images);
        self.report(problems, cx);
    }

    fn report(&mut self, problems: Vec<String>, cx: &mut Context<Self>) {
        for problem in problems {
            cx.emit(InputEvent::Notice(problem));
        }
        cx.notify();
    }

    /// Removes the image at `ix` from the tray.
    pub fn remove_image(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.tray.remove(ix) {
            cx.notify();
        }
    }
}
