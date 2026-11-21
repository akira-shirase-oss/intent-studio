//! PNG and JPEG: decoded under limits and encoded again (D-062). Re-encoding proves the whole
//! image decodes and leaves every metadata chunk behind.

use std::io::Cursor;

use image::{ImageFormat, ImageReader, Limits, codecs::jpeg::JpegEncoder};
use intent_render::{AssetKind, WorkerRefusal};

/// Widest and tallest image accepted.
pub const MAX_SIDE: u32 = 8_192;
/// Most memory the decoder may hold at once.
pub const MAX_DECODE_BYTES: u64 = 256 * 1024 * 1024;
/// JPEG quality on re-encoding.
pub const JPEG_QUALITY: u8 = 85;

/// A re-encoded image.
#[derive(Debug, Clone)]
pub struct Reencoded {
    /// `image/png` or `image/jpeg`.
    pub media_type: &'static str,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The new file.
    pub bytes: Vec<u8>,
}

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
const JPEG_MAGIC: &[u8] = &[0xFF, 0xD8, 0xFF];

/// Decodes and re-encodes `input` as `kind` (PNG or JPEG).
pub fn reencode(kind: AssetKind, input: &[u8]) -> Result<Reencoded, WorkerRefusal> {
    let (format, media_type) = match kind {
        AssetKind::Png if input.starts_with(PNG_MAGIC) => (ImageFormat::Png, "image/png"),
        AssetKind::Jpeg if input.starts_with(JPEG_MAGIC) => (ImageFormat::Jpeg, "image/jpeg"),
        AssetKind::Png | AssetKind::Jpeg => return Err(WorkerRefusal::TypeMismatch),
        AssetKind::Svg | AssetKind::Font | AssetKind::Check => {
            return Err(WorkerRefusal::UnsupportedType);
        }
    };
    let mut reader = ImageReader::with_format(Cursor::new(input), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let image = reader.decode().map_err(|error| match error {
        image::ImageError::Limits(_) => WorkerRefusal::TooLarge,
        _ => WorkerRefusal::Malformed,
    })?;
    let (width, height) = (image.width(), image.height());
    let mut bytes = Vec::new();
    match format {
        ImageFormat::Jpeg => {
            // JPEG has no alpha: anything decoded with one is flattened to RGB.
            let rgb = image.to_rgb8();
            JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
                .encode_image(&rgb)
                .map_err(|_| WorkerRefusal::Malformed)?;
        }
        _ => image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .map_err(|_| WorkerRefusal::Malformed)?,
    }
    Ok(Reencoded {
        media_type,
        width,
        height,
        bytes,
    })
}
