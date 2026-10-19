//! Images pasted into the prompt: checked and normalized before they are
//! shown in the tray, so what the tray shows is exactly what is sent.
//!
//! - png, jpeg, gif and webp are kept as they are;
//! - bmp, tiff, ico and pnm are decoded and re-encoded as png, the formats
//!   the model does not take;
//! - svg is refused (it is a document, not a picture);
//! - the result must pass [`ImageAttachment::check`] (the limits of the
//!   domain), which says what is too large.
//!
//! The bytes stay in memory only: nothing is written to disk, and a removed
//! or sent image is simply dropped.

use std::io::Cursor;
use std::sync::Arc;

use base64::Engine as _;
use domain::conversation::ImageAttachment;
use gpui::{Image, ImageFormat};

/// What the user chose to do with the pasted images.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ImageMode {
    /// Attached to the prompt and sent to the model with it.
    #[default]
    Model,
    /// Added to the chat as image blocks; the model is not called.
    ChatOnly,
}

impl ImageMode {
    /// The other mode.
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::Model => Self::ChatOnly,
            Self::ChatOnly => Self::Model,
        }
    }
}

/// An image waiting in the tray.
#[derive(Debug, Clone)]
pub struct PendingImage {
    /// What is sent: media type, base64 bytes and size in pixels.
    pub attachment: ImageAttachment,
    /// The same bytes for the thumbnail.
    pub preview: Arc<Image>,
}

impl PendingImage {
    /// Size of the bytes that are sent.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.preview.bytes().len()
    }
}

/// Checks a pasted `image` and turns it into a [`PendingImage`]; the error
/// says why it cannot be attached.
///
/// # Errors
///
/// An svg, bytes that do not decode, or an image over the limits.
pub fn pending_image(image: &Image) -> Result<PendingImage, String> {
    let (format, bytes) = match image.format() {
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::Webp => {
            (image.format(), image.bytes().to_vec())
        }
        ImageFormat::Bmp | ImageFormat::Tiff | ImageFormat::Ico | ImageFormat::Pnm => {
            (ImageFormat::Png, to_png(image.bytes())?)
        }
        ImageFormat::Svg => {
            return Err("svg images cannot be attached; paste a png or jpeg instead".into());
        }
    };
    let (width, height) = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|err| format!("cannot read the pasted image: {err}"))?
        .into_dimensions()
        .map_err(|err| format!("cannot read the pasted image: {err}"))?;
    let attachment = ImageAttachment {
        media_type: format.mime_type().to_owned(),
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        width,
        height,
    };
    attachment.check()?;
    Ok(PendingImage {
        attachment,
        preview: Arc::new(Image::from_bytes(format, bytes)),
    })
}

/// Decodes `bytes` (any format `image` reads) and encodes them as png.
fn to_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let decoded = image::load_from_memory(bytes)
        .map_err(|err| format!("cannot read the pasted image: {err}"))?;
    let mut png = Vec::new();
    decoded
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|err| format!("cannot convert the pasted image to png: {err}"))?;
    Ok(png)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A `width`×`height` image encoded as `format`.
    pub(crate) fn encoded(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
        let picture = image::RgbImage::from_pixel(width, height, image::Rgb([200, 30, 30]));
        let mut bytes = Vec::new();
        picture
            .write_to(&mut Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    #[test]
    fn model_formats_pass_through_unchanged() {
        let png = encoded(3, 2, image::ImageFormat::Png);
        let pending = pending_image(&Image::from_bytes(ImageFormat::Png, png.clone())).unwrap();
        assert_eq!(pending.attachment.media_type, "image/png");
        assert_eq!(
            (pending.attachment.width, pending.attachment.height),
            (3, 2)
        );
        assert_eq!(pending.preview.bytes(), png.as_slice());
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&pending.attachment.data)
            .unwrap();
        assert_eq!(decoded, png);
    }

    #[test]
    fn other_formats_become_png() {
        let bmp = encoded(4, 4, image::ImageFormat::Bmp);
        let pending = pending_image(&Image::from_bytes(ImageFormat::Bmp, bmp)).unwrap();
        assert_eq!(pending.attachment.media_type, "image/png");
        assert_eq!(pending.preview.format(), ImageFormat::Png);
        assert_eq!(
            (pending.attachment.width, pending.attachment.height),
            (4, 4)
        );
    }

    #[test]
    fn svg_garbage_and_oversized_images_are_refused() {
        let svg = Image::from_bytes(ImageFormat::Svg, b"<svg/>".to_vec());
        assert!(pending_image(&svg).unwrap_err().contains("svg"));
        let junk = Image::from_bytes(ImageFormat::Png, b"not a png".to_vec());
        assert!(pending_image(&junk).unwrap_err().contains("cannot read"));
        let tall = encoded(
            1,
            domain::conversation::MAX_IMAGE_SIDE + 1,
            image::ImageFormat::Png,
        );
        let err = pending_image(&Image::from_bytes(ImageFormat::Png, tall)).unwrap_err();
        assert!(err.contains("8001 px"), "{err}");
    }
}
