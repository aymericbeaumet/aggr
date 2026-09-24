//! One bounded, lossy copy of an image, shared by archive compaction and deployment compression.
//!
//! Callers own their decoding and their guards. Animated, colour-managed and high-depth images
//! never reach this module: a smaller copy is not worth what they would lose.

use std::io::Cursor;

use anyhow::{Context, Result};
use image::{AnimationDecoder as _, DynamicImage, ImageDecoder as _, ImageEncoder as _};

use super::{Asset, MediaLimits, placeholder};

/// An animation is re-encoded frame by frame, so its cost grows with its length. Past this many
/// frames the exact original is cheaper to keep than to shrink.
const MAX_ANIMATION_FRAMES: usize = 400;
/// `image`'s quantizer runs from 1 (best, slowest) to 30. A mid setting keeps a resized animation
/// legible without spending a quality pass on every frame of every archived GIF.
const ANIMATION_SPEED: i32 = 10;

/// The longest axis a reduced copy may keep, and the quality its JPEG encodes at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactPolicy {
    pub max_axis: u32,
    pub jpeg_quality: u8,
}

impl CompactPolicy {
    /// What `images = "compact"` means before a configuration tunes it. These bounds match the
    /// deployment copy, so a compact archive already holds the image the site would publish and
    /// the build has nothing left to shrink.
    pub const fn archive() -> Self {
        Self {
            max_axis: 1600,
            jpeg_quality: 72,
        }
    }
}

/// A reduced copy together with the image decoded back from it, so callers take geometry and
/// previews from the bytes they will store rather than from the pixels they encoded.
pub struct Compacted {
    pub bytes: Vec<u8>,
    pub extension: &'static str,
    pub decoded: DynamicImage,
}

/// Resize to fit `policy.max_axis`, then encode: transparency keeps a PNG, everything else becomes
/// JPEG. Decoding the result verifies the encode and yields the stored bytes' own dimensions.
pub fn compact_image(image: &DynamicImage, policy: CompactPolicy) -> Result<Compacted> {
    let resized;
    let reduced = if image.width() > policy.max_axis || image.height() > policy.max_axis {
        resized = image.resize(
            policy.max_axis,
            policy.max_axis,
            image::imageops::FilterType::Lanczos3,
        );
        &resized
    } else {
        image
    };
    let rgba = reduced.to_rgba8();
    let transparent = rgba.pixels().any(|pixel| pixel.0[3] != 255);
    let mut bytes = Vec::new();
    let extension = if transparent {
        image::codecs::png::PngEncoder::new_with_quality(
            &mut bytes,
            image::codecs::png::CompressionType::Best,
            image::codecs::png::FilterType::Adaptive,
        )
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
        .context("encoding transparent compact image")?;
        "png"
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, policy.jpeg_quality)
            .encode_image(&DynamicImage::ImageRgb8(reduced.to_rgb8()))
            .context("encoding compact JPEG")?;
        "jpg"
    };
    let decoded = image::load_from_memory(&bytes).context("verifying compact image")?;
    Ok(Compacted {
        bytes,
        extension,
        decoded,
    })
}

/// Replace an asset's master with its compact copy when that copy is actually smaller, and drop
/// the responsive companions either way: lossless copies can outweigh the bounded master they
/// were derived from, and the browser can scale it. The source URL stays the article's key.
pub fn apply(asset: &mut Asset, image: &DynamicImage, policy: CompactPolicy) -> Result<()> {
    let compacted = compact_image(image, policy)?;
    replace(asset, compacted);
    Ok(())
}

/// The same reduction for an animation, which has to stay an animation: its frames are resized
/// and re-encoded, keeping their timing and the loop the original asked for. GIF is the only
/// animated format aggr can write, so animated WebP and APNG keep their exact bytes.
///
/// Declining to reduce one keeps the exact original; failing to *read* one is an error, and the
/// caller drops the image as it drops any other it could not decode. Compaction walks every frame,
/// so it is the first thing to notice a GIF whose later frames are corrupt.
pub fn apply_animation(
    asset: &mut Asset,
    policy: CompactPolicy,
    limits: &MediaLimits,
) -> Result<()> {
    if let Some(compacted) = compact_gif(&asset.master_bytes, policy, limits)? {
        replace(asset, compacted);
    }
    asset.renditions.clear();
    Ok(())
}

fn replace(asset: &mut Asset, compacted: Compacted) {
    if compacted.bytes.len() < asset.master_bytes.len() {
        asset.width = compacted.decoded.width();
        asset.height = compacted.decoded.height();
        // A still's preview comes from the bytes themselves; an animation's comes from the frame
        // a reader sees first, which is what the uncompacted archive also previews.
        if let Ok(preview) = placeholder::from_image(&compacted.decoded) {
            asset.placeholder = preview;
        }
        asset.master_hash = crate::model::sha1_hex(&compacted.bytes);
        asset.master_bytes = compacted.bytes;
        asset.master_extension = compacted.extension;
    }
    asset.renditions.clear();
}

/// `None` when the animation is longer than aggr will spend a re-encode on, or when re-encoding
/// it produced no usable result. Callers keep the exact original in that case.
fn compact_gif(
    bytes: &[u8],
    policy: CompactPolicy,
    limits: &MediaLimits,
) -> Result<Option<Compacted>> {
    let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
        .context("reading archived animation")?;
    decoder
        .set_limits(super::decoder_limits(limits))
        .context("bounding archived animation")?;
    let repeat = match decoder.loop_count() {
        image::metadata::LoopCount::Finite(count) => {
            image::codecs::gif::Repeat::Finite(u16::try_from(count.get()).unwrap_or(u16::MAX))
        }
        image::metadata::LoopCount::Infinite => image::codecs::gif::Repeat::Infinite,
    };

    // Frames are encoded as they are decoded. Holding the whole animation first would let a small,
    // densely compressed GIF expand into gigabytes of RGBA before anything bounded it.
    let mut bytes = Vec::new();
    let mut first = None;
    let mut frames = 0_usize;
    let mut pixels = 0_u64;
    {
        let mut encoder = image::codecs::gif::GifEncoder::new_with_speed(
            Bounded {
                out: &mut bytes,
                limit: limits.max_file_bytes,
            },
            ANIMATION_SPEED,
        );
        encoder
            .set_repeat(repeat)
            .context("setting archived animation loop")?;
        for frame in decoder.into_frames() {
            if frames == MAX_ANIMATION_FRAMES {
                return Ok(None);
            }
            let frame = frame.context("decoding archived animation frame")?;
            let delay = frame.delay();
            let buffer = DynamicImage::ImageRgba8(frame.into_buffer());
            let resized = if buffer.width() > policy.max_axis || buffer.height() > policy.max_axis {
                buffer.resize(
                    policy.max_axis,
                    policy.max_axis,
                    image::imageops::FilterType::Lanczos3,
                )
            } else {
                buffer
            };
            pixels += u64::from(resized.width()) * u64::from(resized.height());
            if pixels > limits.max_pixels {
                return Ok(None);
            }
            if first.is_none() {
                first = Some(resized.clone());
            }
            frames += 1;
            // A frame that will not fit the file limit, and any other encoder refusal, leaves the
            // exact original in place rather than failing the image.
            if encoder
                .encode_frame(image::Frame::from_parts(resized.to_rgba8(), 0, 0, delay))
                .is_err()
            {
                return Ok(None);
            }
        }
    }
    let Some(decoded) = first else {
        return Ok(None);
    };

    // The encoder writes its trailer when it is dropped and throws away any error doing so, so a
    // sink that refused those last bytes would leave a truncated file behind with nothing to say
    // it. Read the result back, one frame at a time, and require every frame that went in: a
    // reduced animation is only worth keeping if it is still the whole animation.
    if !encoded_frames(&bytes, limits).is_some_and(|written| written == frames) {
        return Ok(None);
    }
    Ok(Some(Compacted {
        bytes,
        extension: "gif",
        decoded,
    }))
}

/// How many frames the encoded animation reads back as, or `None` when it cannot be read at all.
fn encoded_frames(bytes: &[u8], limits: &MediaLimits) -> Option<usize> {
    let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).ok()?;
    decoder.set_limits(super::decoder_limits(limits)).ok()?;
    let mut written = 0_usize;
    for frame in decoder.into_frames() {
        frame.ok()?;
        written += 1;
    }
    Some(written)
}

/// A sink that refuses to grow past a limit, so an encoder cannot allocate an unbounded result
/// before anyone gets to compare it with the original.
struct Bounded<'a> {
    out: &'a mut Vec<u8>,
    limit: usize,
}

impl std::io::Write for Bounded<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.out.len().saturating_add(buf.len()) > self.limit {
            return Err(std::io::Error::other(
                "compact animation exceeds the file limit",
            ));
        }
        self.out.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    fn animation(frames: u32, width: u32, height: u32) -> Vec<u8> {
        let frame = |shift: u32| {
            ImageBuffer::from_fn(width, height, move |x, y| {
                Rgba([
                    ((x * 7 + shift) % 251) as u8,
                    (y % 239) as u8,
                    ((x + y) % 241) as u8,
                    255,
                ])
            })
        };
        let mut bytes = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut bytes)
            .encode_frames((0..frames).map(|index| image::Frame::new(frame(index * 60))))
            .unwrap();
        bytes
    }

    #[test]
    fn an_encode_cut_short_by_the_file_limit_is_discarded_whole() {
        // The GIF encoder writes its trailer when it is dropped and throws away the error if that
        // write fails, so a refused byte at the end leaves a truncated file behind that otherwise
        // looks like a successful reduction.
        let source = animation(3, 600, 400);
        let policy = CompactPolicy {
            max_axis: 500,
            jpeg_quality: 72,
        };
        for limit in [64, 512, source.len() / 4, source.len() / 2] {
            let limits = MediaLimits {
                max_file_bytes: limit,
                ..MediaLimits::default()
            };
            let compacted = compact_gif(&source, policy, &limits).unwrap();
            assert!(
                compacted.is_none(),
                "a {limit}-byte limit produced a {} byte animation instead of declining",
                compacted.map_or(0, |compacted| compacted.bytes.len())
            );
        }
    }

    #[test]
    fn a_reduction_that_survives_its_limits_reads_back_as_the_whole_animation() {
        let source = animation(3, 900, 600);
        let compacted = compact_gif(
            &source,
            CompactPolicy {
                max_axis: 200,
                jpeg_quality: 72,
            },
            &MediaLimits::default(),
        )
        .unwrap()
        .expect("a resized animation is worth keeping");
        assert_eq!(compacted.extension, "gif");
        assert_eq!(
            encoded_frames(&compacted.bytes, &MediaLimits::default()),
            Some(3)
        );
        assert_eq!(compacted.decoded.width(), 200);
    }
}
