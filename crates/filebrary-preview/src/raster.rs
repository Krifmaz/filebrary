//! Pure Rust image work: decoding with limits, EXIF orientation, resizing,
//! encoding, and pulling the embedded preview out of camera RAW files.
//!
//! Everything here is CPU-bound and synchronous; callers run it on a blocking
//! thread.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader, Limits};

use crate::error::{PreviewError, Result};

/// Decoder limits that stop decompression bombs: a 100-megapixel image
/// (40 000 px on a side at most) and 1 GiB of decoder memory.
pub fn decode_limits() -> Limits {
    let mut l = Limits::default();
    l.max_image_width = Some(40_000);
    l.max_image_height = Some(40_000);
    l.max_alloc = Some(1 << 30);
    l
}

/// An encoded preview image, ready to write to the cache.
#[derive(Debug, Clone)]
pub struct Encoded {
    /// The encoded bytes.
    pub bytes: Vec<u8>,
    /// `image/jpeg` or `image/png`.
    pub content_type: &'static str,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Encoded {
    /// File extension matching [`Encoded::content_type`].
    pub fn extension(&self) -> &'static str {
        if self.content_type == "image/png" {
            "png"
        } else {
            "jpg"
        }
    }
}

/// Decode an image from memory, guessing the format from its content and
/// applying the EXIF orientation so portrait photos stand upright.
pub fn decode(bytes: &[u8]) -> Result<DynamicImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(decode_limits());
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Shrink `img` to fit in a `max` x `max` box (never enlarges), then encode
/// it: JPEG for opaque images, PNG when there is transparency to keep.
pub fn resize_and_encode(img: &DynamicImage, max: u32) -> Result<Encoded> {
    let resized = if img.width() > max || img.height() > max {
        if max <= 512 {
            // `thumbnail` uses a fast area-averaging filter that is plenty
            // for grid-sized images.
            img.thumbnail(max, max)
        } else {
            img.resize(max, max, FilterType::Triangle)
        }
    } else {
        img.clone()
    };
    encode(&resized)
}

/// Encode an already-sized image.
pub fn encode(img: &DynamicImage) -> Result<Encoded> {
    let (width, height) = (img.width(), img.height());
    let mut bytes = Vec::new();
    if img.color().has_alpha() && has_transparency(img) {
        let rgba = img.to_rgba8();
        PngEncoder::new(&mut bytes).write_image(
            rgba.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )?;
        Ok(Encoded {
            bytes,
            content_type: "image/png",
            width,
            height,
        })
    } else {
        let rgb = img.to_rgb8();
        JpegEncoder::new_with_quality(&mut bytes, 82).write_image(
            rgb.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgb8,
        )?;
        Ok(Encoded {
            bytes,
            content_type: "image/jpeg",
            width,
            height,
        })
    }
}

fn has_transparency(img: &DynamicImage) -> bool {
    img.to_rgba8().pixels().any(|p| p.0[3] < 255)
}

/// Find the largest JPEG a camera embedded in a RAW file and decode it.
///
/// Nearly every RAW format (CR2, CR3, NEF, ARW, DNG, RAF, ORF, RW2, PEF, ...)
/// stores a full or near-full size JPEG preview next to the sensor data. We
/// scan for JPEG start markers, read each candidate's header to learn its
/// size, and decode the biggest one. This is pure Rust and much faster than
/// demosaicing the sensor data.
///
/// If no embedded JPEG is found, the file is tried as a plain TIFF (which
/// also covers `.tif` files that were sniffed as RAW).
pub fn decode_raw(bytes: &[u8]) -> Result<DynamicImage> {
    let mut best: Option<(u64, usize)> = None;
    for offset in jpeg_candidates(bytes) {
        if let Some((w, h)) = jpeg_dimensions(&bytes[offset..]) {
            let area = u64::from(w) * u64::from(h);
            if best.is_none_or(|(a, _)| area > a) {
                best = Some((area, offset));
            }
        }
    }
    if let Some((_, offset)) = best {
        let orientation = raw_orientation(bytes);
        let mut img = decode(&bytes[offset..])?;
        // Embedded previews usually have no EXIF of their own; the
        // orientation lives in the RAW file's main TIFF header instead.
        if let Some(o) = orientation {
            img.apply_orientation(o);
        }
        return Ok(img);
    }
    decode(bytes).map_err(|_| PreviewError::Decode("no embedded preview found".into()))
}

/// Offsets of every `FF D8 FF xx` sequence that plausibly starts a JPEG.
fn jpeg_candidates(bytes: &[u8]) -> impl Iterator<Item = usize> + '_ {
    bytes.windows(4).enumerate().filter_map(|(i, w)| {
        (w[0] == 0xFF && w[1] == 0xD8 && w[2] == 0xFF && matches!(w[3], 0xC0..=0xFE)).then_some(i)
    })
}

/// Read a JPEG's dimensions from its header without decoding pixels.
fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Jpeg);
    reader.limits(decode_limits());
    let (w, h) = reader.into_dimensions().ok()?;
    (w >= 64 && h >= 64).then_some((w, h))
}

/// Read the EXIF orientation tag (0x0112) from IFD0 of a TIFF-based RAW file.
fn raw_orientation(bytes: &[u8]) -> Option<image::metadata::Orientation> {
    let le = match bytes.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let b: [u8; 2] = bytes.get(o..o + 2)?.try_into().ok()?;
        Some(if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b: [u8; 4] = bytes.get(o..o + 4)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    (0..count.min(512)).find_map(|i| {
        let entry = ifd + 2 + i * 12;
        (u16_at(entry)? == 0x0112)
            .then(|| u16_at(entry + 8))
            .flatten()
            .and_then(|v| image::metadata::Orientation::from_exif(v as u8))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage, Rgba, RgbaImage};

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, Rgb([200, 10, 10])));
        encode(&img).unwrap().bytes
    }

    #[test]
    fn resize_fits_box_and_keeps_aspect() {
        let img = DynamicImage::ImageRgb8(RgbImage::new(2000, 1000));
        let e = resize_and_encode(&img, 256).unwrap();
        assert_eq!((e.width, e.height), (256, 128));
        assert_eq!(e.content_type, "image/jpeg");
    }

    #[test]
    fn small_images_are_not_enlarged() {
        let img = DynamicImage::ImageRgb8(RgbImage::new(100, 50));
        let e = resize_and_encode(&img, 256).unwrap();
        assert_eq!((e.width, e.height), (100, 50));
    }

    #[test]
    fn transparency_is_kept_as_png() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 0])));
        assert_eq!(encode(&img).unwrap().content_type, "image/png");
        let opaque = DynamicImage::ImageRgba8(RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 255])));
        assert_eq!(encode(&opaque).unwrap().content_type, "image/jpeg");
    }

    #[test]
    fn raw_picks_the_largest_embedded_jpeg() {
        // Fake RAW: a TIFF-ish header, junk, a small thumb, junk, a big preview.
        let mut raw = b"II*\0\x08\0\0\0".to_vec();
        raw.extend(std::iter::repeat_n(0xAB, 1000));
        raw.extend(jpeg(160, 120));
        raw.extend(std::iter::repeat_n(0xCD, 1000));
        raw.extend(jpeg(640, 480));
        raw.extend(std::iter::repeat_n(0xEF, 1000));
        let img = decode_raw(&raw).unwrap();
        assert_eq!((img.width(), img.height()), (640, 480));
    }

    #[test]
    fn raw_without_preview_is_a_decode_error() {
        let raw = vec![0u8; 4096];
        assert!(matches!(decode_raw(&raw), Err(PreviewError::Decode(_))));
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(matches!(
            decode(b"not an image"),
            Err(PreviewError::Decode(_))
        ));
    }
}
