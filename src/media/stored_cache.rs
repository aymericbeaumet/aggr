//! Local validation receipts for retained images. A build that meets the same master and
//! rendition bytes again skips their decode and resize; placeholders derived from content-addressed
//! bytes are remembered in a sidecar next to the receipts.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{
    Asset, MAX_STORED_RENDITIONS, MediaLimits, Rendition, StoredKind, implementation_fingerprint,
    placeholder, stored_identity_with_hash, validate_stored_dimensions,
};
use crate::model::ArticleImage;

const MAX_ASSET_RECEIPT_BYTES: usize = 8 * 1024;
const ASSET_RECEIPT_SLOTS: usize = 16_384;
const ASSET_RECEIPT_WAYS: usize = 4;
/// Sidecar directory for placeholder receipts inside the validated-images namespace. Additive:
/// asset receipts keep their slot files at the namespace root, so existing caches stay valid.
const PLACEHOLDER_RECEIPTS_DIR: &str = "placeholders-v1";

/// Local validation receipts contain no master pixels. Fixed slots bound disk usage without
/// scanning a large cache; collisions only cause a fresh validation, never incorrect reuse.
#[derive(Clone)]
pub(crate) struct StoredAssetCache {
    root: PathBuf,
    slots: usize,
}

/// One set-associative directory of `{slot:04x}.json` receipts, each checksummed and bound to
/// the key that produced it.
struct ReceiptSlots {
    root: PathBuf,
    capacity: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt<T> {
    key: String,
    checksum: String,
    value: T,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidatedAsset {
    color: String,
    master_hash: String,
    variants: Vec<CachedRendition>,
    thumbhash: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CachedRendition {
    index: usize,
    hash: String,
}

/// A ThumbHash derived from content-addressed bytes; the inline preview is re-derived from it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CachedPlaceholder {
    thumbhash: String,
}

/// Receipt key for a placeholder derived from the bytes identified by `content_hash`, bound to
/// the media implementation so a changed placeholder algorithm never reuses stale hashes.
fn placeholder_receipt_key(content_hash: &str) -> Result<String> {
    ensure!(
        (1..=128).contains(&content_hash.len())
            && content_hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid placeholder content hash"
    );
    let mut hash = Sha256::new();
    hash.update(implementation_fingerprint());
    hash.update(b"placeholder\n");
    hash.update(content_hash.as_bytes());
    Ok(hex::encode(hash.finalize()))
}

impl StoredAssetCache {
    pub(crate) fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: crate::cache::Namespace::ValidatedImages.dir(root.as_ref()),
            slots: ASSET_RECEIPT_SLOTS,
        }
    }

    pub(crate) fn restore(
        &self,
        metadata: &ArticleImage,
        master: Vec<u8>,
        mut variants: Vec<Vec<u8>>,
    ) -> Result<Asset> {
        let key = stored_asset_key(metadata, &master, &variants);
        if let Ok(key) = &key
            && let Ok(value) = self.asset_receipts().read::<ValidatedAsset>(key)
            && let Ok(placeholder) = placeholder::from_hash(&value.thumbhash)
            && let Ok((extension, hash, mut renditions)) = value.parts(metadata, &master, &variants)
        {
            for (cached, rendition) in value.variants.iter().zip(&mut renditions) {
                rendition.bytes = std::mem::take(&mut variants[cached.index]);
            }
            return Ok(Asset {
                source_url: metadata.source.clone(),
                source_hash: crate::model::sha1_hex(metadata.source.as_bytes()),
                alt: None,
                master_bytes: master,
                master_extension: extension,
                master_hash: hash,
                width: metadata.original.width,
                height: metadata.original.height,
                dominant_color: value.color,
                placeholder,
                renditions,
            });
        }
        // Keep validation and fallback semantics identical when caching is unavailable or stale.
        let input_hashes = variants
            .iter()
            .map(|bytes| hex::encode(Sha256::digest(bytes)))
            .collect::<Vec<_>>();
        let asset = Asset::from_stored(metadata, master, variants)?;
        if let Ok(key) = key {
            let _ = self.write(&key, &asset, metadata, &input_hashes);
        }
        Ok(asset)
    }

    fn asset_receipts(&self) -> ReceiptSlots {
        ReceiptSlots {
            root: self.root.clone(),
            capacity: self.slots,
        }
    }

    fn placeholder_receipts(&self) -> ReceiptSlots {
        ReceiptSlots {
            root: self.root.join(PLACEHOLDER_RECEIPTS_DIR),
            capacity: self.slots,
        }
    }

    /// A placeholder previously derived from the bytes identified by `content_hash` (their
    /// `model::sha1_hex`), or `None` when it must be computed again. The inline preview is
    /// re-derived from the stored ThumbHash, so a corrupt receipt only costs a fresh decode.
    pub(crate) fn cached_placeholder(
        &self,
        content_hash: &str,
    ) -> Option<placeholder::Placeholder> {
        let key = placeholder_receipt_key(content_hash).ok()?;
        let value = self
            .placeholder_receipts()
            .read::<CachedPlaceholder>(&key)
            .ok()?;
        placeholder::from_hash(&value.thumbhash).ok()
    }

    /// Remember a placeholder derived from the bytes identified by `content_hash`. The cache is
    /// disposable, so a failed write is ignored and never affects the build.
    pub(crate) fn remember_placeholder(
        &self,
        content_hash: &str,
        placeholder: &placeholder::Placeholder,
    ) {
        if let Ok(key) = placeholder_receipt_key(content_hash) {
            let value = CachedPlaceholder {
                thumbhash: placeholder.hash.clone(),
            };
            let _ = self.placeholder_receipts().write(&key, &value);
        }
    }

    fn write(
        &self,
        key: &str,
        asset: &Asset,
        metadata: &ArticleImage,
        input_hashes: &[String],
    ) -> Result<()> {
        let mut variants = Vec::new();
        for rendition in &asset.renditions {
            let input_hash = hex::encode(Sha256::digest(&rendition.bytes));
            if let Some(index) = input_hashes.iter().enumerate().position(|(index, hash)| {
                hash == &input_hash
                    && metadata.variants.get(index).is_some_and(|file| {
                        (file.width, file.height) == (rendition.width, rendition.height)
                    })
            }) {
                variants.push(CachedRendition {
                    index,
                    hash: rendition.hash.clone(),
                });
            }
        }
        let value = ValidatedAsset {
            color: asset.dominant_color.clone(),
            master_hash: asset.master_hash.clone(),
            variants,
            thumbhash: asset.placeholder.hash.clone(),
        };
        self.asset_receipts().write(key, &value)
    }
}

impl ReceiptSlots {
    fn paths(&self, key: &str) -> Result<Vec<PathBuf>> {
        let prefix = key.get(..4).context("invalid image cache key")?;
        let capacity = self.capacity.max(1);
        let bucket =
            usize::from(u16::from_str_radix(prefix, 16)?) % capacity.div_ceil(ASSET_RECEIPT_WAYS);
        let first = bucket * ASSET_RECEIPT_WAYS;
        Ok((first..(first + ASSET_RECEIPT_WAYS).min(capacity))
            .map(|slot| self.root.join(format!("{slot:04x}.json")))
            .collect())
    }

    fn read<T: Serialize + DeserializeOwned>(&self, key: &str) -> Result<T> {
        self.paths(key)?
            .into_iter()
            .find_map(|path| Self::read_slot::<T>(&path, key).ok())
            .context("image validation receipt missing")
    }

    fn read_slot<T: Serialize + DeserializeOwned>(path: &Path, key: &str) -> Result<T> {
        let receipt = Self::receipt::<T>(path)?;
        ensure!(receipt.key == key, "image cache slot changed");
        Ok(receipt.value)
    }

    fn receipt<T: Serialize + DeserializeOwned>(path: &Path) -> Result<Receipt<T>> {
        let metadata = std::fs::symlink_metadata(path)?;
        ensure!(
            metadata.is_file() && metadata.len() <= MAX_ASSET_RECEIPT_BYTES as u64,
            "invalid image cache receipt"
        );
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take((MAX_ASSET_RECEIPT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= MAX_ASSET_RECEIPT_BYTES,
            "image cache receipt too large"
        );
        let receipt: Receipt<T> = serde_json::from_slice(&bytes)?;
        ensure!(
            receipt.checksum == hex::encode(Sha256::digest(serde_json::to_vec(&receipt.value)?)),
            "image cache receipt checksum changed"
        );
        Ok(receipt)
    }

    /// The slot to overwrite for `key`: its own slot, an unreadable one, or the least recently
    /// written one in its set. The same receipt type must be used to judge readability.
    fn replacement<T: Serialize + DeserializeOwned>(&self, key: &str) -> Result<PathBuf> {
        let mut oldest = None;
        for path in self.paths(key)? {
            match Self::receipt::<T>(&path) {
                Ok(receipt) if receipt.key != key => {}
                _ => return Ok(path),
            }
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                return Ok(path);
            };
            let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
            if oldest.as_ref().is_none_or(|(_, time)| modified < *time) {
                oldest = Some((path, modified));
            }
        }
        oldest
            .map(|(path, _)| path)
            .context("image cache has no slots")
    }

    fn write<T: Serialize + DeserializeOwned>(&self, key: &str, value: &T) -> Result<()> {
        let checksum = hex::encode(Sha256::digest(serde_json::to_vec(value)?));
        let bytes = serde_json::to_vec(&Receipt {
            key: key.to_string(),
            checksum,
            value,
        })?;
        ensure!(
            bytes.len() <= MAX_ASSET_RECEIPT_BYTES,
            "image validation receipt too large"
        );
        std::fs::create_dir_all(&self.root)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        temporary.write_all(&bytes)?;
        temporary.persist(self.replacement::<T>(key)?)?;
        Ok(())
    }
}

impl ValidatedAsset {
    fn parts(
        &self,
        metadata: &ArticleImage,
        master: &[u8],
        variants: &[Vec<u8>],
    ) -> Result<(&'static str, String, Vec<Rendition>)> {
        let limits = MediaLimits::default();
        let (stem, _) = metadata
            .original
            .file
            .rsplit_once(".image-")
            .context("invalid image owner")?;
        ensure!(
            metadata.is_valid_for(stem) && variants.len() == metadata.variants.len(),
            "invalid cached image metadata"
        );
        ensure!(
            variants.len() <= MAX_STORED_RENDITIONS,
            "too many cached renditions"
        );
        ensure!(
            self.color.len() == 7
                && self.color.starts_with('#')
                && self.color[1..].bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid cached image color"
        );
        ensure!(
            metadata
                .color
                .as_ref()
                .is_none_or(|color| color == &self.color),
            "cached image color changed"
        );
        validate_stored_dimensions(metadata.original.width, metadata.original.height, &limits)?;
        let (_, extension, hash) = stored_identity_with_hash(
            master,
            &metadata.original,
            StoredKind::Master,
            &limits,
            self.master_hash.clone(),
        )?;
        ensure!(
            self.variants
                .windows(2)
                .all(|indices| indices[0].index < indices[1].index),
            "invalid cached rendition order"
        );
        let mut retained = master.len();
        let mut renditions = Vec::new();
        for cached in &self.variants {
            let index = cached.index;
            let bytes = variants
                .get(index)
                .context("invalid cached rendition index")?;
            let file = metadata
                .variants
                .get(index)
                .context("invalid cached rendition metadata")?;
            let (_, extension, hash) = stored_identity_with_hash(
                bytes,
                file,
                StoredKind::Rendition,
                &limits,
                cached.hash.clone(),
            )?;
            validate_stored_dimensions(file.width, file.height, &limits)?;
            ensure!(bytes.len() < master.len(), "invalid cached rendition size");
            retained = retained
                .checked_add(bytes.len())
                .context("cached images exceed article limit")?;
            renditions.push(Rendition {
                bytes: Vec::new(),
                extension,
                hash,
                width: file.width,
                height: file.height,
            });
        }
        ensure!(
            retained <= limits.max_article_bytes,
            "cached images exceed article limit"
        );
        Ok((extension, hash, renditions))
    }
}

fn stored_asset_key(
    metadata: &ArticleImage,
    master: &[u8],
    variants: &[Vec<u8>],
) -> Result<String> {
    let implementation = implementation_fingerprint();
    let limits = MediaLimits::default();
    ensure!(
        master.len() <= limits.max_file_bytes
            && variants.len() <= MAX_STORED_RENDITIONS
            && variants
                .iter()
                .all(|bytes| bytes.len() <= limits.max_file_bytes),
        "image cache inputs exceed limits"
    );
    let mut hash = Sha256::new();
    hash.update(implementation);
    let metadata = serde_json::to_vec(metadata)?;
    hash.update((metadata.len() as u64).to_le_bytes());
    hash.update(metadata);
    for bytes in std::iter::once(master).chain(variants.iter().map(Vec::as_slice)) {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    Ok(hex::encode(hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{candidate, png};
    use super::super::{prepare_asset, reset_stored_decode_count, stored_decode_count};
    use super::*;
    use image::{DynamicImage, ImageBuffer};

    #[test]
    fn stored_asset_cache_reuses_validation_across_instances_and_checks_changed_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let cache = StoredAssetCache::new(directory.path());
        let source = DynamicImage::ImageRgb8(ImageBuffer::from_fn(160, 96, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 239) as u8, ((x + y) % 241) as u8])
        }));
        let prepared = prepare_asset(
            &candidate("https://example.com/cache.png"),
            png(&source),
            &MediaLimits::default(),
        )
        .unwrap();
        let metadata = prepared.metadata("cached");
        let variants = prepared
            .renditions
            .iter()
            .map(|r| r.bytes.clone())
            .collect::<Vec<_>>();
        let restore =
            |cache: &StoredAssetCache, metadata: &ArticleImage, variants: Vec<Vec<u8>>| {
                cache.restore(metadata, prepared.master_bytes.clone(), variants)
            };
        reset_stored_decode_count();
        let cold = restore(&cache, &metadata, variants.clone()).unwrap();
        assert!(stored_decode_count() > 0);
        reset_stored_decode_count();
        let warm = restore(
            &StoredAssetCache::new(directory.path()),
            &metadata,
            variants.clone(),
        )
        .unwrap();
        assert_eq!(warm, cold);
        assert_eq!(
            stored_decode_count(),
            0,
            "a reopened cache must skip image decoding and resizing"
        );
        let mut altered = metadata.clone();
        altered.original.width += 1;
        assert!(restore(&cache, &altered, variants.clone()).is_err());
        assert!(stored_decode_count() > 0);
        reset_stored_decode_count();
        let mut damaged = variants;
        damaged[0][0] ^= 1;
        let repaired = restore(&cache, &metadata, damaged).unwrap();
        assert_eq!(repaired.master_bytes, prepared.master_bytes);
        assert!(
            stored_decode_count() > 0,
            "changed bytes must be revalidated"
        );
        assert_ne!(repaired.renditions, cold.renditions);
    }

    #[test]
    fn warm_receipts_transfer_buffers_and_reject_changed_identities() {
        let directory = tempfile::tempdir().unwrap();
        let cache = StoredAssetCache::new(directory.path());
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_fn(320, 192, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 239) as u8, ((x + y) % 241) as u8])
        }));
        let prepared = prepare_asset(
            &candidate("https://example.com/buffers.png"),
            png(&image),
            &MediaLimits::default(),
        )
        .unwrap();
        let metadata = prepared.metadata("buffers");
        let inputs = || {
            prepared
                .renditions
                .iter()
                .map(|part| part.bytes.clone())
                .collect::<Vec<_>>()
        };
        let cold = cache
            .restore(&metadata, prepared.master_bytes.clone(), inputs())
            .unwrap();
        assert!(!cold.renditions.is_empty());
        let master = prepared.master_bytes.clone();
        let master_pointer = master.as_ptr();
        let variants = inputs();
        let pointers = variants.iter().map(Vec::as_ptr).collect::<Vec<_>>();
        let warm = cache.restore(&metadata, master, variants).unwrap();
        assert_eq!(warm, cold);
        assert_eq!(warm.master_bytes.as_ptr(), master_pointer);
        for (variant, pointer) in warm.renditions.iter().zip(pointers) {
            assert_eq!(
                variant.bytes.as_ptr(),
                pointer,
                "warm validation must transfer owned bytes"
            );
        }

        let key = stored_asset_key(&metadata, &prepared.master_bytes, &inputs()).unwrap();
        let path = cache
            .asset_receipts()
            .paths(&key)
            .unwrap()
            .into_iter()
            .find(|path| path.is_file())
            .unwrap();
        let mut receipt = ReceiptSlots::receipt::<ValidatedAsset>(&path).unwrap();
        receipt.value.master_hash = "0".repeat(40);
        receipt.checksum = hex::encode(Sha256::digest(serde_json::to_vec(&receipt.value).unwrap()));
        std::fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        reset_stored_decode_count();
        assert_eq!(
            cache
                .restore(&metadata, prepared.master_bytes.clone(), inputs())
                .unwrap(),
            cold
        );
        assert!(
            stored_decode_count() > 0,
            "inconsistent cached identities require fresh validation"
        );

        let mut changed = prepared.master_bytes.clone();
        let last = changed.len() - 1;
        changed[last] ^= 1;
        assert!(
            cache.restore(&metadata, changed, inputs()).is_err(),
            "every current master byte remains covered by SHA-256"
        );
    }

    #[test]
    #[ignore = "opt-in CPU benchmark for cold and warm retained-image validation"]
    fn benchmark_stored_asset_receipts() {
        let directory = tempfile::tempdir().unwrap();
        let cache = StoredAssetCache::new(directory.path());
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_fn(1600, 900, |x, y| {
            image::Rgb([
                ((x * 17 + y * 13) % 251) as u8,
                ((x ^ y) % 239) as u8,
                ((x * y) % 241) as u8,
            ])
        }));
        let prepared = prepare_asset(
            &candidate("https://example.com/benchmark.png"),
            png(&image),
            &MediaLimits::default(),
        )
        .unwrap();
        let metadata = prepared.metadata("benchmark");
        let inputs = || {
            prepared
                .renditions
                .iter()
                .map(|part| part.bytes.clone())
                .collect::<Vec<_>>()
        };
        let bytes = prepared.master_bytes.len()
            + prepared
                .renditions
                .iter()
                .map(|part| part.bytes.len())
                .sum::<usize>();
        let begin = std::time::Instant::now();
        let expected = cache
            .restore(&metadata, prepared.master_bytes.clone(), inputs())
            .unwrap();
        let cold = begin.elapsed();
        let repetitions = 100;
        reset_stored_decode_count();
        let begin = std::time::Instant::now();
        for _ in 0..repetitions {
            let asset = cache
                .restore(&metadata, prepared.master_bytes.clone(), inputs())
                .unwrap();
            assert_eq!(asset.master_hash, expected.master_hash);
            std::hint::black_box(asset);
        }
        let warm = begin.elapsed();
        assert_eq!(stored_decode_count(), 0);
        // Quantify the byte pass eliminated on receipt hits, separately from allocation/read costs.
        let begin = std::time::Instant::now();
        for _ in 0..repetitions {
            std::hint::black_box(crate::model::sha1_hex(&prepared.master_bytes));
            for part in &prepared.renditions {
                std::hint::black_box(crate::model::sha1_hex(&part.bytes));
                std::hint::black_box(part.bytes.clone());
            }
        }
        let avoided = begin.elapsed();
        eprintln!(
            "image receipt benchmark: bytes={bytes} variants={} cold={cold:?} warm_per_asset={:?} avoided_sha1_and_copies_per_asset={:?} repetitions={repetitions}",
            prepared.renditions.len(),
            warm / repetitions,
            avoided / repetitions
        );
    }

    #[test]
    fn stored_asset_cache_preserves_inline_placeholders_and_recovers_from_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let cache = StoredAssetCache::new(directory.path());
        let prepared = prepare_asset(
            &candidate("https://example.com/legacy-cache.png"),
            png(&DynamicImage::new_rgba8(160, 96)),
            &MediaLimits::default(),
        )
        .unwrap();
        let mut metadata = prepared.metadata("legacy-cache");
        metadata.variants.clear();
        metadata.color = None;
        let restore = || {
            cache
                .restore(&metadata, prepared.master_bytes.clone(), vec![])
                .unwrap()
        };
        let cold = restore();
        assert!(cold.renditions.is_empty());
        assert_eq!(
            placeholder::from_hash(&cold.placeholder.hash).unwrap(),
            cold.placeholder
        );
        reset_stored_decode_count();
        assert_eq!(restore(), cold);
        assert_eq!(stored_decode_count(), 0);
        let receipt = std::fs::read_dir(&cache.root)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(&receipt, b"incomplete cache receipt").unwrap();
        assert_eq!(restore(), cold);
        assert!(stored_decode_count() > 0);
        std::fs::write(&receipt, vec![0; MAX_ASSET_RECEIPT_BYTES + 1]).unwrap();
        reset_stored_decode_count();
        assert_eq!(restore(), cold);
        assert!(stored_decode_count() > 0);
        let mut invalid: Receipt<ValidatedAsset> =
            serde_json::from_slice(&std::fs::read(&receipt).unwrap()).unwrap();
        invalid.value.variants = vec![CachedRendition {
            index: usize::MAX,
            hash: "0".repeat(40),
        }];
        invalid.checksum = hex::encode(Sha256::digest(serde_json::to_vec(&invalid.value).unwrap()));
        std::fs::write(&receipt, serde_json::to_vec(&invalid).unwrap()).unwrap();
        reset_stored_decode_count();
        assert_eq!(restore(), cold);
        assert!(
            stored_decode_count() > 0,
            "invalid receipt indices must fall back to complete validation"
        );
        use base64::Engine as _;
        for bad_hash in [
            base64::engine::general_purpose::STANDARD.encode([0; 5]),
            "A".repeat(100),
        ] {
            let mut invalid = ReceiptSlots::receipt::<ValidatedAsset>(&receipt).unwrap();
            invalid.value.thumbhash = bad_hash;
            invalid.checksum =
                hex::encode(Sha256::digest(serde_json::to_vec(&invalid.value).unwrap()));
            std::fs::write(&receipt, serde_json::to_vec(&invalid).unwrap()).unwrap();
            reset_stored_decode_count();
            assert_eq!(restore(), cold);
            assert!(
                stored_decode_count() > 0,
                "invalid cached ThumbHash must regenerate from the master"
            );
        }
    }

    #[test]
    fn stored_asset_cache_retains_colliding_keys_without_revalidation() {
        let directory = tempfile::tempdir().unwrap();
        let cache = StoredAssetCache {
            root: directory.path().join("bounded"),
            slots: 4,
        };
        let prepared = prepare_asset(
            &candidate("https://example.com/colliding.png"),
            png(&DynamicImage::new_rgba8(80, 48)),
            &MediaLimits::default(),
        )
        .unwrap();
        let inputs = prepared
            .renditions
            .iter()
            .map(|r| r.bytes.clone())
            .collect::<Vec<_>>();
        let metadata = (0..)
            .map(|index| prepared.metadata(&format!("collision-{index}")))
            .filter(|metadata| {
                let key = stored_asset_key(metadata, &prepared.master_bytes, &inputs).unwrap();
                u16::from_str_radix(&key[..4], 16)
                    .unwrap()
                    .is_multiple_of(4)
            })
            .take(4)
            .collect::<Vec<_>>();
        let restore = |metadata: &ArticleImage| {
            cache
                .restore(
                    metadata,
                    prepared.master_bytes.clone(),
                    prepared
                        .renditions
                        .iter()
                        .map(|r| r.bytes.clone())
                        .collect(),
                )
                .unwrap()
        };
        let cold = metadata.iter().map(restore).collect::<Vec<_>>();
        let timestamps = std::fs::read_dir(&cache.root)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.path(), entry.metadata().unwrap().modified().unwrap())
            })
            .collect::<Vec<_>>();
        for _ in 0..2 {
            reset_stored_decode_count();
            assert_eq!(metadata.iter().map(restore).collect::<Vec<_>>(), cold);
            assert_eq!(stored_decode_count(), 0, "collisions must not thrash");
        }
        for (path, modified) in timestamps {
            assert_eq!(
                std::fs::metadata(path).unwrap().modified().unwrap(),
                modified
            );
        }
        restore(&prepared.metadata("overflow"));
        assert_eq!(std::fs::read_dir(&cache.root).unwrap().count(), 4);
    }

    #[test]
    fn stored_asset_cache_collisions_and_unwritable_cache_preserve_full_validation() {
        let directory = tempfile::tempdir().unwrap();
        let cache = StoredAssetCache {
            root: directory.path().join("bounded"),
            slots: 1,
        };
        for suffix in ["one", "two", "one"] {
            let prepared = prepare_asset(
                &candidate(&format!("https://example.com/{suffix}.png")),
                png(&DynamicImage::new_rgba8(80, 48)),
                &MediaLimits::default(),
            )
            .unwrap();
            let metadata = prepared.metadata(suffix);
            reset_stored_decode_count();
            let restored = cache
                .restore(
                    &metadata,
                    prepared.master_bytes.clone(),
                    prepared
                        .renditions
                        .iter()
                        .map(|r| r.bytes.clone())
                        .collect(),
                )
                .unwrap();
            assert_eq!(restored.master_bytes, prepared.master_bytes);
            assert!(stored_decode_count() > 0);
            assert_eq!(std::fs::read_dir(&cache.root).unwrap().count(), 1);
        }
        let blocked = directory.path().join("blocked");
        std::fs::write(&blocked, b"regular file").unwrap();
        let cache = StoredAssetCache {
            root: blocked,
            slots: 1,
        };
        let prepared = prepare_asset(
            &candidate("https://example.com/unwritable.png"),
            png(&DynamicImage::new_rgba8(80, 48)),
            &MediaLimits::default(),
        )
        .unwrap();
        let metadata = prepared.metadata("unwritable");
        assert!(
            cache
                .restore(
                    &metadata,
                    prepared.master_bytes,
                    prepared.renditions.into_iter().map(|r| r.bytes).collect()
                )
                .is_ok()
        );
    }

    #[test]
    fn placeholder_receipts_persist_across_instances_and_reject_invalid_entries() {
        let directory = tempfile::tempdir().unwrap();
        let cache = StoredAssetCache::new(directory.path());
        let bytes = png(&DynamicImage::ImageRgb8(ImageBuffer::from_fn(
            96,
            64,
            |x, y| image::Rgb([(x * 2) as u8, (y * 3) as u8, ((x + y) % 251) as u8]),
        )));
        let hash = crate::model::sha1_hex(&bytes);
        assert!(cache.cached_placeholder(&hash).is_none());

        let placeholder = placeholder::from_bytes(&bytes).unwrap();
        cache.remember_placeholder(&hash, &placeholder);
        assert_eq!(
            StoredAssetCache::new(directory.path()).cached_placeholder(&hash),
            Some(placeholder.clone())
        );
        assert!(cache.cached_placeholder(&"0".repeat(40)).is_none());

        // The sidecar leaves the asset receipt slots at the namespace root untouched.
        let entries = std::fs::read_dir(&cache.root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(entries, [cache.root.join(PLACEHOLDER_RECEIPTS_DIR)]);
        let receipt = std::fs::read_dir(cache.root.join(PLACEHOLDER_RECEIPTS_DIR))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "json"))
            .unwrap();

        // Content hashes that are not hex digests are never stored or looked up.
        for invalid in ["", "not a digest", &"f".repeat(129)] {
            cache.remember_placeholder(invalid, &placeholder);
            assert!(cache.cached_placeholder(invalid).is_none(), "{invalid:?}");
        }
        assert_eq!(
            std::fs::read_dir(cache.root.join(PLACEHOLDER_RECEIPTS_DIR))
                .unwrap()
                .count(),
            1
        );

        // A checksummed receipt whose ThumbHash no longer decodes is ignored, not trusted.
        let mut tampered: Receipt<CachedPlaceholder> =
            serde_json::from_slice(&std::fs::read(&receipt).unwrap()).unwrap();
        tampered.value.thumbhash = "AAAA".into();
        tampered.checksum =
            hex::encode(Sha256::digest(serde_json::to_vec(&tampered.value).unwrap()));
        std::fs::write(&receipt, serde_json::to_vec(&tampered).unwrap()).unwrap();
        assert!(cache.cached_placeholder(&hash).is_none());
        std::fs::write(&receipt, b"not a receipt").unwrap();
        assert!(cache.cached_placeholder(&hash).is_none());

        // Remembering again repairs the slot.
        cache.remember_placeholder(&hash, &placeholder);
        assert_eq!(cache.cached_placeholder(&hash), Some(placeholder));
    }
}
