//! Smaller deployment copies; archived masters never pass through this module on write.

use std::io::{Cursor, Read as _, Write as _};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use anyhow::{Context, Result, ensure};
use image::{DynamicImage, ImageDecoder as _, ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::media::compact::{self, CompactPolicy};
use crate::media::{Asset, MediaLimits, placeholder};

const MAX_AXIS: u32 = 1600;
const DEPLOYMENT_POLICY: CompactPolicy = CompactPolicy {
    max_axis: MAX_AXIS,
    jpeg_quality: 72,
};
const POLICY: &str = "deployment-media-v1";
const MAX_RECEIPT_BYTES: usize = 16 * 1024;
const CACHE_LOCK_STRIPES: usize = 64;

static COMPACT_HITS: AtomicU64 = AtomicU64::new(0);
static COMPACT_MISSES: AtomicU64 = AtomicU64::new(0);
static RESPONSIVE_HITS: AtomicU64 = AtomicU64::new(0);
static RESPONSIVE_MISSES: AtomicU64 = AtomicU64::new(0);
static ENCODED_BYTES: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize)]
pub(crate) struct CacheStatistics {
    compressed_hits: u64,
    compressed_misses: u64,
    responsive_hits: u64,
    responsive_misses: u64,
    encoded_bytes: u64,
}

pub(crate) fn cache_statistics() -> CacheStatistics {
    CacheStatistics {
        compressed_hits: COMPACT_HITS.load(Ordering::Relaxed),
        compressed_misses: COMPACT_MISSES.load(Ordering::Relaxed),
        responsive_hits: RESPONSIVE_HITS.load(Ordering::Relaxed),
        responsive_misses: RESPONSIVE_MISSES.load(Ordering::Relaxed),
        encoded_bytes: ENCODED_BYTES.load(Ordering::Relaxed),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    checksum: String,
    record: Record,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    key: String,
    policy: String,
    #[serde(default)]
    implementation: String,
    clear_renditions: bool,
    replacement: Option<Replacement>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Replacement {
    sha256: String,
    bytes: usize,
    extension: String,
    width: u32,
    height: u32,
    thumbhash: String,
}

/// The caller passes the deployment-media cache namespace, which can survive both build retries
/// and later CI runs. Cache failure never prevents publishing a freshly computed result.
pub(super) fn compact_cached(asset: Asset, cache_root: Option<&Path>) -> Result<Asset> {
    compact_with(asset, cache_root, implementation_key(), compact)
}

fn compact_with(
    asset: Asset,
    cache_root: Option<&Path>,
    implementation: &str,
    compressor: impl FnOnce(Asset) -> Result<Asset>,
) -> Result<Asset> {
    ensure!(
        asset.master_bytes.len() <= MediaLimits::default().max_file_bytes,
        "archived image exceeds deployment decoder byte limit"
    );
    let Some(root) = cache_root else {
        return compressor(asset);
    };
    let key = cache_key(&asset, implementation);
    // Article workers can encounter the same publisher image simultaneously. Hold its stripe
    // through publication so later workers observe the receipt instead of encoding it again.
    // Fixed stripes bound lock bookkeeping without serializing every image behind one lock.
    let _guard = cache_lock(&key)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Ok(record) = read_receipt(root, &key, implementation)
        && let Ok(replacement) = read_replacement(root, &key, &record, asset.master_bytes.len())
    {
        COMPACT_HITS.fetch_add(1, Ordering::Relaxed);
        crate::cache::mark_used(&root.join(format!("{key}.json")));
        let mut restored = asset;
        if let Some((bytes, extension, width, height, placeholder)) = replacement {
            restored.master_hash = crate::model::sha1_hex(&bytes);
            restored.master_bytes = bytes;
            restored.master_extension = extension;
            restored.width = width;
            restored.height = height;
            restored.placeholder = placeholder;
        }
        if record.clear_renditions {
            restored.renditions.clear();
        }
        return Ok(restored);
    }
    let original_hash = hex::encode(Sha256::digest(&asset.master_bytes));
    let original_size = asset.master_bytes.len();
    COMPACT_MISSES.fetch_add(1, Ordering::Relaxed);
    let output = compressor(asset)?;
    if output.master_bytes.len() < original_size {
        ENCODED_BYTES.fetch_add(output.master_bytes.len() as u64, Ordering::Relaxed);
    }
    if let Err(error) = save_receipt(root, &key, implementation, &original_hash, &output) {
        log::debug!("could not cache compressed deployment image: {error:#}");
    }
    Ok(output)
}

fn cache_lock(key: &str) -> &'static Mutex<()> {
    static LOCKS: OnceLock<[Mutex<()>; CACHE_LOCK_STRIPES]> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| std::array::from_fn(|_| Mutex::new(())));
    let stripe = key.bytes().fold(0usize, |stripe, byte| {
        (stripe.wrapping_mul(31) + usize::from(byte)) % CACHE_LOCK_STRIPES
    });
    &locks[stripe]
}

pub(crate) fn implementation_key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| {
        let mut hash = Sha256::new();
        for source in [
            POLICY,
            include_str!("compressed_media.rs"),
            include_str!("../media/compact.rs"),
            include_str!("../media/placeholder.rs"),
            include_str!("../media.rs"),
            crate::media::codec_fingerprint(),
        ] {
            hash.update(source.as_bytes());
        }
        hex::encode(hash.finalize())
    })
}

fn cache_key(asset: &Asset, implementation: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(implementation.as_bytes());
    hash.update(b"\0");
    hash.update(&asset.master_bytes);
    hex::encode(hash.finalize())
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() <= limit as u64,
        "invalid deployment cache file"
    );
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "deployment cache file exceeds limit");
    Ok(bytes)
}

fn read_receipt(root: &Path, key: &str, implementation: &str) -> Result<Record> {
    let bytes = read_bounded(&root.join(format!("{key}.json")), MAX_RECEIPT_BYTES)?;
    let receipt: Receipt = serde_json::from_slice(&bytes)?;
    ensure!(
        receipt.record.key == key
            && receipt.record.policy == POLICY
            && receipt.record.implementation == implementation,
        "stale deployment cache receipt"
    );
    ensure!(
        receipt.checksum == hex::encode(Sha256::digest(serde_json::to_vec(&receipt.record)?)),
        "corrupt deployment cache receipt"
    );
    Ok(receipt.record)
}

type CachedImage = (Vec<u8>, &'static str, u32, u32, placeholder::Placeholder);

fn read_replacement(
    root: &Path,
    key: &str,
    record: &Record,
    original_bytes: usize,
) -> Result<Option<CachedImage>> {
    let Some(image) = &record.replacement else {
        return Ok(None);
    };
    let extension = match image.extension.as_str() {
        "jpg" => "jpg",
        "png" => "png",
        _ => anyhow::bail!("invalid compressed deployment format"),
    };
    ensure!(
        image.bytes > 0
            && image.bytes < original_bytes
            && image.bytes <= MediaLimits::default().max_file_bytes,
        "invalid compressed deployment size"
    );
    ensure!(
        (1..=MAX_AXIS).contains(&image.width) && (1..=MAX_AXIS).contains(&image.height),
        "invalid compressed deployment dimensions"
    );
    let bytes = read_bounded(&root.join(format!("{key}.image")), image.bytes)?;
    ensure!(
        bytes.len() == image.bytes && hex::encode(Sha256::digest(&bytes)) == image.sha256,
        "corrupt compressed deployment bytes"
    );
    let expected_format = if extension == "jpg" {
        ImageFormat::Jpeg
    } else {
        ImageFormat::Png
    };
    ensure!(
        image::guess_format(&bytes)? == expected_format,
        "incorrect compressed deployment format"
    );
    let mut reader = ImageReader::with_format(Cursor::new(&bytes), expected_format);
    reader.limits(decoder_limits());
    ensure!(
        reader.into_dimensions()? == (image.width, image.height),
        "incorrect compressed deployment dimensions"
    );
    let placeholder = placeholder::from_hash(&image.thumbhash)?;
    Ok(Some((
        bytes,
        extension,
        image.width,
        image.height,
        placeholder,
    )))
}

fn save_receipt(
    root: &Path,
    key: &str,
    implementation: &str,
    original_hash: &str,
    output: &Asset,
) -> Result<()> {
    std::fs::create_dir_all(root)?;
    let output_hash = hex::encode(Sha256::digest(&output.master_bytes));
    let replacement = if output_hash != original_hash {
        atomic_write(root, &format!("{key}.image"), &output.master_bytes)?;
        Some(Replacement {
            sha256: output_hash,
            bytes: output.master_bytes.len(),
            extension: output.master_extension.into(),
            width: output.width,
            height: output.height,
            thumbhash: output.placeholder.hash.clone(),
        })
    } else {
        None
    };
    let record = Record {
        key: key.into(),
        policy: POLICY.into(),
        implementation: implementation.into(),
        clear_renditions: output.renditions.is_empty(),
        replacement,
    };
    let checksum = hex::encode(Sha256::digest(serde_json::to_vec(&record)?));
    let bytes = serde_json::to_vec(&Receipt { checksum, record })?;
    ensure!(
        bytes.len() <= MAX_RECEIPT_BYTES,
        "deployment cache receipt exceeds limit"
    );
    atomic_write(root, &format!("{key}.json"), &bytes)?;
    crate::cache::mark_used(&root.join(format!("{key}.json")));
    Ok(())
}

fn atomic_write(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    file.write_all(bytes)?;
    file.persist(root.join(name))?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponsiveReceipt {
    checksum: String,
    record: ResponsiveRecord,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponsiveRecord {
    key: String,
    implementation: String,
    payload_sha256: String,
    renditions: Vec<ResponsivePart>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponsivePart {
    bytes: usize,
    sha1: String,
    width: u32,
    height: u32,
}

/// Original-image subscriptions derive responsive copies only when they publish them. Encoded
/// copies belong to the disposable media cache, never to the append-only archive.
pub(crate) fn responsive_cached(asset: Asset, root: Option<&Path>) -> Result<Asset> {
    responsive_with(asset, root, crate::media::derive_renditions)
}

fn responsive_with(
    mut asset: Asset,
    root: Option<&Path>,
    derive: impl FnOnce(&Asset) -> Result<Vec<crate::media::Rendition>>,
) -> Result<Asset> {
    if !asset.renditions.is_empty() {
        return Ok(asset);
    }
    let Some(root) = root else {
        asset.renditions = derive(&asset)?;
        return Ok(asset);
    };
    let mut hash = Sha256::new();
    hash.update(b"responsive-v1\0");
    hash.update(implementation_key());
    hash.update(&asset.master_bytes);
    let key = hex::encode(hash.finalize());
    let _guard = cache_lock(&key)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Ok(renditions) = read_responsive(root, &key, &asset) {
        RESPONSIVE_HITS.fetch_add(1, Ordering::Relaxed);
        crate::cache::mark_used(&root.join(format!("{key}.json")));
        asset.renditions = renditions;
        return Ok(asset);
    }
    RESPONSIVE_MISSES.fetch_add(1, Ordering::Relaxed);
    asset.renditions = derive(&asset)?;
    ENCODED_BYTES.fetch_add(
        asset
            .renditions
            .iter()
            .map(|image| image.bytes.len() as u64)
            .sum::<u64>(),
        Ordering::Relaxed,
    );
    if let Err(error) = save_responsive(root, &key, &asset.renditions) {
        log::debug!("could not cache responsive images: {error:#}");
    }
    Ok(asset)
}

fn read_responsive(root: &Path, key: &str, asset: &Asset) -> Result<Vec<crate::media::Rendition>> {
    let bytes = read_bounded(&root.join(format!("{key}.json")), MAX_RECEIPT_BYTES)?;
    let receipt: ResponsiveReceipt = serde_json::from_slice(&bytes)?;
    let record = receipt.record;
    ensure!(
        record.key == key && record.implementation == implementation_key(),
        "stale responsive cache receipt"
    );
    ensure!(
        receipt.checksum == hex::encode(Sha256::digest(serde_json::to_vec(&record)?)),
        "corrupt responsive receipt"
    );
    ensure!(
        record.renditions.len() <= crate::media::MAX_STORED_RENDITIONS,
        "too many responsive images"
    );
    let limit = MediaLimits::default();
    let total = record
        .renditions
        .iter()
        .try_fold(0usize, |total, part| total.checked_add(part.bytes))
        .context("responsive payload overflow")?;
    ensure!(
        total <= limit.max_article_bytes,
        "responsive payload exceeds byte limit"
    );
    let payload = if total == 0 {
        Vec::new()
    } else {
        read_bounded(&root.join(format!("{key}.image")), total)?
    };
    ensure!(
        payload.len() == total && hex::encode(Sha256::digest(&payload)) == record.payload_sha256,
        "corrupt responsive payload"
    );
    let mut offset = 0;
    let mut previous_width = 0;
    let mut restored = Vec::with_capacity(record.renditions.len());
    for part in record.renditions {
        ensure!(
            part.bytes > 0
                && part.bytes <= limit.max_file_bytes
                && part.width > previous_width
                && part.width <= asset.width
                && part.height > 0
                && part.height <= asset.height,
            "invalid responsive metadata"
        );
        let bytes = &payload[offset..offset + part.bytes];
        ensure!(
            crate::model::sha1_hex(bytes) == part.sha1,
            "responsive identity mismatch"
        );
        ensure!(
            image::guess_format(bytes)? == ImageFormat::WebP,
            "invalid responsive format"
        );
        let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::WebP);
        reader.limits(decoder_limits());
        ensure!(
            reader.into_dimensions()? == (part.width, part.height),
            "responsive geometry mismatch"
        );
        previous_width = part.width;
        offset += part.bytes;
        restored.push(crate::media::Rendition {
            bytes: bytes.to_vec(),
            extension: "webp",
            hash: part.sha1,
            width: part.width,
            height: part.height,
        });
    }
    Ok(restored)
}

fn save_responsive(root: &Path, key: &str, renditions: &[crate::media::Rendition]) -> Result<()> {
    std::fs::create_dir_all(root)?;
    let payload: Vec<_> = renditions
        .iter()
        .flat_map(|image| image.bytes.iter().copied())
        .collect();
    let record = ResponsiveRecord {
        key: key.into(),
        implementation: implementation_key().into(),
        payload_sha256: hex::encode(Sha256::digest(&payload)),
        renditions: renditions
            .iter()
            .map(|image| ResponsivePart {
                bytes: image.bytes.len(),
                sha1: image.hash.clone(),
                width: image.width,
                height: image.height,
            })
            .collect(),
    };
    let checksum = hex::encode(Sha256::digest(serde_json::to_vec(&record)?));
    let receipt = serde_json::to_vec(&ResponsiveReceipt { checksum, record })?;
    ensure!(
        receipt.len() <= MAX_RECEIPT_BYTES,
        "responsive receipt exceeds size limit"
    );
    if !payload.is_empty() {
        atomic_write(root, &format!("{key}.image"), &payload)?;
    }
    atomic_write(root, &format!("{key}.json"), &receipt)?;
    crate::cache::mark_used(&root.join(format!("{key}.json")));
    Ok(())
}

/// Keep source identity while replacing only the deployment bytes. Animation and color-managed
/// or high-depth images retain their original representation rather than losing information.
pub(super) fn compact(mut asset: Asset) -> Result<Asset> {
    let budget = MediaLimits::default();
    ensure!(
        asset.master_bytes.len() <= budget.max_file_bytes,
        "archived image exceeds deployment decoder byte limit"
    );
    let format = image::guess_format(&asset.master_bytes)
        .context("recognizing archived image for deployment compression")?;
    // An AVIF is already compact, and there is no decoder to re-encode it with.
    if format == ImageFormat::Avif || preserve_animation(&asset.master_bytes, format)? {
        return Ok(asset);
    }
    let mut reader = ImageReader::with_format(Cursor::new(&asset.master_bytes), format);
    reader.limits(decoder_limits());
    let mut decoder = reader
        .into_decoder()
        .context("reading archived image for deployment compression")?;
    let (width, height) = decoder.dimensions();
    ensure!(
        width > 0 && height > 0 && u64::from(width) * u64::from(height) <= budget.max_pixels,
        "archived image exceeds deployment decoder pixel limit"
    );
    if !matches!(
        decoder.color_type(),
        image::ColorType::L8
            | image::ColorType::La8
            | image::ColorType::Rgb8
            | image::ColorType::Rgba8
    ) || decoder
        .icc_profile()
        .map_or(true, |profile| profile.is_some())
    {
        drop(decoder);
        return Ok(asset);
    }
    let orientation = decoder
        .orientation()
        .context("reading archived image orientation for deployment")?;
    let mut decoded = DynamicImage::from_decoder(decoder)
        .context("decoding archived image for deployment compression")?;
    decoded.apply_orientation(orientation);
    compact::apply(&mut asset, &decoded, DEPLOYMENT_POLICY)?;
    Ok(asset)
}

fn decoder_limits() -> image::Limits {
    let budget = MediaLimits::default();
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(budget.max_axis);
    limits.max_image_height = Some(budget.max_axis);
    limits.max_alloc = Some(budget.max_pixels.saturating_mul(8).min(768 * 1024 * 1024));
    limits
}

fn preserve_animation(bytes: &[u8], format: ImageFormat) -> Result<bool> {
    match format {
        // Even single-frame GIFs stay exact; this avoids decoding frames just to classify them.
        ImageFormat::Gif => Ok(true),
        ImageFormat::Png => {
            let decoder =
                image::codecs::png::PngDecoder::with_limits(Cursor::new(bytes), decoder_limits())
                    .context("reading deployment PNG animation metadata")?;
            decoder
                .is_apng()
                .context("checking deployment PNG animation")
        }
        ImageFormat::WebP => {
            let mut decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
                .context("reading deployment WebP animation metadata")?;
            decoder.set_limits(decoder_limits())?;
            Ok(decoder.has_animation())
        }
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageEncoder as _, Rgba};

    fn asset(bytes: Vec<u8>) -> Asset {
        let image = image::load_from_memory(&bytes).unwrap();
        let extension = match image::guess_format(&bytes).unwrap() {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Gif => "gif",
            ImageFormat::WebP => "webp",
            _ => panic!("unsupported fixture image"),
        };
        Asset {
            source_url: "https://example.com/image.png".into(),
            source_hash: "source-identity".into(),
            alt: Some("An article illustration".into()),
            master_hash: crate::model::sha1_hex(&bytes),
            master_bytes: bytes,
            master_extension: extension,
            width: image.width(),
            height: image.height(),
            dominant_color: "#808080".into(),
            placeholder: placeholder::from_image(&image).unwrap(),
            renditions: vec![],
        }
    }

    fn png(image: &DynamicImage, compression: image::codecs::png::CompressionType) -> Vec<u8> {
        let rgba = image.to_rgba8();
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new_with_quality(
            &mut bytes,
            compression,
            image::codecs::png::FilterType::NoFilter,
        )
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
        bytes
    }

    fn cache_fixture() -> Asset {
        let image = DynamicImage::ImageRgba8(ImageBuffer::from_fn(80, 48, |x, y| {
            Rgba([(x * 3) as u8, (y * 5) as u8, 80, 255])
        }));
        asset(png(
            &image,
            image::codecs::png::CompressionType::Uncompressed,
        ))
    }

    #[test]
    fn responsive_cache_reuses_encoded_copies_and_recovers_from_corruption() {
        let root = tempfile::tempdir().unwrap();
        let image = DynamicImage::ImageRgba8(ImageBuffer::from_fn(720, 480, |x, y| {
            Rgba([(x % 256) as u8, (y % 256) as u8, 80, 255])
        }));
        let original = asset(png(
            &image,
            image::codecs::png::CompressionType::Uncompressed,
        ));
        let first = responsive_cached(original.clone(), Some(root.path())).unwrap();
        assert!(!first.renditions.is_empty());
        assert_eq!(first.master_bytes, original.master_bytes);
        let second = responsive_with(original.clone(), Some(root.path()), |_| {
            anyhow::bail!("cache hit must not encode")
        })
        .unwrap();
        assert_eq!(first, second);
        let payload = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "image")
            })
            .unwrap();
        std::fs::write(payload, b"corrupt").unwrap();
        let repaired = responsive_cached(original, Some(root.path())).unwrap();
        assert_eq!(repaired, first);
    }

    #[test]
    fn responsive_cache_remembers_images_without_useful_renditions() {
        let root = tempfile::tempdir().unwrap();
        let original = cache_fixture();
        let first =
            responsive_with(original.clone(), Some(root.path()), |_| Ok(Vec::new())).unwrap();
        assert!(first.renditions.is_empty());
        let second = responsive_with(original, Some(root.path()), |_| {
            anyhow::bail!("negative cache hit must not decode")
        })
        .unwrap();
        assert_eq!(second, first);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn cached_bytes_avoid_a_second_decode_and_compression() {
        let directory = tempfile::tempdir().unwrap();
        let original = cache_fixture();
        let first = compact_with(
            original.clone(),
            Some(directory.path()),
            "policy-a",
            compact,
        )
        .unwrap();
        assert!(first.master_bytes.len() < original.master_bytes.len());
        let second = compact_with(original, Some(directory.path()), "policy-a", |_| {
            anyhow::bail!("a cache hit must not invoke the compressor")
        })
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn concurrent_workers_compress_the_same_image_once() {
        use std::sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        };

        let directory = tempfile::tempdir().unwrap();
        let original = cache_fixture();
        let start = Barrier::new(4);
        let calls = AtomicUsize::new(0);
        let outputs = std::thread::scope(|scope| {
            let workers = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        compact_with(
                            original.clone(),
                            Some(directory.path()),
                            "concurrent",
                            |asset| {
                                calls.fetch_add(1, Ordering::SeqCst);
                                // Keep the cache miss open long enough for other workers to contend.
                                std::thread::sleep(std::time::Duration::from_millis(30));
                                compact(asset)
                            },
                        )
                        .unwrap()
                    })
                })
                .collect::<Vec<_>>();
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(outputs.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn cached_no_op_still_removes_redundant_renditions() {
        let directory = tempfile::tempdir().unwrap();
        let image = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(1, 1, Rgba([1, 2, 3, 255])));
        let mut original = asset(png(&image, image::codecs::png::CompressionType::Best));
        let first = compact_with(
            original.clone(),
            Some(directory.path()),
            "no-growth",
            compact,
        )
        .unwrap();
        assert_eq!(first.master_bytes, original.master_bytes);
        // A later restore can include optional renditions that were unavailable on the first run.
        original.renditions.push(crate::media::Rendition {
            bytes: vec![0; 500],
            extension: "webp",
            hash: "old".into(),
            width: 1,
            height: 1,
        });
        let second = compact_with(original, Some(directory.path()), "no-growth", |_| {
            anyhow::bail!("receipt must remember removal independently of replacement bytes")
        })
        .unwrap();
        assert_eq!(second, first);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn no_op_receipts_avoid_redecoding_without_copying_masters() {
        let directory = tempfile::tempdir().unwrap();
        let mut bytes = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut bytes)
            .encode_frame(image::Frame::new(ImageBuffer::from_pixel(
                16,
                16,
                Rgba([10, 20, 30, 255]),
            )))
            .unwrap();
        let original = asset(bytes);
        let first = compact_with(
            original.clone(),
            Some(directory.path()),
            "policy-a",
            compact,
        )
        .unwrap();
        let second = compact_with(original.clone(), Some(directory.path()), "policy-a", |_| {
            anyhow::bail!("a no-op receipt must not invoke the compressor")
        })
        .unwrap();
        assert_eq!(first, original);
        assert_eq!(second, original);
        let entries = std::fs::read_dir(directory.path())
            .unwrap()
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0]
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
        );
    }

    #[test]
    fn corrupt_and_oversized_cached_bytes_are_recomputed() {
        let directory = tempfile::tempdir().unwrap();
        let original = cache_fixture();
        let first = compact_with(
            original.clone(),
            Some(directory.path()),
            "policy-a",
            compact,
        )
        .unwrap();
        let file = directory
            .path()
            .join(format!("{}.image", cache_key(&original, "policy-a")));
        for oversized in [false, true] {
            if oversized {
                std::fs::File::create(&file)
                    .unwrap()
                    .set_len(MediaLimits::default().max_file_bytes as u64 + 1)
                    .unwrap();
            } else {
                std::fs::write(&file, b"corrupt").unwrap();
            }
            let mut recomputed = false;
            let result = compact_with(
                original.clone(),
                Some(directory.path()),
                "policy-a",
                |asset| {
                    recomputed = true;
                    compact(asset)
                },
            )
            .unwrap();
            assert!(recomputed);
            assert_eq!(result, first);
        }
    }

    #[test]
    fn stale_policy_and_corrupt_metadata_cannot_hit() {
        let directory = tempfile::tempdir().unwrap();
        let original = cache_fixture();
        let expected = compact_with(
            original.clone(),
            Some(directory.path()),
            "policy-a",
            compact,
        )
        .unwrap();
        let mut recomputed = 0;
        let changed = compact_with(
            original.clone(),
            Some(directory.path()),
            "policy-b",
            |asset| {
                recomputed += 1;
                compact(asset)
            },
        )
        .unwrap();
        assert_eq!(changed, expected);
        let file = directory
            .path()
            .join(format!("{}.json", cache_key(&original, "policy-b")));
        let mut receipt: Receipt = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        receipt.record.replacement.as_mut().unwrap().width = 0;
        std::fs::write(&file, serde_json::to_vec(&receipt).unwrap()).unwrap();
        let repaired = compact_with(original, Some(directory.path()), "policy-b", |asset| {
            recomputed += 1;
            compact(asset)
        })
        .unwrap();
        assert_eq!(repaired, expected);
        assert_eq!(recomputed, 2);
    }

    #[test]
    fn unavailable_cache_does_not_fail_compression() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let original = cache_fixture();
        assert_eq!(
            compact_cached(original.clone(), Some(file.path())).unwrap(),
            compact(original).unwrap()
        );
    }

    #[test]
    fn an_avif_master_is_deployed_as_it_was_archived() {
        // Only the `ftyp` brand: nothing past it is read.
        let mut bytes = 20u32.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"ftypavif\0\0\0\0avif");
        let original = Asset {
            master_hash: crate::model::sha1_hex(&bytes),
            master_bytes: bytes,
            master_extension: "avif",
            width: 1200,
            height: 800,
            ..cache_fixture()
        };
        assert_eq!(compact(original.clone()).unwrap(), original);
    }

    #[test]
    fn opaque_images_shrink_without_losing_source_identity_or_geometry() {
        let image = DynamicImage::ImageRgba8(ImageBuffer::from_fn(2000, 1000, |x, y| {
            Rgba([(x * 255 / 1999) as u8, (y * 255 / 999) as u8, 90, 255])
        }));
        let original = asset(png(
            &image,
            image::codecs::png::CompressionType::Uncompressed,
        ));
        let output = compact(original.clone()).unwrap();
        assert_eq!(output.master_extension, "jpg");
        assert_eq!((output.width, output.height), (1600, 800));
        assert!(output.master_bytes.len() < original.master_bytes.len());
        assert_eq!(output.source_url, original.source_url);
        assert_eq!(output.source_hash, original.source_hash);
        assert_eq!(output.alt, original.alt);
        assert_eq!(
            output.master_hash,
            crate::model::sha1_hex(&output.master_bytes)
        );
        let decoded = image::load_from_memory(&output.master_bytes).unwrap();
        assert_eq!(
            output.placeholder,
            placeholder::from_image(&decoded).unwrap()
        );
        let expected = image
            .resize(1600, 1600, image::imageops::FilterType::Lanczos3)
            .to_rgb8();
        let actual = decoded.to_rgb8();
        let mean_error = expected
            .as_raw()
            .iter()
            .zip(actual.as_raw())
            .map(|(a, b)| a.abs_diff(*b) as u64)
            .sum::<u64>()
            / expected.as_raw().len() as u64;
        assert!(mean_error < 5, "JPEG color error: {mean_error}");
        assert_eq!(
            image::load_from_memory(&original.master_bytes)
                .unwrap()
                .width(),
            2000
        );
    }

    #[test]
    fn transparency_is_preserved_exactly_without_resizing() {
        let image = DynamicImage::ImageRgba8(ImageBuffer::from_fn(128, 64, |x, _| {
            Rgba([200, 50, 80, (x * 2) as u8])
        }));
        let original = asset(png(&image, image::codecs::png::CompressionType::Fast));
        let output = compact(original.clone()).unwrap();
        assert_eq!(output.master_extension, "png");
        assert!(output.master_bytes.len() <= original.master_bytes.len());
        assert_eq!(
            image::load_from_memory(&output.master_bytes)
                .unwrap()
                .to_rgba8(),
            image.to_rgba8()
        );
    }

    #[test]
    fn tiny_masters_never_grow_and_obsolete_renditions_are_removed() {
        let image = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(1, 1, Rgba([1, 2, 3, 255])));
        let mut original = asset(png(&image, image::codecs::png::CompressionType::Best));
        original.renditions.push(crate::media::Rendition {
            bytes: vec![0; 500],
            extension: "webp",
            hash: "old".into(),
            width: 1,
            height: 1,
        });
        let output = compact(original.clone()).unwrap();
        assert_eq!(output.master_bytes, original.master_bytes);
        assert_eq!(output.master_hash, original.master_hash);
        assert_eq!(output.placeholder, original.placeholder);
        assert!(output.renditions.is_empty());
    }

    #[test]
    fn animated_gifs_and_pngs_are_retained_byte_for_byte() {
        let mut gif = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut gif)
            .encode_frames([
                image::Frame::new(ImageBuffer::from_pixel(16, 16, Rgba([200, 0, 0, 255]))),
                image::Frame::new(ImageBuffer::from_pixel(16, 16, Rgba([0, 200, 0, 255]))),
            ])
            .unwrap();
        let mut apng = Vec::new();
        {
            let mut encoder = ::png::Encoder::new(&mut apng, 16, 16);
            encoder.set_color(::png::ColorType::Rgba);
            encoder.set_depth(::png::BitDepth::Eight);
            encoder.set_animated(2, 0).unwrap();
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&vec![100; 16 * 16 * 4]).unwrap();
            writer.write_image_data(&vec![200; 16 * 16 * 4]).unwrap();
            writer.finish().unwrap();
        }
        for bytes in [gif, apng] {
            let original = asset(bytes);
            assert_eq!(compact(original.clone()).unwrap(), original);
        }
    }

    #[test]
    fn high_depth_and_color_profiles_are_preserved() {
        let high_depth = DynamicImage::ImageRgb16(ImageBuffer::from_fn(128, 64, |x, y| {
            image::Rgb([(x * 503 + y * 7) as u16, (y * 911 + x * 3) as u16, 65001])
        }));
        let mut high_depth_bytes = Cursor::new(Vec::new());
        high_depth
            .write_to(&mut high_depth_bytes, ImageFormat::Png)
            .unwrap();
        let mut profiled = Vec::new();
        let mut encoder = image::codecs::png::PngEncoder::new_with_quality(
            &mut profiled,
            image::codecs::png::CompressionType::Uncompressed,
            image::codecs::png::FilterType::NoFilter,
        );
        encoder
            .set_icc_profile(b"preserved test profile".to_vec())
            .unwrap();
        encoder
            .write_image(&[100; 32 * 16 * 3], 32, 16, image::ExtendedColorType::Rgb8)
            .unwrap();
        for bytes in [high_depth_bytes.into_inner(), profiled] {
            let original = asset(bytes);
            assert_eq!(compact(original.clone()).unwrap(), original);
        }
    }

    #[test]
    fn invalid_bytes_report_the_failed_operation() {
        let image = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(1, 1, Rgba([1, 2, 3, 255])));
        let mut original = asset(png(&image, image::codecs::png::CompressionType::Fast));
        original.master_bytes = b"invalid image".to_vec();
        assert!(
            compact(original)
                .unwrap_err()
                .to_string()
                .contains("recognizing archived image")
        );
    }
}
