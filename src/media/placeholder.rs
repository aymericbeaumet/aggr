//! Inline previews derived from ThumbHash, with no browser decoder or network request.

use std::io::Cursor;

use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageDecoder as _, ImageEncoder as _, ImageReader};
use serde::{Deserialize, Serialize};

const PREFIX: &str = "data:image/png;base64,";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placeholder {
    pub hash: String,
    pub data_url: String,
}

/// The caller has already applied orientation and selected the animation's first frame.
pub fn from_image(image: &DynamicImage) -> Result<Placeholder> {
    ensure!(
        image.width() > 0 && image.height() > 0,
        "empty placeholder image"
    );
    let small = image.thumbnail(100, 100);
    // Alpha uses five frequencies per axis; undersampling very thin images aliases them.
    let small = if small.width() < 5 || small.height() < 5 {
        small.resize_exact(
            small.width().max(5),
            small.height().max(5),
            image::imageops::FilterType::Nearest,
        )
    } else {
        small
    }
    .to_rgba8();
    let hash = thumbhash::rgba_to_thumb_hash(
        small.width() as usize,
        small.height() as usize,
        small.as_raw(),
    );
    from_hash(&STANDARD.encode(hash))
}

pub(super) fn from_hash(encoded: &str) -> Result<Placeholder> {
    ensure!(encoded.len() <= 48, "ThumbHash too large");
    let hash = STANDARD.decode(encoded)?;
    ensure!((5..=32).contains(&hash.len()), "invalid ThumbHash length");
    let (width, height, pixels) =
        thumbhash::thumb_hash_to_rgba(&hash).map_err(|()| anyhow::anyhow!("decoding ThumbHash"))?;
    ensure!(
        (1..=32).contains(&width) && (1..=32).contains(&height),
        "invalid ThumbHash dimensions"
    );
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(
        &pixels,
        width as u32,
        height as u32,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(Placeholder {
        hash: STANDARD.encode(hash),
        data_url: format!("{PREFIX}{}", STANDARD.encode(png)),
    })
}

/// Bounded decoding for existing thumbnail companions, including their EXIF orientation.
pub fn from_bytes(bytes: &[u8]) -> Result<Placeholder> {
    let limits = super::MediaLimits::default();
    ensure!(
        bytes.len() <= limits.max_file_bytes,
        "placeholder input too large"
    );
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(super::decoder_limits(&limits));
    let mut decoder = reader.into_decoder().context("reading placeholder image")?;
    let (width, height) = decoder.dimensions();
    super::validate_stored_dimensions(width, height, &limits)?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    from_image(&image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(placeholder: &Placeholder) -> image::RgbaImage {
        image::load_from_memory(
            &STANDARD
                .decode(placeholder.data_url.strip_prefix(PREFIX).unwrap())
                .unwrap(),
        )
        .unwrap()
        .to_rgba8()
    }

    #[test]
    fn deterministic_inline_png_is_the_decoded_hash_with_preserved_aspect_and_color() {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1200,
            600,
            image::Rgba([220, 50, 30, 255]),
        ));
        let placeholder = from_image(&image).unwrap();
        assert_eq!(placeholder, from_image(&image).unwrap());
        assert_eq!(from_hash(&placeholder.hash).unwrap(), placeholder);
        let png = decoded(&placeholder);
        let (width, height, pixels) =
            thumbhash::thumb_hash_to_rgba(&STANDARD.decode(&placeholder.hash).unwrap()).unwrap();
        assert_eq!(png.dimensions(), (width as u32, height as u32));
        assert_eq!(png.as_raw(), &pixels);
        assert!(width > height);
        assert!((i16::from(png.get_pixel(0, 0)[0]) - 220).abs() < 15);
    }

    #[test]
    fn transparent_and_extreme_aspect_images_produce_bounded_placeholders() {
        for (width, height, alpha) in [(64, 64, 0), (1, 1000, 100), (1000, 1, 255)] {
            let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                width,
                height,
                image::Rgba([30, 80, 180, alpha]),
            ));
            let placeholder = from_image(&image).unwrap();
            assert_eq!(from_hash(&placeholder.hash).unwrap(), placeholder);
            let png = decoded(&placeholder);
            assert!(png.width() > 0 && png.height() > 0);
            assert!((i16::from(png.get_pixel(0, 0)[3]) - i16::from(alpha)).abs() < 20);
        }
        assert!(from_image(&DynamicImage::new_rgba8(0, 0)).is_err());
        assert!(from_bytes(b"invalid image").is_err());
    }

    #[test]
    fn cache_validation_rejects_truncated_and_oversized_hashes() {
        for hash in [
            "".into(),
            STANDARD.encode([0; 4]),
            STANDARD.encode([0; 5]),
            STANDARD.encode([0; 33]),
            "bad()'".into(),
        ] {
            assert!(from_hash(&hash).is_err());
        }
    }

    #[test]
    fn encoded_image_orientation_is_applied_before_hashing() {
        let original = DynamicImage::ImageRgb8(image::RgbImage::from_fn(80, 40, |x, _| {
            if x < 40 {
                image::Rgb([240, 30, 20])
            } else {
                image::Rgb([20, 40, 240])
            }
        }));
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut bytes);
        encoder
            .set_exif_metadata(vec![
                b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0,
                0, 0,
            ])
            .unwrap();
        encoder.encode_image(&original).unwrap();
        let placeholder = from_bytes(&bytes).unwrap();
        let png = decoded(&placeholder);
        assert!(png.height() > png.width());
        let top = png.get_pixel(png.width() / 2, 0);
        let bottom = png.get_pixel(png.width() / 2, png.height() - 1);
        assert!(top[0] > top[2]);
        assert!(bottom[2] > bottom[0]);
    }
}
